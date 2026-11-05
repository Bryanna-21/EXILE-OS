/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#pragma once

// Derives what the store should show for an app (the single primary action) and
// which installed apps have compatible updates.

#include <string>
#include <vector>

#include "device.h"
#include "library.h"
#include "model.h"

namespace exile {

enum class PrimaryAction {
    Install,     // compatible, not installed
    Open,        // installed, current, and the backend can launch it
    Installed,   // installed, current, but this system cannot launch it (shown as a state, not a button)
    Update,      // installed and a newer compatible version exists
    Unavailable, // not installable on this device
};

struct AppStatus {
    PrimaryAction action = PrimaryAction::Unavailable;
    std::string label; // text of the primary action
    std::string note;  // one-line explanation, may be empty
    Compatibility compat;
    InstalledApp const* installed = nullptr;
};

AppStatus computeStatus(AppEntry const& app, Library const& library, DeviceProfile const& device, bool backendCanLaunch);

struct UpdateInfo {
    AppEntry const* app = nullptr;
    std::string installedVersion;
    std::string newVersion;
    std::string releaseNotes;
};

std::vector<UpdateInfo> findUpdates(Catalog const& catalog, Library const& library, DeviceProfile const& device);

} // namespace exile
