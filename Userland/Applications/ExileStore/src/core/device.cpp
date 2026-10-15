#include "core/device.h"

#include <algorithm>

#include "core/version.h"

namespace exile {

bool DeviceProfile::channelEnabled(ReleaseChannel c) const {
    return std::find(enabledChannels.begin(), enabledChannels.end(), c) != enabledChannels.end();
}

std::string DeviceProfile::describe() const {
    return displayName(deviceClass) + ", " + std::string(toString(arch)) + ", Exile OS " + osVersion;
}

DeviceProfile detectHostDevice() {
    DeviceProfile d;
#if defined(__x86_64__) || defined(_M_X64)
    d.arch = Arch::X86_64;
#elif defined(__aarch64__) || defined(_M_ARM64)
    d.arch = Arch::Aarch64;
#elif defined(__riscv) && __riscv_xlen == 64
    d.arch = Arch::Riscv64;
#else
    d.arch = Arch::Any;
#endif
    d.deviceClass = DeviceClass::Desktop;
    d.osVersion = "0.1.0";
    return d;
}

std::string effectiveVersion(const AppEntry& app, const PackageBuild& build) {
    return build.version.empty() ? app.currentVersion : build.version;
}

namespace {

bool contains(const std::vector<DeviceClass>& v, DeviceClass d) { return std::find(v.begin(), v.end(), d) != v.end(); }

bool archMatches(const PackageBuild& b, Arch deviceArch) {
    for (Arch a : b.architectures)
        if (a == Arch::Any || a == deviceArch) return true;
    return false;
}

bool archExact(const PackageBuild& b, Arch deviceArch) {
    return std::find(b.architectures.begin(), b.architectures.end(), deviceArch) != b.architectures.end();
}

// Order in which a build is checked; the build that gets furthest decides which
// explanation the user sees.
CompatStatus statusFor(const PackageBuild& b, const DeviceProfile& dev) {
    if (!contains(b.devices, dev.deviceClass)) return CompatStatus::WrongDevice;
    if (!archMatches(b, dev.arch)) return CompatStatus::WrongArchitecture;
    if (!b.minimumOsVersion.empty() && compareVersionStrings(dev.osVersion, b.minimumOsVersion) < 0) return CompatStatus::OsTooOld;
    if (!dev.channelEnabled(b.channel)) return CompatStatus::ChannelNotEnabled;
    if (b.withdrawn) return CompatStatus::Withdrawn;
    return CompatStatus::Compatible;
}

int progress(CompatStatus s) {
    switch (s) {
        case CompatStatus::WrongDevice: return 0;
        case CompatStatus::WrongArchitecture: return 1;
        case CompatStatus::OsTooOld: return 2;
        case CompatStatus::ChannelNotEnabled: return 3;
        case CompatStatus::Withdrawn: return 4;
        default: return 5;
    }
}

int rank(const PackageBuild& b, const DeviceProfile& dev) {
    int kind = b.kind == PackageKind::Native ? 0 : (b.kind == PackageKind::CompatibilityLayer ? 1 : 2);
    return kind * 10 + (archExact(b, dev.arch) ? 0 : 1);
}

std::string join(const std::vector<std::string>& v) {
    std::string out;
    for (size_t i = 0; i < v.size(); ++i) out += (i ? ", " : "") + v[i];
    return out;
}

}  // namespace

Compatibility resolveCompatibility(const AppEntry& app, const DeviceProfile& dev) {
    Compatibility result;
    if (app.packages.empty()) {
        result.status = CompatStatus::NoPackages;
        result.reason = "This app has no downloadable builds.";
        return result;
    }

    const PackageBuild* best = nullptr;
    const PackageBuild* furthest = nullptr;
    CompatStatus furthestStatus = CompatStatus::WrongDevice;
    for (const PackageBuild& b : app.packages) {
        CompatStatus s = statusFor(b, dev);
        if (s == CompatStatus::Compatible) {
            if (!best || rank(b, dev) < rank(*best, dev)) best = &b;
        } else if (!furthest || progress(s) > progress(furthestStatus)) {
            furthest = &b;
            furthestStatus = s;
        }
    }

    if (best) {
        result.status = CompatStatus::Compatible;
        result.build = best;
        result.version = effectiveVersion(app, *best);
        return result;
    }

    result.status = furthestStatus;
    switch (furthestStatus) {
        case CompatStatus::WrongDevice: {
            std::vector<std::string> names;
            for (DeviceClass d : app.supportedDevices()) names.push_back(displayName(d));
            result.reason = "Not available for this device. It is built for " + join(names) + "; this is a " +
                            displayName(dev.deviceClass) + " device.";
            break;
        }
        case CompatStatus::WrongArchitecture: {
            std::vector<std::string> names;
            for (Arch a : app.supportedArchitectures()) names.push_back(std::string(toString(a)));
            result.reason = "Not available for this device's processor (" + std::string(toString(dev.arch)) +
                            "). Builds exist for: " + join(names) + ".";
            break;
        }
        case CompatStatus::OsTooOld:
            result.reason = "Needs Exile OS " + furthest->minimumOsVersion + " or newer; this device has " + dev.osVersion + ".";
            break;
        case CompatStatus::ChannelNotEnabled:
            result.reason = "Only available in the " + std::string(toString(furthest->channel)) + " channel, which is not turned on for this device.";
            break;
        case CompatStatus::Withdrawn:
            result.reason = "This release was withdrawn by the publisher" +
                            (furthest->withdrawnReason.empty() ? std::string(".") : ": " + furthest->withdrawnReason);
            break;
        default:
            result.reason = "Not available for this device.";
    }
    return result;
}

bool isListedFor(const AppEntry& app, const DeviceProfile& device) {
    for (const auto& b : app.packages)
        if (device.channelEnabled(b.channel)) return true;
    return false;
}

}  // namespace exile
