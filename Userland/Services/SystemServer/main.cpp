/*
 * Copyright (c) 2018-2021, Andreas Kling <kling@serenityos.org>
 * Copyright (c) 2021, Peter Elliott <pelliott@serenityos.org>
 * Copyright (c) 2023, Liav A. <liavalb@hotmail.co.il>
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include "Service.h"
#include <AK/Assertions.h>
#include <AK/ByteBuffer.h>
#include <AK/Debug.h>
#include <AK/String.h>
#include <AK/Vector.h>
#include <Kernel/API/DeviceEvent.h>
#include <LibCore/ArgsParser.h>
#include <LibCore/ConfigFile.h>
#include <LibCore/DirIterator.h>
#include <LibCore/Event.h>
#include <LibCore/EventLoop.h>
#include <LibCore/File.h>
#include <LibCore/FileWatcher.h>
#include <LibCore/System.h>
#include <LibCore/Timer.h>
#include <LibMain/Main.h>
#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <signal.h>
#include <stdio.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

static constexpr StringView text_system_mode = "text"sv;
static constexpr StringView selftest_system_mode = "self-test"sv;
static constexpr StringView graphical_system_mode = "graphical"sv;
ByteString g_system_mode = graphical_system_mode;
Vector<NonnullRefPtr<Service>> g_services;

// NOTE: This handler ensures that the destructor of g_services is called.
static void sigterm_handler(int)
{
    exit(0);
}

static void sigchld_handler(int)
{
    for (;;) {
        int status = 0;
        pid_t pid = waitpid(-1, &status, WNOHANG);
        if (pid < 0) {
            perror("waitpid");
            break;
        }
        if (pid == 0)
            break;

        dbgln_if(SYSTEMSERVER_DEBUG, "Reaped child with pid {}, exit status {}", pid, status);

        Service* service = Service::find_by_pid(pid);
        if (service == nullptr) {
            // This can happen for multi-instance services.
            continue;
        }

        if (auto result = service->did_exit(status); result.is_error())
            dbgln("{}: {}", service->name(), result.release_error());
    }
}

namespace SystemServer {

static ErrorOr<void> determine_system_mode()
{
    ArmedScopeGuard declare_text_mode_on_failure([&] {
        // Note: Only if the mode is not set to self-test, degrade it to text mode.
        if (g_system_mode != selftest_system_mode)
            g_system_mode = text_system_mode;
    });

    auto file_or_error = Core::File::open("/sys/kernel/system_mode"sv, Core::File::OpenMode::Read);
    if (file_or_error.is_error()) {
        dbgln("Failed to read system_mode: {}", file_or_error.error());
        // Continue and assume "text" mode.
        return {};
    }
    auto const system_mode_buf_or_error = file_or_error.value()->read_until_eof();
    if (system_mode_buf_or_error.is_error()) {
        dbgln("Failed to read system_mode: {}", system_mode_buf_or_error.error());
        // Continue and assume "text" mode.
        return {};
    }
    ByteString const system_mode = ByteString::copy(system_mode_buf_or_error.value(), Chomp);

    g_system_mode = system_mode;
    declare_text_mode_on_failure.disarm();

    dbgln("Read system_mode: {}", g_system_mode);
    return {};
}

static Optional<size_t> find_service_index(Vector<NonnullRefPtr<Service>> const& services, StringView name)
{
    for (size_t i = 0; i < services.size(); ++i) {
        if (services[i]->name() == name)
            return i;
    }
    return {};
}

// Return a stable topological order. Missing or mode-disabled dependencies block
// the service and its dependents; a cycle is rejected before any service is spawned.
static ErrorOr<Vector<size_t>> service_startup_order(Core::ConfigFile const& config, Vector<NonnullRefPtr<Service>> const& services)
{
    Vector<u8> blocked;
    Vector<u8> scheduled;
    TRY(blocked.try_resize(services.size()));
    TRY(scheduled.try_resize(services.size()));
    for (size_t i = 0; i < services.size(); ++i) {
        blocked[i] = 0;
        scheduled[i] = 0;
    }

    auto dependencies_for = [&](size_t index) {
        auto service_name = ByteString::copy(services[index]->name());
        return config.read_entry(service_name, "Dependencies").split(',');
    };

    // First mark direct references that are absent from this active configuration.
    for (size_t i = 0; i < services.size(); ++i) {
        for (auto const& dependency : dependencies_for(i)) {
            if (dependency.is_empty())
                continue;
            if (!find_service_index(services, dependency)) {
                warnln("Service {} is blocked: dependency {} is not enabled in this system mode", services[i]->name(), dependency);
                blocked[i] = 1;
            }
        }
    }

    // Propagate dependency failure before ordering, so no dependent starts after
    // an unavailable dependency has been discovered.
    bool changed = true;
    while (changed) {
        changed = false;
        for (size_t i = 0; i < services.size(); ++i) {
            if (blocked[i])
                continue;
            for (auto const& dependency : dependencies_for(i)) {
                if (dependency.is_empty())
                    continue;
                auto dependency_index = find_service_index(services, dependency);
                if (!dependency_index.has_value() || blocked[dependency_index.value()]) {
                    warnln("Service {} is blocked because dependency {} is unavailable", services[i]->name(), dependency);
                    blocked[i] = 1;
                    changed = true;
                    break;
                }
            }
        }
    }

    Vector<size_t> order;
    bool made_progress = true;
    while (made_progress) {
        made_progress = false;
        for (size_t i = 0; i < services.size(); ++i) {
            if (blocked[i] || scheduled[i])
                continue;
            bool dependencies_scheduled = true;
            for (auto const& dependency : dependencies_for(i)) {
                if (dependency.is_empty())
                    continue;
                auto dependency_index = find_service_index(services, dependency);
                if (dependency_index.has_value() && !scheduled[dependency_index.value()]) {
                    dependencies_scheduled = false;
                    break;
                }
            }
            if (!dependencies_scheduled)
                continue;
            TRY(order.try_append(i));
            scheduled[i] = 1;
            made_progress = true;
        }
    }

    for (size_t i = 0; i < services.size(); ++i) {
        if (!blocked[i] && !scheduled[i])
            return Error::from_string_literal("Circular service dependency detected; no services in this configuration were started");
    }
    return order;
}

static ErrorOr<void> activate_services(Core::ConfigFile const& config)
{
    Vector<NonnullRefPtr<Service>> services_to_activate;
    for (auto const& name : config.groups()) {
        auto service = TRY(Service::try_create(config, name));
        if (service->is_enabled_for_system_mode(g_system_mode)) {
            g_services.append(move(service));
            services_to_activate.append(g_services.last());
        }
    }

    auto order = TRY(service_startup_order(config, services_to_activate));

    // Create service sockets only after validating the dependency graph. A socket
    // setup failure is recorded so that dependents will not be launched.
    Vector<ByteString> failed_services;
    for (auto index : order) {
        auto& service = services_to_activate[index];
        if (auto result = service->setup_sockets(); result.is_error()) {
            warnln("{}: Failed to set up service sockets: {}", service->name(), result.release_error());
            failed_services.append(ByteString::copy(service->name()));
        }
    }

    dbgln("Activating {} services in dependency order...", order.size());
    for (auto index : order) {
        auto& service = services_to_activate[index];
        bool dependency_failed = false;
        for (auto const& dependency : config.read_entry(ByteString::copy(service->name()), "Dependencies").split(',')) {
            if (!dependency.is_empty() && failed_services.contains_slow(dependency)) {
                warnln("{}: Not starting because dependency {} failed", service->name(), dependency);
                dependency_failed = true;
                break;
            }
        }
        if (dependency_failed)
            continue;

        // A service whose socket setup failed is itself marked failed above.
        if (failed_services.contains_slow(ByteString::copy(service->name())))
            continue;

        dbgln_if(SYSTEMSERVER_DEBUG, "Activating {}", service->name());
        if (auto result = service->activate(); result.is_error()) {
            warnln("{}: Failed to activate service: {}", service->name(), result.release_error());
            failed_services.append(ByteString::copy(service->name()));
        }
    }
    return {};
}

static ErrorOr<void> activate_base_services_based_on_system_mode()
{
    if (g_system_mode == graphical_system_mode) {
        bool done_searching_for_gpu = false;

        auto timeout = Core::Timer::create_single_shot(10000, [&]() {
            dbgln("WARNING: No device nodes at /dev/gpu/ directory after 10 seconds. This is probably a sign of disabled graphics functionality.");
            dbgln("To cope with this, graphical mode will not be enabled.");
            g_system_mode = text_system_mode;

            done_searching_for_gpu = true;
        });

        auto log = Core::Timer::create_single_shot(1000, [&]() {
            dbgln("Waiting on /dev/gpu/connector0 to appear...");
        });

        auto watcher = TRY(Core::FileWatcher::create());
        watcher->on_change = [&](Core::FileWatcherEvent const& event) {
            if (event.event_path != "/dev/gpu/connector0"sv)
                return;
            done_searching_for_gpu = true;
        };

        TRY(watcher->add_watch("/dev/gpu/", Core::FileWatcherEvent::Type::ChildCreated));

        // The GPU might have appeared while we were setting up the watcher.
        // Only wait for the file if we can't stat it.
        if (Core::System::lstat("/dev/gpu/connector0"sv).is_error()) {
            log->start();
            timeout->start();
            Core::EventLoop::current().spin_until([&]() { return done_searching_for_gpu; });
        }
    }

    // Read our config and instantiate services.
    // This takes care of setting up sockets.
    auto config = TRY(Core::ConfigFile::open_for_system("SystemServer"));
    TRY(activate_services(*config));
    return {};
}

static ErrorOr<void> activate_user_services_based_on_system_mode()
{
    // Read system wide user config and instantiate services.
    auto config = TRY(Core::ConfigFile::open_for_system("SystemServerUser"));
    TRY(activate_services(*config));

    // Try to read user config.
    if (auto config = Core::ConfigFile::open_for_app("SystemServer"); !config.is_error()) {
        TRY(activate_services(config.release_value()));
    }
    return {};
}

};

ErrorOr<int> serenity_main(Main::Arguments arguments)
{
    bool user = false;
    Core::ArgsParser args_parser;
    args_parser.add_option(user, "Run in user-mode", "user", 'u');
    args_parser.parse(arguments);

    TRY(Core::System::pledge("stdio proc exec tty accept unix rpath wpath cpath chown fattr id sigaction"));

    if (!user)
        TRY(SystemServer::determine_system_mode());

    Core::EventLoop event_loop;

    event_loop.register_signal(SIGCHLD, sigchld_handler);
    event_loop.register_signal(SIGTERM, sigterm_handler);

    if (!user) {
        TRY(SystemServer::activate_base_services_based_on_system_mode());
    } else {
        TRY(SystemServer::activate_user_services_based_on_system_mode());
    }

    return event_loop.exec();
}
