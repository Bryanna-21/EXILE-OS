#pragma once
// Derives what the store should show for an app (the single primary action) and
// which installed apps have compatible updates.

#include <string>
#include <vector>

#include "core/device.h"
#include "core/library.h"
#include "core/model.h"

namespace exile {

enum class PrimaryAction {
    Install,      // compatible, not installed
    Open,         // installed, current, and the backend can launch it
    Installed,    // installed, current, but this system cannot launch it (shown as a state, not a button)
    Update,       // installed and a newer compatible version exists
    Unavailable,  // not installable on this device
};

struct AppStatus {
    PrimaryAction action = PrimaryAction::Unavailable;
    std::string label;  // text of the primary action
    std::string note;   // one-line explanation, may be empty
    Compatibility compat;
    const InstalledApp* installed = nullptr;
};

AppStatus computeStatus(const AppEntry& app, const Library& library, const DeviceProfile& device, bool backendCanLaunch);

struct UpdateInfo {
    const AppEntry* app = nullptr;
    std::string installedVersion;
    std::string newVersion;
    std::string releaseNotes;
};

std::vector<UpdateInfo> findUpdates(const Catalog& catalog, const Library& library, const DeviceProfile& device);

}  // namespace exile
