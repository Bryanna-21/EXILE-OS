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

// Resolve a stable dependency-first order. Unknown dependencies block their
// dependents, while cycles and duplicate service identifiers are configuration
// errors and must be rejected before SystemServer creates sockets or processes.
static ErrorOr<Vector<size_t>> resolve_service_startup_order(Vector<ByteString> const& services, HashMap<ByteString, Vector<ByteString>> const& dependencies)
{
    for (size_t i = 0; i < services.size(); ++i) {
        for (size_t j = i + 1; j < services.size(); ++j) {
            if (services[i] == services[j])
                return Error::from_string_literal("Duplicate service name in startup graph");
        }
    }

    Vector<u8> blocked;
    Vector<u8> scheduled;
    TRY(blocked.try_resize(services.size()));
    TRY(scheduled.try_resize(services.size()));
    for (size_t i = 0; i < services.size(); ++i) {
        blocked[i] = 0;
        scheduled[i] = 0;
    }

    // Mark references to services that are not active in this configuration.
    for (size_t i = 0; i < services.size(); ++i) {
        auto dependency_it = dependencies.find(services[i]);
        if (dependency_it == dependencies.end())
            continue;
        for (auto const& dependency : dependency_it->value) {
            if (dependency.is_empty())
                continue;
            if (!find_service_index(services, dependency))
                blocked[i] = 1;
        }
    }

    // A service depending on a blocked service is itself blocked. Propagate until
    // the set is stable so that no dependent gets into the startup order.
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
            bool dependencies_scheduled = true;
            if (dependency_it != dependencies.end()) {
                for (auto const& dependency : dependency_it->value) {
                    if (dependency.is_empty())
                        continue;
                    auto dependency_index = find_service_index(services, dependency);
                    if (dependency_index.has_value() && !scheduled[dependency_index.value()]) {
                        dependencies_scheduled = false;
                        break;
                    }
                }
            }
            if (!dependencies_scheduled)
                continue;
            TRY(order.try_append(i));
            scheduled[i] = 1;
            made_progress = true;
        }
    }

    // Any remaining unblocked node is part of or depends on a cycle. Do not
    // silently classify a self-dependency as an ordinary unavailable service.
    for (size_t i = 0; i < services.size(); ++i) {
        if (!blocked[i] && !scheduled[i])
            return Error::from_string_literal("Circular service dependency detected");
    }
    return order;
}

}
