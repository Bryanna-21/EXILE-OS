/*
 * Copyright (c) 2026, Exile OS contributors
 * SPDX-License-Identifier: BSD-2-Clause
 */
#include "StorageMonitor.h"

#include <cassert>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <limits>
#include <string>
#include <unistd.h>
#include <vector>

using namespace Exile::StorageMonitor;

class FixedProvider final : public DeviceObservationProvider {
public:
    bool available { true };
    std::vector<StorageObservation> observations;
    bool discover(std::vector<StorageObservation>& output, std::string& error) override
    {
        if (!available) {
            error = "fixture discovery unavailable";
            return false;
        }
        output = observations;
        error.clear();
        return true;
    }
};

int main()
{
    FixedProvider provider;
    StorageMonitor monitor(provider);
    assert(monitor.start());
    assert(!monitor.start());

    StorageObservation disk;
    disk.device_id = "00:00.0";
    disk.last_lba = 2047;
    disk.sector_size = 512;
    provider.observations = { disk };
    assert(monitor.observe(1000));
    auto const& status = monitor.statuses().at(disk.device_id);
    assert(status.availability == Availability::Available);
    assert(status.capacity.known && status.capacity.total_bytes == 1024 * 1024);
    assert(status.health == DeviceHealth::Unknown);
    assert(status.io_errors == IoErrorState::Unknown);

    provider.available = false;
    assert(!monitor.observe(2000));
    assert(monitor.statuses().contains(disk.device_id));
    assert(!monitor.take_errors().empty());

    provider.available = true;
    provider.observations.clear();
    assert(monitor.observe(3000));
    assert(monitor.statuses().at(disk.device_id).status == StatusCode::DeviceDisappeared);
    assert(monitor.statuses().at(disk.device_id).health == DeviceHealth::Unknown);

    disk.last_lba = std::numeric_limits<uint64_t>::max();
    provider.observations = { disk };
    assert(monitor.observe(4000));
    assert(monitor.statuses().at(disk.device_id).status == StatusCode::InvalidCapacity);

    auto root = std::filesystem::temp_directory_path() / ("exile-storage-test-" + std::to_string(getpid()));
    std::filesystem::remove_all(root);
    auto device_path = root / "01:00.0";
    std::filesystem::create_directories(device_path);
    {
        std::ofstream(device_path / "last_lba") << "4095\n";
        std::ofstream(device_path / "sector_size") << "512\n";
        std::ofstream(device_path / "command_set") << "nvme\n";
    }
    SysfsStorageProvider sysfs(root.string());
    std::vector<StorageObservation> found;
    std::string error;
    assert(sysfs.discover(found, error));
    assert(found.size() == 1);
    assert(found[0].last_lba == 4095);
    assert(found[0].sector_size == 512);
    assert(found[0].health == DeviceHealth::Unknown);
    assert(found[0].io_errors == IoErrorState::Unknown);
    std::filesystem::remove_all(root);

    monitor.request_stop();
    monitor.stop();
    assert(monitor.lifecycle() == Lifecycle::Stopped);
    std::cout << "StorageMonitor host tests: PASS\n";
    return 0;
}
