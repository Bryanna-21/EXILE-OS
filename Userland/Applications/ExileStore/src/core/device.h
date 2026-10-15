#pragma once
// Device capability description and the compatibility resolver.

#include <string>
#include <vector>

#include "core/model.h"

namespace exile {

struct DeviceProfile {
    DeviceClass deviceClass = DeviceClass::Desktop;
    Arch arch = Arch::X86_64;
    std::string osVersion = "0.1.0";
    // Release channels this device has opted into. Stable only by default.
    std::vector<ReleaseChannel> enabledChannels = {ReleaseChannel::Stable};

    bool channelEnabled(ReleaseChannel c) const;
    std::string describe() const;  // "Desktop, x86_64, Exile OS 0.1.0"
};

// Architecture comes from the compiler; the device class and OS version are
// defaults the caller is expected to override (a real Exile build would query
// the system). The host is never assumed to be a phone or TV.
DeviceProfile detectHostDevice();

enum class CompatStatus {
    Compatible,
    WrongDevice,
    WrongArchitecture,
    OsTooOld,
    ChannelNotEnabled,
    Withdrawn,
    NoPackages,
};

struct Compatibility {
    CompatStatus status = CompatStatus::NoPackages;
    const PackageBuild* build = nullptr;  // set only when Compatible
    std::string version;                  // effective version of `build`
    std::string reason;                   // user-facing explanation when not compatible
    bool ok() const { return status == CompatStatus::Compatible; }
};

// Version of a build: its own override or the app's currentVersion.
std::string effectiveVersion(const AppEntry& app, const PackageBuild& build);

// Picks the best compatible build (native > compatibility layer > web, exact
// architecture before "any"), or explains why none fits.
Compatibility resolveCompatibility(const AppEntry& app, const DeviceProfile& device);

// An app is listed unless every one of its builds belongs to a channel the
// device has not enabled (so beta-only apps do not clutter normal browsing).
bool isListedFor(const AppEntry& app, const DeviceProfile& device);

}  // namespace exile
