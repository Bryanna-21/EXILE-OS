#pragma once
// Package transport. Only local transport is implemented in this build; HTTPS
// is an explicit, honest gap (see docs/INTEGRATION.md) rather than a stub that
// pretends to download.

#include <atomic>
#include <cstdint>
#include <filesystem>
#include <functional>
#include <optional>
#include <string>

namespace exile {

class CancelToken {
public:
    void cancel() { flag_.store(true); }
    bool cancelled() const { return flag_.load(); }
    void reset() { flag_.store(false); }

private:
    std::atomic<bool> flag_{false};
};

struct TransferResult {
    bool ok = false;
    bool cancelled = false;
    std::string error;
    uint64_t bytes = 0;  // total size of the destination after the transfer
};

using ProgressFn = std::function<void(uint64_t done, uint64_t total)>;

class ITransport {
public:
    virtual ~ITransport() = default;
    // Fetches `source` into `partFile`. With resume=true an existing partial
    // file is continued instead of restarted. On cancellation the partial file
    // is kept so the download can be resumed later.
    virtual TransferResult fetch(const std::string& source, const std::filesystem::path& partFile, bool resume,
                                 const ProgressFn& progress, const CancelToken& cancel) = 0;
};

// Resolves a catalog source to a local path under `baseDir`, or explains why not.
// Relative sources must stay inside baseDir (checked after symlink resolution).
// "file://" absolute URLs are refused unless allowFileUrls is set.
std::optional<std::filesystem::path> resolveLocalSource(const std::string& source, const std::filesystem::path& baseDir,
                                                        bool allowFileUrls, std::string& error);

class LocalTransport final : public ITransport {
public:
    LocalTransport(std::filesystem::path baseDir, bool allowFileUrls) : baseDir_(std::move(baseDir)), allowFileUrls_(allowFileUrls) {}
    TransferResult fetch(const std::string& source, const std::filesystem::path& partFile, bool resume,
                         const ProgressFn& progress, const CancelToken& cancel) override;

private:
    std::filesystem::path baseDir_;
    bool allowFileUrls_;
};

// Refuses https:// sources with a clear message until a TLS-capable transport is plugged in.
class UnavailableHttpsTransport final : public ITransport {
public:
    TransferResult fetch(const std::string& source, const std::filesystem::path&, bool, const ProgressFn&,
                         const CancelToken&) override;
};

// Routes https:// to the remote transport and everything else to the local one.
class RoutingTransport final : public ITransport {
public:
    RoutingTransport(ITransport& local, ITransport& remote) : local_(local), remote_(remote) {}
    TransferResult fetch(const std::string& source, const std::filesystem::path& partFile, bool resume,
                         const ProgressFn& progress, const CancelToken& cancel) override;

private:
    ITransport& local_;
    ITransport& remote_;
};

}  // namespace exile
