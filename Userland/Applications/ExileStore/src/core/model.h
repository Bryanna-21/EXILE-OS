/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#pragma once

// Typed catalog model. Application identity (AppEntry) is separate from the
// device-specific builds (PackageBuild), so one app can ship several builds.

#include <cstdint>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace exile {

// "Any" is only valid in a build's device list (a universal build, e.g. a web app that
// adapts to any screen). A DeviceProfile is always one concrete class.
enum class DeviceClass { Desktop,
    Mobile,
    Tablet,
    Tv,
    Other,
    Any };
enum class Arch { X86_64,
    Aarch64,
    Riscv64,
    Any };
enum class PackageKind { Native,
    CompatibilityLayer,
    Web };
enum class ReleaseChannel { Stable,
    Beta,
    Nightly };

std::string_view toString(DeviceClass v); // wire name, e.g. "desktop"
std::string_view toString(Arch v);        // "x86_64", "aarch64", "riscv64", "any"
std::string_view toString(PackageKind v); // "native", "compat-layer", "web"
std::string_view toString(ReleaseChannel v);

std::string displayName(DeviceClass v); // "Desktop", "Mobile", ...
std::string displayName(PackageKind v);

std::optional<DeviceClass> parseDeviceClass(std::string_view s);
std::optional<Arch> parseArch(std::string_view s);
std::optional<PackageKind> parsePackageKind(std::string_view s);
std::optional<ReleaseChannel> parseChannel(std::string_view s);

struct Publisher {
    std::string id;
    std::string name;
    std::string website;
};

struct Permission {
    std::string id;     // e.g. "network", "camera"
    std::string reason; // the publisher's own explanation (shown alongside ours)
};

struct Dependency {
    std::string appId;
    std::string minVersion; // empty => any version
};

// Metadata only. It is NOT trusted: public keys live in the separate trust store.
struct SignatureMetadata {
    std::string keyId;
    std::string algorithm;
    std::string signatureHex;
};

struct PackageBuild {
    std::string id; // unique inside the app, e.g. "desktop-x86_64"
    PackageKind kind = PackageKind::Native;
    std::vector<DeviceClass> devices;
    std::vector<Arch> architectures;
    std::string minimumOsVersion; // empty => no minimum
    std::string version;          // empty => the app's currentVersion
    std::string url;              // https://, file:// or a safe relative path
    std::string sha256;           // lowercase hex digest of the package file
    uint64_t downloadSize = 0;    // bytes, 0 => unknown
    uint64_t packageSize = 0;     // installed size in bytes, 0 => unknown
    SignatureMetadata signature;
    ReleaseChannel channel = ReleaseChannel::Stable;
    std::vector<Dependency> dependencies;
    bool withdrawn = false;
    std::string withdrawnReason;
};

struct Rating {
    double average = 0;
    uint64_t count = 0;
};

struct AppEntry {
    std::string appId;
    std::string name;
    std::string summary;
    std::string description;
    Publisher publisher;
    std::string icon;
    std::vector<std::string> screenshots;
    std::vector<std::string> categories;
    std::vector<std::string> keywords;
    std::string currentVersion;
    std::string releaseNotes;
    std::vector<Permission> permissions;
    std::string license;
    std::string privacyPolicy;
    std::string sourceRepository;
    std::string publishedAt;
    std::string updatedAt;
    bool featured = false;
    std::optional<Rating> rating; // only present when real data exists
    std::vector<PackageBuild> packages;

    // Derived views over `packages`.
    std::vector<DeviceClass> supportedDevices() const;
    std::vector<Arch> supportedArchitectures() const;
    std::string minimumOsVersion() const; // lowest minimum across builds, empty if none
};

struct Category {
    std::string id;
    std::string name;
};

struct Catalog {
    int schemaVersion = 1;
    std::string generatedAt;
    std::string baseDir; // directory used to resolve relative package paths
    std::vector<Category> categories;
    std::vector<AppEntry> apps;

    AppEntry const* find(std::string_view appId) const;
    Category const* findCategory(std::string_view id) const;
};

} // namespace exile
