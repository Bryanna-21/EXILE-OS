/*
 * Copyright (c) 2026, Exile OS contributors
 * SPDX-License-Identifier: BSD-2-Clause
 */
#include "StorageMonitor.h"

#include <algorithm>
#include <cerrno>
#include <charconv>
#include <chrono>
#include <cstring>
#include <cstdio>
#include <dirent.h>
#include <limits>
#include <sys/stat.h>

namespace Exile::StorageMonitor {

static bool read_unsigned_file(std::string const& path, uint64_t& value)
{
    FILE* file = fopen(path.c_str(), "r");
    if (!file)
        return false;
    char buffer[64] {};
    auto count = fread(buffer, 1, sizeof(buffer) - 1, file);
    // A value longer than the buffer is malformed for this bounded sysfs parser.
    // Probe one more byte so a long decimal prefix cannot be accepted silently.
    auto extra = fgetc(file);
    bool okay = !ferror(file) && extra == EOF;
    fclose(file);
    if (!okay || count == 0)
        return false;
    auto* first = buffer;
    auto* last = buffer + count;
    while (first != last && (*first == ' ' || *first == '\n' || *first == '\t' || *first == '\r'))
        ++first;
    while (last != first && (last[-1] == ' ' || last[-1] == '\n' || last[-1] == '\t' || last[-1] == '\r'))
        --last;
    if (first == last)
        return false;
    uint64_t parsed = 0;
    auto result = std::from_chars(first, last, parsed, 10);
    if (result.ec != std::errc {} || result.ptr != last)
        return false;
    value = parsed;
    return true;
}

uint64_t monotonic_time_ms()
{
    auto now = std::chrono::steady_clock::now().time_since_epoch();
    return static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::milliseconds>(now).count());
}

StorageMonitor::StorageMonitor(DeviceObservationProvider& provider, MonitorConfig config)
    : m_provider(provider)
    , m_config(config)
{
    if (m_config.minimum_interval_ms == 0)
        m_config.minimum_interval_ms = 1000;
    if (m_config.maximum_interval_ms < m_config.minimum_interval_ms)
        m_config.maximum_interval_ms = m_config.minimum_interval_ms;
    m_config.interval_ms = std::clamp(m_config.interval_ms, m_config.minimum_interval_ms, m_config.maximum_interval_ms);
    if (m_config.maximum_devices == 0)
        m_config.maximum_devices = 1;
    m_config.maximum_devices = std::min<size_t>(m_config.maximum_devices, 4096);
}

bool StorageMonitor::start()
{
    if (m_lifecycle != Lifecycle::Stopped)
        return false;
    m_lifecycle = Lifecycle::Running;
    return true;
}

void StorageMonitor::request_stop()
{
    if (m_lifecycle == Lifecycle::Running)
        m_lifecycle = Lifecycle::Stopping;
}

void StorageMonitor::stop()
{
    if (m_lifecycle == Lifecycle::Running)
        m_lifecycle = Lifecycle::Stopping;
    m_lifecycle = Lifecycle::Stopped;
}

void StorageMonitor::report(StorageError error)
{
    constexpr size_t maximum_queued_errors = 128;
    if (m_errors.size() == maximum_queued_errors)
        m_errors.erase(m_errors.begin());
    m_errors.push_back(std::move(error));
}

StorageStatus StorageMonitor::make_status(StorageObservation const& observation, uint64_t now)
{
    StorageStatus status;
    status.device_id = observation.device_id;
    status.availability = observation.available ? Availability::Available : Availability::Unavailable;
    status.io_errors = observation.io_errors;
    status.health = observation.health;
    status.last_successful_observation_ms = observation.available ? now : 0;
    if (!observation.available) {
        status.status = StatusCode::DeviceDisappeared;
        status.diagnostic = "Device is not currently available";
        return status;
    }
    if (!observation.last_lba.has_value() || !observation.sector_size.has_value()) {
        status.status = observation.io_errors != IoErrorState::Unknown && observation.io_errors != IoErrorState::NoneReported
            ? StatusCode::IoErrorReported : StatusCode::CapacityUnavailable;
        status.diagnostic = status.status == StatusCode::IoErrorReported
            ? "The provider reported an I/O error; capacity attributes are also unavailable"
            : "Capacity attributes are not exposed or could not be read";
        return status;
    }
    auto blocks = *observation.last_lba == std::numeric_limits<uint64_t>::max()
        ? 0
        : *observation.last_lba + 1;
    uint64_t bytes = 0;
    if (blocks == 0 || *observation.sector_size == 0
        || blocks > std::numeric_limits<uint64_t>::max() / *observation.sector_size) {
        status.status = observation.io_errors != IoErrorState::Unknown && observation.io_errors != IoErrorState::NoneReported
            ? StatusCode::IoErrorReported : StatusCode::InvalidCapacity;
        status.diagnostic = status.status == StatusCode::IoErrorReported
            ? "The provider reported an I/O error; capacity geometry is also invalid"
            : "Capacity attributes contain an invalid or overflowing geometry";
        return status;
    }
    bytes = blocks * *observation.sector_size;
    status.capacity = { blocks, *observation.sector_size, bytes, true };
    if (status.io_errors != IoErrorState::Unknown && status.io_errors != IoErrorState::NoneReported) {
        status.status = StatusCode::IoErrorReported;
        status.diagnostic = "The provider reported an I/O error";
    } else {
        status.status = StatusCode::Ok;
    }
    return status;
}

