#pragma once
// Install / update / uninstall orchestration.
//
// Pipeline (each stage is an explicit, validated state transition):
//   compatibility -> dependencies -> disk space -> download (resumable)
//   -> integrity (size + SHA-256) -> signature/trust -> backend install
//   -> backend confirmation -> library record.
// "Succeeded" is reached only after the backend itself confirms the install.

#include <filesystem>
#include <functional>
#include <future>
#include <mutex>
#include <optional>
#include <string>
#include <vector>

#include "core/backend.h"
#include "core/device.h"
#include "core/library.h"
#include "core/model.h"
#include "core/transport.h"
#include "core/trust.h"

namespace exile {

enum class JobState {
    Queued,
    CheckingCompatibility,
    CheckingDependencies,
    CheckingSpace,
    Downloading,
    Verifying,
    Installing,
    Succeeded,
    Failed,
    Cancelled,
};

enum class JobKind { Install, Update };

const char* toString(JobState s);
bool isTerminal(JobState s);
bool isValidTransition(JobState from, JobState to);

struct InstallerEnv {
    const Catalog* catalog = nullptr;
    const DeviceProfile* device = nullptr;
    Library* library = nullptr;
    IPackageBackend* backend = nullptr;
    ITransport* transport = nullptr;
    const TrustStore* trust = nullptr;
    const ISignatureVerifier* verifier = nullptr;
    std::filesystem::path cacheDir;
    // Free bytes at a path; defaults to std::filesystem::space. Injectable for tests.
    std::function<std::optional<uint64_t>(const std::filesystem::path&)> freeSpace;
};

struct JobProgress {
    JobState state = JobState::Queued;
    uint64_t done = 0;
    uint64_t total = 0;
    std::string message;
};

struct JobResult {
    JobState finalState = JobState::Failed;
    std::string error;                 // plain-language reason when not Succeeded
    std::string detail;                // optional extra information (e.g. warnings)
    std::vector<JobState> trail;       // every state visited, in order
    bool ok() const { return finalState == JobState::Succeeded; }
};

class InstallJob {
public:
    InstallJob(InstallerEnv env, std::string appId, JobKind kind);

    JobResult run();                         // blocking
    std::future<JobResult> runAsync();       // runs on a worker thread
    JobProgress progress() const;            // thread-safe snapshot
    CancelToken& cancelToken() { return cancel_; }

private:
    bool transition(JobState to, std::string message = {});
    JobResult finish(JobState terminal, std::string error, std::string detail = {});
    void setProgress(uint64_t done, uint64_t total);
    void record(const std::string& outcome, const std::string& version, const std::string& detail);

    InstallerEnv env_;
    std::string appId_;
    JobKind kind_;
    CancelToken cancel_;
    mutable std::mutex mutex_;
    JobProgress progress_;
    std::vector<JobState> trail_;
};

struct UninstallResult {
    bool ok = false;
    std::string error;
    std::vector<std::string> blockedBy;  // installed apps that depend on the target
};

// Refuses when other installed apps depend on this one, unless force is set.
UninstallResult uninstallApp(InstallerEnv& env, const std::string& appId, bool force = false);

}  // namespace exile
