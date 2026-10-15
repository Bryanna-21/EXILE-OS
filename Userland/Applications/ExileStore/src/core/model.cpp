#include "core/model.h"

#include <algorithm>

#include "core/version.h"

namespace exile {

std::string_view toString(DeviceClass v) {
    switch (v) {
        case DeviceClass::Desktop: return "desktop";
        case DeviceClass::Mobile: return "mobile";
        case DeviceClass::Tablet: return "tablet";
        case DeviceClass::Tv: return "tv";
        case DeviceClass::Other: return "other";
    }
    return "other";
}

std::string_view toString(Arch v) {
    switch (v) {
        case Arch::X86_64: return "x86_64";
        case Arch::Aarch64: return "aarch64";
        case Arch::Riscv64: return "riscv64";
        case Arch::Any: return "any";
    }
    return "any";
}

std::string_view toString(PackageKind v) {
    switch (v) {
        case PackageKind::Native: return "native";
        case PackageKind::CompatibilityLayer: return "compat-layer";
        case PackageKind::Web: return "web";
    }
    return "native";
}

std::string_view toString(ReleaseChannel v) {
    switch (v) {
        case ReleaseChannel::Stable: return "stable";
        case ReleaseChannel::Beta: return "beta";
        case ReleaseChannel::Nightly: return "nightly";
    }
    return "stable";
}

std::string displayName(DeviceClass v) {
    switch (v) {
        case DeviceClass::Desktop: return "Desktop";
        case DeviceClass::Mobile: return "Mobile";
        case DeviceClass::Tablet: return "Tablet";
        case DeviceClass::Tv: return "TV";
        case DeviceClass::Other: return "Other";
    }
    return "Other";
}

std::string displayName(PackageKind v) {
    switch (v) {
        case PackageKind::Native: return "Native app";
        case PackageKind::CompatibilityLayer: return "Runs through a compatibility layer";
        case PackageKind::Web: return "Web app";
    }
    return "Native app";
}

std::optional<DeviceClass> parseDeviceClass(std::string_view s) {
    for (DeviceClass d : {DeviceClass::Desktop, DeviceClass::Mobile, DeviceClass::Tablet, DeviceClass::Tv, DeviceClass::Other})
        if (toString(d) == s) return d;
    return std::nullopt;
}
std::optional<Arch> parseArch(std::string_view s) {
    for (Arch a : {Arch::X86_64, Arch::Aarch64, Arch::Riscv64, Arch::Any})
        if (toString(a) == s) return a;
    return std::nullopt;
}
std::optional<PackageKind> parsePackageKind(std::string_view s) {
    for (PackageKind k : {PackageKind::Native, PackageKind::CompatibilityLayer, PackageKind::Web})
        if (toString(k) == s) return k;
    return std::nullopt;
}
std::optional<ReleaseChannel> parseChannel(std::string_view s) {
    for (ReleaseChannel c : {ReleaseChannel::Stable, ReleaseChannel::Beta, ReleaseChannel::Nightly})
        if (toString(c) == s) return c;
    return std::nullopt;
}

std::vector<DeviceClass> AppEntry::supportedDevices() const {
    std::vector<DeviceClass> out;
    for (const auto& p : packages)
        for (DeviceClass d : p.devices)
            if (std::find(out.begin(), out.end(), d) == out.end()) out.push_back(d);
    std::sort(out.begin(), out.end());
    return out;
}

std::vector<Arch> AppEntry::supportedArchitectures() const {
    std::vector<Arch> out;
    for (const auto& p : packages)
        for (Arch a : p.architectures)
            if (std::find(out.begin(), out.end(), a) == out.end()) out.push_back(a);
    std::sort(out.begin(), out.end());
    return out;
}

std::string AppEntry::minimumOsVersion() const {
    std::string best;
    for (const auto& p : packages) {
        if (p.minimumOsVersion.empty()) return {};  // some build has no minimum
        if (best.empty() || compareVersionStrings(p.minimumOsVersion, best) < 0) best = p.minimumOsVersion;
    }
    return best;
}

const AppEntry* Catalog::find(std::string_view appId) const {
    for (const auto& a : apps)
        if (a.appId == appId) return &a;
    return nullptr;
}

const Category* Catalog::findCategory(std::string_view id) const {
    for (const auto& c : categories)
        if (c.id == id) return &c;
    return nullptr;
}

}  // namespace exile
