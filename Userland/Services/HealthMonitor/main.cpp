/*
 * Copyright (c) 2026, Exile OS contributors
 * SPDX-License-Identifier: BSD-2-Clause
 */
#include "HealthMonitor.h"
#include "../MonitoringCommon/MonitorEventPublisher.h"

#include <AK/Debug.h>
#include <LibCore/EventLoop.h>
#include <LibCore/System.h>
#include <LibCore/Timer.h>
#include <LibMain/Main.h>
#include <signal.h>

using namespace Exile::HealthMonitor;

static void terminate_service(int)
{
    Core::EventLoop::current().quit(0);
}

ErrorOr<int> serenity_main(Main::Arguments)
{
    TRY(Core::System::pledge("stdio rpath unix sigaction"));
    TRY(Core::System::unveil("/sys/kernel/memstat", "r"));
    TRY(Core::System::unveil("/sys/devices/storage", "r"));
    TRY(Core::System::unveil("/tmp/portal", "rw"));
    TRY(Core::System::unveil(nullptr, nullptr));

    SysfsHealthProvider provider;
    HealthMonitor monitor(provider);
    if (!monitor.start())
        return Error::from_string_literal("HealthMonitor failed to start");

    Core::EventLoop event_loop;
    event_loop.register_signal(SIGTERM, terminate_service);
    auto timer = Core::Timer::create_repeating(monitor.interval_ms(), [&] {
        if (!monitor.observe(monotonic_time_ms()))
            warnln("HealthMonitor: observation provider failed; health remains unknown");
        for (auto const& report : monitor.take_changes()) {
            auto detail = report.reason;
            dbgln("HealthMonitor: component {} state {} severity {}: {}",
                static_cast<unsigned>(report.component),
                static_cast<unsigned>(report.state),
                static_cast<unsigned>(report.severity), detail);
            (void)Exile::MonitoringCommon::publish_monitor_event("HealthMonitor", detail.c_str());
        }
    });
    timer->start();
    dbgln("HealthMonitor: started; sampling available sysfs health data every {} ms", monitor.interval_ms());
    auto result = event_loop.exec();
    monitor.request_stop();
    timer->stop();
    monitor.stop();
    return result;
}
