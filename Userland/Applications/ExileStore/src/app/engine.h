/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#pragma once

// Engine: the thread-safe facade that front-ends other than the terminal (the local
// web service today, a native GUI tomorrow) use to drive the store core.
//
// Threading model
//  * Catalog and library are published as immutable snapshots (shared_ptr<const>).
//    Request threads only ever read a snapshot, so they never race with a job.
//  * All mutation (install, update, uninstall) happens on ONE worker thread that
//    owns the live Library and the package backend. After each job it publishes a
//    fresh library snapshot.
//  * Jobs are queued; at most one runs at a time.

#include <atomic>
#include <condition_variable>
#include <deque>
#include <filesystem>
#include <map>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <thread>
#include <vector>

#include "../core/backend.h"
#include "../core/catalogclient.h"
#include "../core/device.h"
#include "../core/installer.h"
#include "../core/library.h"
#include "../core/transport.h"
#include "../core/trust.h"

namespace exile::app {

struct EngineConfig {
    std::filesystem::path catalog; // empty => search the usual places
    std::filesystem::path trust;   // empty => <catalog dir>/../trust/trusted-keys.json
    std::filesystem::path dataDir; // empty => per-user data directory
    DeviceProfile device = detectHostDevice();
    bool allowFileUrls = false;
};

// Fills in every empty path of the config with its default.
void resolveDefaults(EngineConfig& cfg);

enum class TaskKind { Install,
    Update,
    Uninstall };
char const* toString(TaskKind k);

struct TaskInfo {
    uint64_t id = 0;
    TaskKind kind = TaskKind::Install;
    std::string appId;
    std::string appName;
    JobState state = JobState::Queued;
    uint64_t done = 0;
    uint64_t total = 0;
    std::string message; // what is happening now
    std::string error;   // plain-language reason when it did not succeed
    std::string detail;
    std::vector<std::string> blockedBy; // uninstall refused: installed apps that depend on it
    bool finished = false;
    bool ok = false;
    bool cancelRequested = false;
};

struct Snapshot {
    std::shared_ptr<CatalogState const> catalog;
    std::shared_ptr<Library const> library;
    Catalog const* cat() const { return catalog && catalog->catalog ? &*catalog->catalog : nullptr; }
};

class Engine {
public:
    explicit Engine(EngineConfig cfg);
    ~Engine();
    Engine(Engine const&) = delete;
    Engine& operator=(Engine const&) = delete;

    EngineConfig const& config() const { return cfg_; }
    DeviceProfile const& device() const { return cfg_.device; }
    TrustStore const& trust() const { return trust_; }
    ISignatureVerifier const& verifier() const { return *verifier_; }
    bool canLaunch() const { return backend_->canLaunch(); }
    std::string backendName() const { return backend_->name(); }
    // Notices about missing trust keys / unreadable library, empty when fine.
    std::vector<std::string> startupNotices() const { return notices_; }

    Snapshot snapshot() const;

    // Re-reads the catalog source. Falls back to the cache when the source fails.
    void refreshCatalog();

    // Queues work. Returns 0 and sets `error` when the request is refused up front
    // (unknown app, already queued, not installed, ...). Whether an install is
    // *allowed* is decided by the installer pipeline itself, never by the caller.
    uint64_t submit(TaskKind kind, std::string const& appId, std::string& error);
    bool cancel(uint64_t id);
    std::optional<TaskInfo> task(uint64_t id) const;
    std::vector<TaskInfo> tasks() const; // newest first, bounded history

    // Blocks until the queue is empty and nothing runs. For tests and shutdown.
    void waitIdle();

private:
    struct Pending {
        uint64_t id;
    };
    void workerLoop();
    void runTask(uint64_t id);
    TaskInfo liveInfo(TaskInfo const& stored) const; // overlays progress of the running job
    void publishLibrary();
    InstallerEnv makeEnv(Catalog const* cat);

    EngineConfig cfg_;
    TrustStore trust_;
    std::unique_ptr<ISignatureVerifier> verifier_;
    std::unique_ptr<LocalPrefixBackend> backend_;
    std::unique_ptr<LocalTransport> local_;
    UnavailableHttpsTransport https_;
    std::unique_ptr<RoutingTransport> transport_;
    std::vector<std::string> notices_;

    // Owned by the worker thread only (after construction).
    Library library_;

    mutable std::mutex mu_;
    std::condition_variable cv_;
    std::condition_variable idleCv_;
    std::shared_ptr<CatalogState const> catalog_;
    std::shared_ptr<Library const> libraryView_;
    std::deque<uint64_t> queue_;
    std::map<uint64_t, TaskInfo> tasks_;
    std::deque<uint64_t> order_;
    InstallJob* active_ = nullptr;
    uint64_t activeId_ = 0;
    uint64_t nextId_ = 1;
    bool stop_ = false;
    bool busy_ = false;
    std::thread worker_;
};

} // namespace exile::app
