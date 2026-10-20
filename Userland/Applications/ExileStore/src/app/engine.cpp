/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include "engine.h"

#include <algorithm>
#include <cstdlib>

#include "../core/util.h"

namespace fs = std::filesystem;

namespace exile::app {

namespace {
constexpr size_t kMaxRememberedTasks = 40;
}

void resolveDefaults(EngineConfig& cfg)
{
    if (cfg.dataDir.empty())
        cfg.dataDir = util::defaultDataDir();
    if (cfg.catalog.empty())
        cfg.catalog = util::findDefaultCatalog();
    if (cfg.trust.empty()) {
        fs::path base = cfg.catalog.has_parent_path() ? cfg.catalog.parent_path() : fs::path(".");
        cfg.trust = base.parent_path() / "trust" / "trusted-keys.json";
    }
}

char const* toString(TaskKind k)
{
    switch (k) {
    case TaskKind::Install:
        return "install";
    case TaskKind::Update:
        return "update";
    case TaskKind::Uninstall:
        return "uninstall";
    }
    return "install";
}

Engine::Engine(EngineConfig cfg)
    : cfg_(std::move(cfg))
    , verifier_(makeDefaultVerifier())
    , library_((resolveDefaults(cfg_), cfg_.dataDir / "library.json"))
{
    std::error_code ec;
    fs::create_directories(cfg_.dataDir, ec);

    FileCatalogSource source(cfg_.catalog);
    catalog_ = std::make_shared<CatalogState const>(loadCatalog(source, cfg_.dataDir / "catalog-cache.json"));

    std::string terr;
    if (auto ts = TrustStore::loadFile(cfg_.trust.string(), terr))
        trust_ = std::move(*ts);
    else
        notices_.push_back("No trusted publisher keys could be loaded (" + terr + "). Installs are disabled.");

    std::string lerr;
    if (!library_.load(lerr))
        notices_.push_back(lerr);
    libraryView_ = std::make_shared<Library const>(library_);

    backend_ = std::make_unique<LocalPrefixBackend>(cfg_.dataDir / "prefix");
    backend_->recoverInterrupted();
    fs::path base = catalog_->catalog ? fs::path(catalog_->catalog->baseDir) : cfg_.catalog.parent_path();
    local_ = std::make_unique<LocalTransport>(base, cfg_.allowFileUrls);
    transport_ = std::make_unique<RoutingTransport>(*local_, https_);

    worker_ = std::thread([this] { workerLoop(); });
}

Engine::~Engine()
{
    {
        std::lock_guard<std::mutex> lk(mu_);
        stop_ = true;
        if (active_)
            active_->cancelToken().cancel();
    }
    cv_.notify_all();
    if (worker_.joinable())
        worker_.join();
}

Snapshot Engine::snapshot() const
{
    std::lock_guard<std::mutex> lk(mu_);
    return Snapshot { catalog_, libraryView_ };
}

void Engine::refreshCatalog()
{
    FileCatalogSource source(cfg_.catalog);
    auto fresh = std::make_shared<CatalogState const>(loadCatalog(source, cfg_.dataDir / "catalog-cache.json"));
    std::lock_guard<std::mutex> lk(mu_);
    catalog_ = std::move(fresh);
}

void Engine::publishLibrary()
{
    auto copy = std::make_shared<Library const>(library_);
    std::lock_guard<std::mutex> lk(mu_);
    libraryView_ = std::move(copy);
}

uint64_t Engine::submit(TaskKind kind, std::string const& appId, std::string& error)
{
    std::lock_guard<std::mutex> lk(mu_);
    Catalog const* cat = catalog_ && catalog_->catalog ? &*catalog_->catalog : nullptr;
    AppEntry const* app = cat ? cat->find(appId) : nullptr;
    if (kind != TaskKind::Uninstall && !app) {
        error = cat ? "That app is not in the catalog." : "The catalog is not available right now.";
        return 0;
    }
    for (auto const& [id, t] : tasks_)
        if (!t.finished && t.appId == appId) {
            error = "Something is already in progress for this app.";
            return 0;
        }
    if (kind == TaskKind::Uninstall && !libraryView_->find(appId)) {
        error = "That app is not installed.";
        return 0;
    }
    TaskInfo t;
    t.id = nextId_++;
    t.kind = kind;
    t.appId = appId;
    if (app)
        t.appName = app->name;
    else if (InstalledApp const* i = libraryView_->find(appId))
        t.appName = i->name.empty() ? appId : i->name;
    else
        t.appName = appId;
    t.message = "Waiting";
    tasks_[t.id] = t;
    order_.push_front(t.id);
    queue_.push_back(t.id);
    while (order_.size() > kMaxRememberedTasks) {
        uint64_t old = order_.back();
        auto it = tasks_.find(old);
        if (it != tasks_.end() && !it->second.finished)
            break;
        tasks_.erase(old);
        order_.pop_back();
    }
    cv_.notify_all();
    return t.id;
}

bool Engine::cancel(uint64_t id)
{
    std::lock_guard<std::mutex> lk(mu_);
    auto it = tasks_.find(id);
    if (it == tasks_.end() || it->second.finished)
        return false;
    if (active_ && activeId_ == id) {
        if (it->second.state == JobState::Installing)
            return false; // not safely cancellable
        it->second.cancelRequested = true;
        active_->cancelToken().cancel();
        return true;
    }
    auto q = std::find(queue_.begin(), queue_.end(), id);
    if (q == queue_.end())
        return false;
    queue_.erase(q);
    TaskInfo& t = it->second;
    t.state = JobState::Cancelled;
    t.finished = true;
    t.message = "Cancelled";
    idleCv_.notify_all();
    return true;
}

TaskInfo Engine::liveInfo(TaskInfo const& stored) const
{
    TaskInfo t = stored;
    if (!t.finished && active_ && activeId_ == t.id) {
        JobProgress p = active_->progress();
        t.state = p.state;
        t.done = p.done;
        t.total = p.total;
        t.message = p.message;
    }
    return t;
}

std::optional<TaskInfo> Engine::task(uint64_t id) const
{
    std::lock_guard<std::mutex> lk(mu_);
    auto it = tasks_.find(id);
    if (it == tasks_.end())
        return std::nullopt;
    return liveInfo(it->second);
}

std::vector<TaskInfo> Engine::tasks() const
{
    std::lock_guard<std::mutex> lk(mu_);
    std::vector<TaskInfo> out;
    for (uint64_t id : order_) {
        auto it = tasks_.find(id);
        if (it != tasks_.end())
            out.push_back(liveInfo(it->second));
    }
    return out;
}

void Engine::waitIdle()
{
    std::unique_lock<std::mutex> lk(mu_);
    idleCv_.wait(lk, [this] { return queue_.empty() && !busy_; });
}

InstallerEnv Engine::makeEnv(Catalog const* cat)
{
    InstallerEnv e;
    e.catalog = cat;
    e.device = &cfg_.device;
    e.library = &library_;
    e.backend = backend_.get();
    e.transport = transport_.get();
    e.trust = &trust_;
    e.verifier = verifier_.get();
    e.cacheDir = cfg_.dataDir / "downloads";
    return e;
}

void Engine::workerLoop()
{
    for (;;) {
        uint64_t id = 0;
        {
            std::unique_lock<std::mutex> lk(mu_);
            cv_.wait(lk, [this] { return stop_ || !queue_.empty(); });
            if (stop_)
                return;
            id = queue_.front();
            queue_.pop_front();
            busy_ = true;
        }
        runTask(id);
        {
            std::lock_guard<std::mutex> lk(mu_);
            busy_ = false;
        }
        idleCv_.notify_all();
    }
}

void Engine::runTask(uint64_t id)
{
    TaskKind kind;
    std::string appId;
    std::shared_ptr<CatalogState const> cs;
    {
        std::lock_guard<std::mutex> lk(mu_);
        TaskInfo& t = tasks_[id];
        kind = t.kind;
        appId = t.appId;
        cs = catalog_;
        t.state = JobState::CheckingCompatibility;
        t.message = "Starting";
    }
    Catalog const* cat = cs && cs->catalog ? &*cs->catalog : nullptr;
    InstallerEnv env = makeEnv(cat);

    TaskInfo result;
    if (kind == TaskKind::Uninstall) {
        UninstallResult ur = uninstallApp(env, appId, false);
        result.ok = ur.ok;
        result.error = ur.error;
        result.blockedBy = ur.blockedBy;
        result.state = ur.ok ? JobState::Succeeded : JobState::Failed;
        result.message = ur.ok ? "Removed" : "Not removed";
    } else {
        InstallJob job(env, appId, kind == TaskKind::Update ? JobKind::Update : JobKind::Install);
        {
            std::lock_guard<std::mutex> lk(mu_);
            active_ = &job;
            activeId_ = id;
            if (stop_)
                job.cancelToken().cancel();
        }
        JobResult r = job.run();
        {
            std::lock_guard<std::mutex> lk(mu_);
            active_ = nullptr;
            activeId_ = 0;
        }
        result.ok = r.ok();
        result.state = r.finalState;
        result.error = r.error;
        result.detail = r.detail;
        result.message = r.ok() ? "Done" : (r.finalState == JobState::Cancelled ? "Cancelled" : "Did not finish");
    }
    publishLibrary();
    std::lock_guard<std::mutex> lk(mu_);
    TaskInfo& t = tasks_[id];
    t.finished = true;
    t.ok = result.ok;
    t.state = result.state;
    t.error = result.error;
    t.detail = result.detail;
    t.message = result.message;
    t.blockedBy = result.blockedBy;
}

} // namespace exile::app
