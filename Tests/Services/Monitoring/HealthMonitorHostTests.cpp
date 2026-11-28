/*
 * Copyright (c) 2026, Exile OS contributors
 * SPDX-License-Identifier: BSD-2-Clause
 */
#include "HealthMonitor.h"
#include "../../../Userland/Services/MonitoringCommon/MonitorEventPublisher.h"

#include <cassert>
#include <algorithm>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>
#include <vector>
#include <unistd.h>

using namespace Exile::HealthMonitor;

class FixedProvider final : public HealthObservationProvider {
public:
    bool available { true };
    std::vector<HealthObservation> observations;
    bool observe(std::vector<HealthObservation>& output, std::string& error) override
    {
        if (!available) {
            error = "fixture provider unavailable";
            return false;
        }
        output = observations;
        error.clear();
        return true;
    }
};

static std::string read_file(std::filesystem::path const& path)
{
    std::ifstream stream(path);
    assert(stream.good());
    return { std::istreambuf_iterator<char>(stream), std::istreambuf_iterator<char>() };
}

int main()
{
    FixedProvider provider;
    ThresholdConfig config;
    config.memory_warning_percent = 80;
    config.memory_critical_percent = 95;
    HealthMonitor monitor(provider, config);
    assert(monitor.start());
    assert(!monitor.start());

    provider.observations = { { Component::Memory, HealthState::Unknown, "79 percent", 79, true } };
    assert(monitor.observe(1000));
    assert(monitor.reports().at(Component::Memory).state == HealthState::Healthy);

    provider.observations[0].measured_percent = 80;
    assert(monitor.observe(2000));
    assert(monitor.reports().at(Component::Memory).state == HealthState::Warning);

    provider.observations[0].measured_percent = 95;
    assert(monitor.observe(3000));
    assert(monitor.reports().at(Component::Memory).state == HealthState::Critical);

    provider.observations[0].has_measurement = false;
    provider.observations[0].state = HealthState::Unknown;
    assert(monitor.observe(4000));
    assert(monitor.reports().at(Component::Memory).state == HealthState::Unknown);

    // Establish a healthy reading, then ensure provider failure invalidates
    // the stale result instead of leaving it visible as current health.
    provider.available = true;
    provider.observations = { { Component::Memory, HealthState::Unknown, "10 percent", 10, true } };
    assert(monitor.observe(4500));
    assert(monitor.reports().at(Component::Memory).state == HealthState::Healthy);

    provider.available = false;
    assert(!monitor.observe(5000));
    assert(monitor.overall_state() == HealthState::Unknown);
    assert(monitor.reports().at(Component::Memory).state == HealthState::Unknown);
    auto failure_changes = monitor.take_changes();
    assert(!failure_changes.empty());
    assert(!monitor.observe(6000));
    assert(monitor.take_changes().empty()); // repeated failure is not re-announced
    monitor.request_stop();
    monitor.stop();
    assert(monitor.lifecycle() == Lifecycle::Stopped);

    auto root = std::filesystem::temp_directory_path() / ("exile-health-test-" + std::to_string(getpid()));
    std::filesystem::remove_all(root);
    std::filesystem::create_directories(root / "storage" / "00:00.0");
    auto fixture_root = std::filesystem::path("Tests/Services/Monitoring/fixtures");
    {
        std::ofstream output(root / "memstat");
        output << read_file(fixture_root / "memstat-valid.json");
    }
    SysfsHealthProvider sysfs((root / "memstat").string(), (root / "storage").string());
    std::vector<HealthObservation> observations;
    std::string error;
    assert(sysfs.observe(observations, error));
    auto memory = std::find_if(observations.begin(), observations.end(), [](auto const& item) { return item.component == Component::Memory; });
    auto storage = std::find_if(observations.begin(), observations.end(), [](auto const& item) { return item.component == Component::Storage; });
    assert(memory != observations.end() && memory->has_measurement && memory->measured_percent == 80);
    assert(storage != observations.end() && storage->state == HealthState::Unknown);
    assert(storage->reason.find("device health telemetry is unavailable") != std::string::npos);

    {
        std::ofstream output(root / "memstat", std::ios::trunc);
        output << read_file(fixture_root / "memstat-malformed.json");
    }
    assert(sysfs.observe(observations, error));
    memory = std::find_if(observations.begin(), observations.end(), [](auto const& item) { return item.component == Component::Memory; });
    assert(memory != observations.end() && !memory->has_measurement && memory->state == HealthState::Unknown);
    std::filesystem::remove_all(root);

    char record[128] {};
    auto length = Exile::MonitoringCommon::format_monitor_event(record, sizeof(record), "HealthMonitor", "line one\nline two");
    assert(length > 0);
    assert(std::string(record).find('\n') == std::string::npos);
    assert(std::string(record).starts_with("<5>HealthMonitor: "));

    std::cout << "HealthMonitor host tests: PASS\n";
    return 0;
}
