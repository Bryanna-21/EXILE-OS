/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#pragma once

// Projects broker grants onto the mechanisms the existing Kernel/ really has: pledge promises and
// unveil paths (Kernel/Syscalls/pledge.cpp, unveil.cpp). This yields a *specification* to apply in a
// launcher before exec; nothing here calls the kernel. What the kernel cannot express (host-level
// network scoping, device and settings access) is listed in `brokerMediated` and must be enforced
// by the owning service through the broker, not assumed to be enforced by pledge.

#include <string>
#include <vector>

#include "broker.h"

namespace exile::policy {

struct UnveilEntry {
    std::string path;
    std::string perms; // subset of "rwxcb"
};

struct KernelProjection {
    std::vector<std::string> promises; // sorted, unique, always includes "stdio"
    std::vector<UnveilEntry> unveil;   // always non-empty so the veil is dropped, then lock
    std::vector<std::string> brokerMediated;
    std::vector<std::string> skipped; // grants that could not be projected (fail closed: less access)
    std::string promiseString() const;
};

// Pure function of the grants (symbols resolved through the broker). Does not mark anything projected.
KernelProjection projectGrants(Broker const& broker, std::string const& principal);
// Same, and records on the broker that these grants became ambient kernel authority, so a later
// revocation reports needsProcessRestart (pledge/unveil can only be tightened, never loosened).
KernelProjection projectAndMark(Broker& broker, std::string const& principal);

} // namespace exile::policy
