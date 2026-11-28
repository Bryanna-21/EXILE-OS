/*
 * Copyright (c) 2026, Exile OS contributors
 * SPDX-License-Identifier: BSD-2-Clause
 */
#include "StorageMonitor.h"
#include "../MonitoringCommon/MonitorEventPublisher.h"

#include <AK/Debug.h>
#include <LibCore/EventLoop.h>
#include <LibCore/System.h>
#include <LibCore/Timer.h>
#include <LibMain/Main.h>
#include <signal.h>

using namespace Exile::StorageMonitor;

static void terminate_service(int)
{
    Core::EventLoop::current().quit(0);
}

ErrorOr<int> serenity_main(Main::Arguments)
{
    TRY(Core::System::pledge("stdio rpath unix sigaction"));
    TRY(Core::System::unveil("/sys/devices/storage", "r"));
    TRY(Core::System::unveil("/tmp/portal", "rw"));
    TRY(Core::System::unveil(nullptr, nullptr));

    SysfsStorageProvider provider;
    StorageMonitor monitor(provider);
    if (!monitor.start())
        return Error::from_string_literal("StorageMonitor failed to start");

    Core::EventLoop event_loop;
    event_loop.register_signal(SIGTERM, terminate_service);
    auto timer = Core::Timer::create_repeating(monitor.interval_ms(), [&] {
        if (!monitor.observe(monotonic_time_ms()))
            warnln("StorageMonitor: storage discovery failed; retaining last known state");
        for (auto const& error : monitor.take_errors()) {
            warnln("StorageMonitor: {}: {}", error.device_id, error.message);
            auto detail = error.device_id + ": " + error.message;
            (void)Exile::MonitoringCommon::publish_monitor_event("StorageMonitor", detail.c_str());
        }
        for (auto const& change : monitor.take_changes()) {
            dbgln("StorageMonitor: {} availability {} -> {}, status {}",
                change.after.device_id,
                static_cast<unsigned>(change.before.availability),
                static_cast<unsigned>(change.after.availability),
                static_cast<unsigned>(change.after.status));
            auto detail = change.after.device_id + ": " + change.after.diagnostic;
            (void)Exile::MonitoringCommon::publish_monitor_event("StorageMonitor", detail.c_str());
        }
    });
    timer->start();
    dbgln("StorageMonitor: started; polling /sys/devices/storage every {} ms", monitor.interval_ms());
    auto result = event_loop.exec();
    monitor.request_stop();
    timer->stop();
    monitor.stop();
    return result;
}
