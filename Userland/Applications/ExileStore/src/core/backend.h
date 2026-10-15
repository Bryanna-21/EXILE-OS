#pragma once
// Package-manager adapter. The store never installs anything itself; it hands a
// verified package to an IPackageBackend and trusts only the backend's own
// confirmation (isInstalled) before reporting success.
//
// The only backend implemented here is LocalPrefixBackend, a development
// backend that unpacks .expkg files into a user-owned prefix directory with
// atomic replace and rollback. It runs unprivileged and is NOT the Exile OS
// system installer: that service does not exist in this repository, so it is
// documented as an integration point (docs/INTEGRATION.md) instead of faked.

#include <filesystem>
#include <string>

namespace exile {

struct InstallRequest {
    std::string appId;
    std::string version;
    std::string packageId;
    std::filesystem::path packageFile;  // already downloaded, digest- and signature-verified
};

struct BackendResult {
    bool ok = false;
    std::string error;
};

class IPackageBackend {
public:
    virtual ~IPackageBackend() = default;
    virtual std::string name() const = 0;
    // True only if launch() can really start the app on this system.
    virtual bool canLaunch() const = 0;
    // Where installed payloads go (used for free-space checks).
    virtual std::filesystem::path storagePath() const = 0;
    virtual BackendResult install(const InstallRequest& request) = 0;
    virtual BackendResult uninstall(const std::string& appId) = 0;
    // Ground truth from the backend, independent of the store's own records.
    virtual bool isInstalled(const std::string& appId, const std::string& version) const = 0;
    virtual BackendResult launch(const std::string& appId) = 0;
    // Cleans up after a crash/interruption (half-finished staging, orphaned backups).
    virtual void recoverInterrupted() = 0;
};

class LocalPrefixBackend final : public IPackageBackend {
public:
    explicit LocalPrefixBackend(std::filesystem::path prefix);

    std::string name() const override { return "Local development prefix"; }
    bool canLaunch() const override { return false; }
    std::filesystem::path storagePath() const override { return prefix_; }
    BackendResult install(const InstallRequest& request) override;
    BackendResult uninstall(const std::string& appId) override;
    bool isInstalled(const std::string& appId, const std::string& version) const override;
    BackendResult launch(const std::string& appId) override;
    void recoverInterrupted() override;

    std::filesystem::path installDir(const std::string& appId) const;  // <prefix>/apps/<id>/current

private:
    std::filesystem::path prefix_;
};

}  // namespace exile
