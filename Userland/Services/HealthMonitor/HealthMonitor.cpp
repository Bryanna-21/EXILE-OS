/*
 * Copyright (c) 2026, Exile OS contributors
 * SPDX-License-Identifier: BSD-2-Clause
 */
#include "HealthMonitor.h"

#include <algorithm>
#include <array>
#include <charconv>
#include <chrono>
#include <cerrno>
#include <cstring>
#include <dirent.h>
#include <fstream>
#include <limits>
#include <sstream>
#include <sys/stat.h>

namespace Exile::HealthMonitor {

static bool parse_json_unsigned(std::string const& text, std::string const& key, uint64_t& value)
{
    auto position = text.find('"' + key + '"');
    if (position == std::string::npos)
        return false;
    position = text.find(':', position);
    if (position == std::string::npos)
        return false;
    ++position;
    while (position < text.size() && (text[position] == ' ' || text[position] == '\t'))
        ++position;
    auto begin = text.data() + position;
    auto end = text.data() + text.size();
    uint64_t parsed = 0;
    auto result = std::from_chars(begin, end, parsed, 10);
    if (result.ec != std::errc {} || result.ptr == begin)
        return false;

    // Accept only a complete JSON integer token. A prefix such as `42garbage`
    // must not be treated as a valid health measurement.
    auto* trailing = result.ptr;
    while (trailing != end && (*trailing == ' ' || *trailing == '\t'
        || *trailing == '\n' || *trailing == '\r'))
        ++trailing;
    if (trailing != end && *trailing != ',' && *trailing != '}')
        return false;

    value = parsed;
    return true;
}

static bool storage_directory_available(std::string const& path, size_t& count, bool& exceeded_bound)
{
    constexpr size_t maximum_inventory_entries = 256;
    exceeded_bound = false;
    DIR* directory = opendir(path.c_str());
    if (!directory)
        return false;
    count = 0;
    int saved_errno = 0;
    while (true) {
        errno = 0;
        auto* entry = readdir(directory);
        if (!entry) {
            saved_errno = errno;
            break;
        }
        if (entry->d_name[0] == '.')
            continue;
        std::string child = path + "/" + entry->d_name;
        struct stat st {};
        if (stat(child.c_str(), &st) == 0 && S_ISDIR(st.st_mode)) {
            if (count == maximum_inventory_entries) {
                exceeded_bound = true;
                closedir(directory);
                return false;
            }
            ++count;
        }
    }
    closedir(directory);
    return saved_errno == 0;
}

uint64_t monotonic_time_ms()
{
    auto now = std::chrono::steady_clock::now().time_since_epoch();
    return static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::milliseconds>(now).count());
}

HealthMonitor::HealthMonitor(HealthObservationProvider& provider, ThresholdConfig config)
    : m_provider(provider)
    , m_config(config)
{
    m_config.memory_warning_percent = std::min<uint8_t>(m_config.memory_warning_percent, 100);
    m_config.memory_critical_percent = std::clamp<uint8_t>(m_config.memory_critical_percent, m_config.memory_warning_percent, 100);
    if (m_config.minimum_interval_ms == 0)
        m_config.minimum_interval_ms = 1000;
    if (m_config.maximum_interval_ms < m_config.minimum_interval_ms)
        m_config.maximum_interval_ms = m_config.minimum_interval_ms;
    m_config.interval_ms = std::clamp(m_config.interval_ms, m_config.minimum_interval_ms, m_config.maximum_interval_ms);
}

bool HealthMonitor::start()
{
    if (m_lifecycle != Lifecycle::Stopped)
        return false;
    m_lifecycle = Lifecycle::Running;
    return true;
}

void HealthMonitor::request_stop()
{
    if (m_lifecycle == Lifecycle::Running)
        m_lifecycle = Lifecycle::Stopping;
}

void HealthMonitor::stop()
{
    if (m_lifecycle == Lifecycle::Running)
        m_lifecycle = Lifecycle::Stopping;
    m_lifecycle = Lifecycle::Stopped;
}

Severity HealthMonitor::severity_for(HealthState state)
{
    switch (state) {
    case HealthState::Critical: return Severity::Critical;
    case HealthState::Warning: return Severity::Warning;
    case HealthState::Unknown:
    case HealthState::Healthy: return Severity::Informational;
    }
    return Severity::Informational;
}

