/*
 * Copyright (c) 2026, Exile OS contributors
 * SPDX-License-Identifier: BSD-2-Clause
 */
#pragma once

#include <cstdint>
#include <optional>
#include <string>

namespace Exile::StorageMonitor {

enum class Availability : uint8_t { Unknown, Available, Unavailable };
enum class IoErrorState : uint8_t { Unknown, NoneReported, Read, Write, ReadWrite };
enum class DeviceHealth : uint8_t { Unknown, Healthy, Degraded, Failed };
enum class StatusCode : uint8_t {
    Ok, DiscoveryUnavailable, DuplicateIdentifier, DeviceDisappeared,
    CapacityUnavailable, InvalidCapacity, ObservationFailed, IoErrorReported
};

struct CapacityInfo {
    uint64_t logical_blocks { 0 };
    uint32_t block_size_bytes { 0 };
    uint64_t total_bytes { 0 };
    bool known { false };
};

struct StorageStatus {
    std::string device_id;
    Availability availability { Availability::Unknown };
    CapacityInfo capacity;
    IoErrorState io_errors { IoErrorState::Unknown };
    DeviceHealth health { DeviceHealth::Unknown };
    // CLOCK_MONOTONIC milliseconds; zero means no successful observation yet.
    uint64_t last_successful_observation_ms { 0 };
    StatusCode status { StatusCode::ObservationFailed };
    std::string diagnostic;

    bool equivalent_to(StorageStatus const& other) const
    {
        return device_id == other.device_id
            && availability == other.availability
            && capacity.logical_blocks == other.capacity.logical_blocks
            && capacity.block_size_bytes == other.capacity.block_size_bytes
            && capacity.total_bytes == other.capacity.total_bytes
            && capacity.known == other.capacity.known
            && io_errors == other.io_errors
            && health == other.health
            && status == other.status
            && diagnostic == other.diagnostic;
    }
};

struct StorageObservation {
    std::string device_id;
    bool available { true };
    std::optional<uint64_t> last_lba;
    std::optional<uint32_t> sector_size;
    // The current kernel sysfs interface does not expose read/write error counters.
    IoErrorState io_errors { IoErrorState::Unknown };
    DeviceHealth health { DeviceHealth::Unknown };
};

struct StorageError {
    StatusCode code { StatusCode::ObservationFailed };
    std::string device_id;
    std::string message;
};

}
