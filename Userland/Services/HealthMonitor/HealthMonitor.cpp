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
#include <fstream>
#include <dirent.h>
#include <limits>
#include <sstream>
#include <sys/stat.h>

namespace Exile::HealthMonitor {

static bool parse_json_unsigned(std::string const& text, std::string const& key, uint64_t& value)
{
    auto position = text.find('"' + key + '"');
    if (position == std::string::npos)
        return false;
    position = text.find(':', position + key.size() + 2);
    if (position == std::string::npos)
        return false;
    ++position;
    while (position < text.size() && (text[position] == ' ' || text[position] == '\t' || text[position] == '\r' || text[position] == '\n'))
        ++position;

    uint64_t parsed = 0;
    auto result = std::from_chars(text.data() + position, text.data() + text.size(), parsed, 10);
    if (result.ec != std::errc {} || result.ptr == text.data() + position)
        return false;
    auto end = static_cast<size_t>(result.ptr - text.data());
    while (end < text.size() && (text[end] == ' ' || text[end] == '\t' || text[end] == '\r' || text[end] == '\n'))
        ++end;
    if (end >= text.size() || (text[end] != ',' && text[end] != '}'))
        return false;
    value = parsed;
    return true;
}

static bool storage_directory_available(std::string const& path, size_t& count)
{
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
        if (stat(child.c_str(), &st) == 0 && S_ISDIR(st.st_mode))
            ++count;
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
        auto reason = error.empty()
            ? std::string("Health provider unavailable; no component health can be confirmed")
            : std::string("Health provider unavailable: ") + error;

        // Do not leave previously healthy observations visible as current after
        // the provider stops responding. Transition every retained component to
        // Unknown, and record only state/reason changes so repeated failed polls
        // cannot fill the bounded change queue with duplicates.
        for (auto& [component, previous] : m_reports) {
            HealthReport unavailable { component, HealthState::Unknown,
                Severity::Informational, now, reason };
            if ((previous.state != unavailable.state || previous.reason != unavailable.reason)
                && m_changes.size() < 512)
                m_changes.push_back(unavailable);
            previous = std::move(unavailable);
        }

        HealthReport critical_component { Component::CriticalComponent, HealthState::Unknown,
            Severity::Informational, now, reason };
        auto [critical, inserted] = m_reports.try_emplace(Component::CriticalComponent, critical_component);
        if (!inserted) {
            if ((critical->second.state != critical_component.state
                    || critical->second.reason != critical_component.reason)
                && m_changes.size() < 512)
                m_changes.push_back(critical_component);
            critical->second = std::move(critical_component);
        } else if (m_changes.size() < 512) {
            m_changes.push_back(critical_component);
        }

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
    if (storage_directory_available(m_storage_path, devices)) {
        // Enumeration proves only that sysfs currently lists devices. The kernel
        // exposes capacity and command-set metadata, not a device health verdict.
        output.push_back({ Component::Storage, HealthState::Unknown,
            devices ? std::to_string(devices) + " storage device(s) enumerated; device health telemetry is unavailable"
                    : "Storage sysfs is readable but currently lists no storage devices; absence alone is not a device failure" });
    } else {
        output.push_back({ Component::Storage, HealthState::Unknown,
            "Storage device inventory unavailable at " + m_storage_path });
    }

    output.push_back({ Component::SystemService, HealthState::Unknown,
        "No service health reporting registry/provider exists in this baseline" });
    output.push_back({ Component::ResourcePressure, HealthState::Unknown,
        "CPU/load and non-memory resource pressure telemetry are not exposed to this service" });
    output.push_back({ Component::CriticalComponent, HealthState::Unknown,
        "Critical-component health provider is not available in this baseline" });
    error.clear();
    return true;
}

}
