/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include "appstate.h"

#include "version.h"

namespace exile {

AppStatus computeStatus(AppEntry const& app, Library const& library, DeviceProfile const& device, bool canLaunch)
{
    AppStatus s;
    s.compat = resolveCompatibility(app, device);
    s.installed = library.find(app.appId);

    if (s.installed) {
        if (s.compat.ok() && compareVersionStrings(s.compat.version, s.installed->version) > 0) {
            s.action = PrimaryAction::Update;
            s.label = "Update";
            s.note = "Version " + s.compat.version + " is available (you have " + s.installed->version + ").";
        } else if (canLaunch) {
            s.action = PrimaryAction::Open;
            s.label = "Open";
        } else {
            s.action = PrimaryAction::Installed;
            s.label = "Installed";
            s.note = "Version " + s.installed->version + " is installed.";
        }
        if (!s.compat.ok())
            s.note = "Installed. " + s.compat.reason;
        return s;
    }

    if (s.compat.ok()) {
        s.action = PrimaryAction::Install;
        s.label = "Install";
    } else {
        s.action = PrimaryAction::Unavailable;
        s.label = "Not available for this device";
        s.note = s.compat.reason;
    }
    return s;
}

std::vector<UpdateInfo> findUpdates(Catalog const& catalog, Library const& library, DeviceProfile const& device)
{
    std::vector<UpdateInfo> out;
    for (auto const& inst : library.installed()) {
        AppEntry const* app = catalog.find(inst.appId);
        if (!app)
            continue;
        Compatibility c = resolveCompatibility(*app, device);
        if (!c.ok())
            continue;
        if (compareVersionStrings(c.version, inst.version) > 0)
            out.push_back({ app, inst.version, c.version, app->releaseNotes });
    }
    return out;
}

} // namespace exile
