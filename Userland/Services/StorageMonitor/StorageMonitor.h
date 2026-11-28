/*
 * Copyright (c) 2026, Exile OS contributors
 * SPDX-License-Identifier: BSD-2-Clause
 */
#pragma once

#include "StorageStatus.h"
#include <cstddef>
#include <cstdint>
#include <map>
#include <string>
#include <vector>
#include <utility>

namespace Exile::StorageMonitor {

class DeviceObservationProvider {
public:
    virtual ~DeviceObservationProvider() = default;
    // false means discovery itself failed; an empty successful list means no devices.
    virtual bool discover(std::vector<StorageObservation>&, std::string& error) = 0;
};

struct MonitorConfig {
    uint32_t interval_ms { 5000 };
    uint32_t minimum_interval_ms { 1000 };
    uint32_t maximum_interval_ms { 60000 };
    size_t maximum_devices { 256 };
};

enum class Lifecycle : uint8_t { Stopped, Running, Stopping, Failed };

struct Change {
    StorageStatus before;
    StorageStatus after;
};

class StorageMonitor {
public:
    explicit StorageMonitor(DeviceObservationProvider&, MonitorConfig = {});
    bool start();
    void request_stop();
    void stop();
    Lifecycle lifecycle() const { return m_lifecycle; }
    uint32_t interval_ms() const { return m_config.interval_ms; }

    // One bounded observation pass. Called by the event-loop timer, never a tight loop.
    bool observe(uint64_t monotonic_now_ms);
    std::map<std::string, StorageStatus> const& statuses() const { return m_statuses; }
    std::vector<Change> take_changes();
    std::vector<StorageError> take_errors();

private:
    static StorageStatus make_status(StorageObservation const&, uint64_t);
    void report(StorageError);

    DeviceObservationProvider& m_provider;
    MonitorConfig m_config;
    Lifecycle m_lifecycle { Lifecycle::Stopped };
    std::map<std::string, StorageStatus> m_statuses;
    std::vector<Change> m_changes;
    std::vector<StorageError> m_errors;
};

class SysfsStorageProvider final : public DeviceObservationProvider {
public:
    explicit SysfsStorageProvider(std::string root = "/sys/devices/storage")
        : m_root(std::move(root)) { }
    bool discover(std::vector<StorageObservation>&, std::string& error) override;
private:
    std::string m_root;
};

uint64_t monotonic_time_ms();

}
