#pragma once

#include <AK/ByteString.h>
#include <AK/HashMap.h>
#include <AK/Optional.h>
#include <AK/Vector.h>

namespace SystemServer {

static Optional<size_t> find_service_index(Vector<ByteString> const& services, StringView name)
{
    for (size_t i = 0; i < services.size(); ++i) {
        if (services[i].view() == name)
            return i;
    }
    return {};
}

static ErrorOr<Vector<size_t>> resolve_service_startup_order(Vector<ByteString> const& services, HashMap<ByteString, Vector<ByteString>> const& dependencies)
{
    Vector<u8> blocked;
    Vector<u8> scheduled;
    TRY(blocked.try_resize(services.size()));
    TRY(scheduled.try_resize(services.size()));
    for (size_t i = 0; i < services.size(); ++i) {
        blocked[i] = 0;
        scheduled[i] = 0;
    }

    for (size_t i = 0; i < services.size(); ++i) {
        auto const& service_name = services[i];
        auto dependency_it = dependencies.find(service_name);
        if (dependency_it == dependencies.end())
            continue;

        for (auto const& dependency : dependency_it->value) {
            if (dependency.is_empty())
                continue;
            auto dependency_index = find_service_index(services, dependency);
            if (!dependency_index.has_value()) {
                blocked[i] = 1;
                continue;
            }
            if (dependency_index.value() == i) {
                blocked[i] = 1;
                continue;
            }
        }
    }

    bool changed = true;
    while (changed) {
        changed = false;
        for (size_t i = 0; i < services.size(); ++i) {
            if (blocked[i])
                continue;
            auto dependency_it = dependencies.find(services[i]);
            if (dependency_it == dependencies.end())
                continue;
            for (auto const& dependency : dependency_it->value) {
                if (dependency.is_empty())
                    continue;
                auto dependency_index = find_service_index(services, dependency);
                if (!dependency_index.has_value() || blocked[dependency_index.value()]) {
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
            auto dependency_it = dependencies.find(services[i]);
            if (dependency_it == dependencies.end()) {
                TRY(order.try_append(i));
                scheduled[i] = 1;
                made_progress = true;
                continue;
            }

            bool dependencies_scheduled = true;
            for (auto const& dependency : dependency_it->value) {
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

}