bool StorageMonitor::observe(uint64_t now)
{
    if (m_lifecycle != Lifecycle::Running)
        return false;

    std::vector<StorageObservation> observations;
    std::string discovery_error;
    if (!m_provider.discover(observations, discovery_error)) {
        report({ StatusCode::DiscoveryUnavailable, {}, discovery_error.empty() ? "Storage discovery failed" : discovery_error });
        return false;
    }

    std::map<std::string, StorageStatus> next;
    for (auto const& observation : observations) {
        if (observation.device_id.empty()) {
            report({ StatusCode::ObservationFailed, {}, "Provider returned an empty device identifier" });
            continue;
        }
        if (next.contains(observation.device_id)) {
            report({ StatusCode::DuplicateIdentifier, observation.device_id, "Duplicate device identifier ignored" });
            continue;
        }
        if (next.size() >= m_config.maximum_devices) {
            report({ StatusCode::ObservationFailed, observation.device_id, "Configured device limit reached; remaining devices skipped" });
            break;
        }
        auto status = make_status(observation, now);
        auto old = m_statuses.find(observation.device_id);
        if (old != m_statuses.end()) {
            if (old->second.last_successful_observation_ms != 0 && status.last_successful_observation_ms == 0)
                status.last_successful_observation_ms = old->second.last_successful_observation_ms;
            if (!old->second.equivalent_to(status))
                if (m_changes.size() < 512)
                    m_changes.push_back({ old->second, status });
        } else {
            StorageStatus unknown_before;
            unknown_before.device_id = observation.device_id;
            if (m_changes.size() < 512)
                m_changes.push_back({ unknown_before, status });
        }
        if (status.status == StatusCode::IoErrorReported)
            report({ status.status, status.device_id, status.diagnostic });
        next.emplace(status.device_id, std::move(status));
    }

    // A successful discovery that no longer lists a previously-known device is evidence of
    // disappearance. A failed discovery above deliberately does not mark devices absent.
    for (auto const& [id, old] : m_statuses) {
        if (next.contains(id))
            continue;
        // Current observations take priority. Retain disappearance tombstones only
        // while the bounded status map has room, preventing unbounded growth as
        // removable devices churn over the lifetime of the service.
        if (next.size() >= m_config.maximum_devices)
            break;
        auto missing = old;
        missing.availability = Availability::Unavailable;
        missing.health = DeviceHealth::Unknown;
        missing.io_errors = IoErrorState::Unknown;
        missing.status = StatusCode::DeviceDisappeared;
        missing.diagnostic = "Device disappeared from the storage sysfs directory";
        if (!old.equivalent_to(missing)) {
            if (m_changes.size() < 512)
                m_changes.push_back({ old, missing });
            report({ StatusCode::DeviceDisappeared, id, missing.diagnostic });
        }
        next.emplace(id, std::move(missing));
    }

    m_statuses = std::move(next);
    return true;
}

std::vector<Change> StorageMonitor::take_changes()
{
    auto changes = std::move(m_changes);
    m_changes.clear();
    return changes;
}

std::vector<StorageError> StorageMonitor::take_errors()
{
    auto errors = std::move(m_errors);
    m_errors.clear();
    return errors;
}

bool SysfsStorageProvider::discover(std::vector<StorageObservation>& output, std::string& error)
{
    output.clear();
    DIR* directory = opendir(m_root.c_str());
    if (!directory) {
        error = "Cannot open " + m_root + ": " + std::strerror(errno);
        return false;
    }
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
        std::string path = m_root + "/" + entry->d_name;
        struct stat st {};
        if (stat(path.c_str(), &st) < 0 || !S_ISDIR(st.st_mode))
            continue;
        constexpr size_t maximum_discovered_devices = 256;
        if (output.size() >= maximum_discovered_devices) {
            closedir(directory);
            error = "Storage inventory exceeds the service's 256-device bound";
            return false;
        }
        StorageObservation observation;
        observation.device_id = entry->d_name;
        observation.available = true;
        uint64_t last_lba = 0;
        uint64_t sector_size = 0;
        if (read_unsigned_file(path + "/last_lba", last_lba))
            observation.last_lba = last_lba;
        if (read_unsigned_file(path + "/sector_size", sector_size)
            && sector_size <= std::numeric_limits<uint32_t>::max())
            observation.sector_size = static_cast<uint32_t>(sector_size);
        // SysFS currently exposes last_lba, sector_size and command_set only. It has no
        // SMART, read/write error counters, temperature or media-health attribute.
        output.push_back(std::move(observation));
    }
    closedir(directory);
    if (saved_errno != 0) {
        error = "Error while enumerating " + m_root + ": " + std::strerror(saved_errno);
        return false;
    }
    return true;
}

}