bool HealthMonitor::observe(uint64_t now)
{
    if (m_lifecycle != Lifecycle::Running)
        return false;
    std::vector<HealthObservation> observations;
    std::string error;
    if (!m_provider.observe(observations, error)) {
        HealthReport report { Component::CriticalComponent, HealthState::Unknown,
            Severity::Informational, now,
            error.empty() ? "Health provider unavailable; no failure inferred" : error };
        auto old = m_reports.find(report.component);
        if (old == m_reports.end() || old->second.state != report.state || old->second.reason != report.reason) {
            if (m_changes.size() < 512)
                m_changes.push_back(report);
        }
        // Do not persist a synthetic failed-component entry: a later successful provider
        // sample must be able to recover without inheriting a fabricated component failure.
        m_overall = HealthState::Unknown;
        return false;
    }

    std::map<Component, HealthReport> next;
    for (auto const& observation : observations) {
        if (!m_config.selected_components.empty()
            && std::find(m_config.selected_components.begin(), m_config.selected_components.end(), observation.component) == m_config.selected_components.end())
            continue;
        auto state = observation.state;
        if (observation.component == Component::Memory && observation.has_measurement) {
            if (observation.measured_percent >= m_config.memory_critical_percent)
                state = HealthState::Critical;
            else if (observation.measured_percent >= m_config.memory_warning_percent)
                state = HealthState::Warning;
            else
                state = HealthState::Healthy;
        }
        HealthReport report { observation.component, state, severity_for(state), now, observation.reason };
        // Duplicate component observations are ambiguous; preserve first and do not fabricate a state.
        if (next.contains(observation.component))
            continue;
        next.emplace(observation.component, std::move(report));
    }

    for (auto component : m_config.selected_components) {
        if (!next.contains(component)) {
            next.emplace(component, HealthReport { component, HealthState::Unknown,
                Severity::Informational, now, "Selected component has no available observation provider" });
        }
    }

    HealthState aggregate = HealthState::Healthy;
    bool any = false;
    bool unknown = false;
    for (auto const& [component, report] : next) {
        (void)component;
        any = true;
        if (report.state == HealthState::Critical)
            aggregate = HealthState::Critical;
        else if (report.state == HealthState::Warning && aggregate != HealthState::Critical)
            aggregate = HealthState::Warning;
        else if (report.state == HealthState::Unknown)
            unknown = true;
    }
    if (!any || (aggregate == HealthState::Healthy && unknown))
        aggregate = HealthState::Unknown;

    for (auto const& [component, report] : next) {
        auto old = m_reports.find(component);
        if (old == m_reports.end() || old->second.state != report.state || old->second.reason != report.reason)
            if (m_changes.size() < 512)
                m_changes.push_back(report);
    }
    // A previously observed component omitted from a successful sample becomes explicitly unknown.
    for (auto const& [component, old] : m_reports) {
        if (next.contains(component))
            continue;
        HealthReport missing { component, HealthState::Unknown, Severity::Informational, now,
            "Provider did not return this previously observed component" };
        if (old.state != missing.state || old.reason != missing.reason)
            if (m_changes.size() < 512)
                m_changes.push_back(missing);
        next.emplace(component, std::move(missing));
        if (aggregate == HealthState::Healthy)
            aggregate = HealthState::Unknown;
    }
    m_reports = std::move(next);
    m_overall = aggregate;
    return true;
}

std::vector<HealthReport> HealthMonitor::take_changes()
{
    auto changes = std::move(m_changes);
    m_changes.clear();
    return changes;
}

bool SysfsHealthProvider::observe(std::vector<HealthObservation>& output, std::string& error)
{
    output.clear();
    std::ifstream memstat(m_memstat_path);
    if (memstat) {
        std::array<char, 4096> buffer {};
        memstat.read(buffer.data(), static_cast<std::streamsize>(buffer.size()));
        auto bytes_read = memstat.gcount();
        bool exceeded_bound = !memstat.eof();
        if (exceeded_bound)
            bytes_read = 0;
        std::string json(buffer.data(), static_cast<size_t>(bytes_read));
        uint64_t allocated = 0, available = 0;
        if (exceeded_bound) {
            output.push_back({ Component::Memory, HealthState::Unknown,
                "Kernel memstat exceeded the 4096-byte parsing bound" });
        } else if (bytes_read > 0 && parse_json_unsigned(json, "physical_allocated", allocated)
            && parse_json_unsigned(json, "physical_available", available)
            && allocated <= std::numeric_limits<uint64_t>::max() - available
            && allocated + available != 0) {
            auto total = allocated + available;
            auto percent = static_cast<uint8_t>((static_cast<long double>(allocated) * 100.0L) / static_cast<long double>(total));
            output.push_back({ Component::Memory, HealthState::Unknown,
                "Physical memory allocated: " + std::to_string(percent) + "% (kernel memstat)",
                percent, true });
        } else {
            output.push_back({ Component::Memory, HealthState::Unknown,
                "Kernel memstat is present but physical memory fields are missing or invalid" });
        }
    } else {
        output.push_back({ Component::Memory, HealthState::Unknown,
            "Kernel memory statistics unavailable at " + m_memstat_path });
    }

    size_t devices = 0;
    bool inventory_exceeded_bound = false;
    if (storage_directory_available(m_storage_path, devices, inventory_exceeded_bound)) {
        // Enumeration proves that inventory is readable, not that the devices are healthy.
        // This sysfs interface does not expose SMART or media-health telemetry.
        output.push_back({ Component::Storage, HealthState::Unknown,
            devices ? std::to_string(devices) + " storage device(s) enumerated; device health telemetry is unavailable"
                    : "Storage sysfs is readable but currently lists no storage devices; absence alone is not a device failure" });
    } else {
        output.push_back({ Component::Storage, HealthState::Unknown,
            inventory_exceeded_bound
                ? "Storage inventory exceeds the 256-device observation bound"
                : "Storage device inventory unavailable at " + m_storage_path });
    }

    // There is no service-health IPC/registry contract in the inspected baseline. Do not
    // infer process health from names or assume a service is failed because no status is published.
    output.push_back({ Component::SystemService, HealthState::Unknown,
        "No service health reporting IPC/provider exists in this baseline" });
    output.push_back({ Component::ResourcePressure, HealthState::Unknown,
        "CPU/load and non-memory resource pressure telemetry are not exposed to this service" });
    output.push_back({ Component::CriticalComponent, HealthState::Unknown,
        "Critical-component health provider is not available in this baseline" });
    error.clear();
    return true;
}

}
