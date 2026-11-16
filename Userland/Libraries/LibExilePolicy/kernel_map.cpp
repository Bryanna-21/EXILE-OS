/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include "kernel_map.h"

#include <algorithm>
#include <map>
#include <set>

namespace exile::policy {

std::string KernelProjection::promiseString() const
{
    std::string s;
    for (auto const& p : promises)
        s += (s.empty() ? "" : " ") + p;
    return s;
}

KernelProjection projectGrants(Broker const& broker, std::string const& principal)
{
    KernelProjection k;
    std::set<std::string> promises { "stdio" };
    std::map<std::string, std::string> veil; // path -> perms
    auto addVeil = [&](std::string const& path, std::string const& perms) {
        auto& cur = veil[path];
        for (char c : perms)
            if (cur.find(c) == std::string::npos)
                cur += c;
        std::sort(cur.begin(), cur.end());
    };
    for (auto const& g : broker.activeGrants(principal)) {
        if (g.lifetime == Lifetime::Once)
            continue; // per-use grants are never ambient authority
        auto const& key = g.permission.key;
        if (key == "fs.read" || key == "fs.write" || key == "proc.spawn") {
            std::string path = broker.resolveRoot(principal, g.permission.scope);
            if (path.empty() || path[0] != '/') {
                k.skipped.push_back(g.permission.canonical() + " (symbolic root not resolved)");
                continue;
            }
            if (key == "fs.read") {
                promises.insert("rpath");
                addVeil(path, "r");
            } else if (key == "fs.write") {
                promises.insert({ "rpath", "wpath", "cpath" });
                addVeil(path, "rwc");
            } else {
                promises.insert({ "proc", "exec" });
                addVeil(path, "x");
            }
        } else if (key == "net.connect" || key == "ai.cloud") {
            promises.insert({ "inet", "unix" }); // unix: name lookup goes through a local socket
            k.brokerMediated.push_back(g.permission.canonical() + " (kernel cannot restrict hosts or ports)");
        } else {
            k.brokerMediated.push_back(g.permission.canonical());
        }
    }
    if (veil.empty())
        veil["/"] = ""; // no filesystem grants: unveil nothing, then lock
    k.promises.assign(promises.begin(), promises.end());
    for (auto const& [p, perms] : veil)
        k.unveil.push_back({ p, perms });
    return k;
}

KernelProjection projectAndMark(Broker& broker, std::string const& principal)
{
    auto k = projectGrants(broker, principal);
    broker.markProjected(principal);
    return k;
}

} // namespace exile::policy
