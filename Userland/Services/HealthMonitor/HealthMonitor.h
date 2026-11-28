/*
 * Copyright (c) 2026, Exile OS contributors
 * SPDX-License-Identifier: BSD-2-Clause
 */
#pragma once

#include "HealthReport.h"
#include <cstdint>
#include <map>
#include <string>
#include <vector>
#include <utility>

namespace Exile::HealthMonitor {

class HealthObservationProvider {
public:
    virtual ~HealthObservationProvider() = default;
    // Return false only when the provider itself could not be queried.
    virtual bool observe(std::vector<HealthObservation>&, std::string& error) = 0;
};

struct ThresholdConfig {
    uint8_t memory_warning_percent { 80 };
    uint8_t memory_critical_percent { 95 };
    uint32_t interval_ms { 5000 };
    uint32_t minimum_interval_ms { 1000 };
    uint32_t maximum_interval_ms { 60000 };
    // Empty means all provider components; otherwise only these components are aggregated.
    std::vector<Component> selected_components;
};

enum class Lifecycle : uint8_t { Stopped, Running, Stopping, Failed };

class HealthMonitor {
public:
    explicit HealthMonitor(HealthObservationProvider&, ThresholdConfig = {});
    bool start();
    void request_stop();
    void stop();
    Lifecycle lifecycle() const { return m_lifecycle; }
    uint32_t interval_ms() const { return m_config.interval_ms; }

    bool observe(uint64_t monotonic_now_ms);
    HealthState overall_state() const { return m_overall; }
    std::map<Component, HealthReport> const& reports() const { return m_reports; }
    std::vector<HealthReport> take_changes();

private:
    static Severity severity_for(HealthState);
    HealthObservationProvider& m_provider;
    ThresholdConfig m_config;
    Lifecycle m_lifecycle { Lifecycle::Stopped };
    HealthState m_overall { HealthState::Unknown };
    std::map<Component, HealthReport> m_reports;
    std::vector<HealthReport> m_changes;
};

class SysfsHealthProvider final : public HealthObservationProvider {
public:
    explicit SysfsHealthProvider(std::string memstat_path = "/sys/kernel/memstat",
        std::string storage_path = "/sys/devices/storage")
        : m_memstat_path(std::move(memstat_path))
        , m_storage_path(std::move(storage_path)) { }
    bool observe(std::vector<HealthObservation>&, std::string& error) override;
private:
    std::string m_memstat_path;
    std::string m_storage_path;
};

uint64_t monotonic_time_ms();

}
