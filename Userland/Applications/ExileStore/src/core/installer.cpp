#include "core/installer.h"

#include <algorithm>
#include <system_error>

#include "core/sha256.h"
#include "core/util.h"
#include "core/version.h"

#if defined(__unix__) || defined(__APPLE__)
#include <sys/stat.h>
#define EXILE_HAVE_STAT 1
#endif

namespace fs = std::filesystem;

namespace exile {

const char* toString(JobState s) {
    switch (s) {
        case JobState::Queued: return "Queued";
        case JobState::CheckingCompatibility: return "Checking compatibility";
        case JobState::CheckingDependencies: return "Checking requirements";
        case JobState::CheckingSpace: return "Checking storage";
        case JobState::Downloading: return "Downloading";
        case JobState::Verifying: return "Verifying";
        case JobState::Installing: return "Installing";
        case JobState::Succeeded: return "Done";
        case JobState::Failed: return "Failed";
        case JobState::Cancelled: return "Cancelled";
    }
    return "?";
}

bool isTerminal(JobState s) { return s == JobState::Succeeded || s == JobState::Failed || s == JobState::Cancelled; }

bool isValidTransition(JobState from, JobState to) {
    using S = JobState;
    switch (from) {
        case S::Queued: return to == S::CheckingCompatibility || to == S::Cancelled;
        case S::CheckingCompatibility: return to == S::CheckingDependencies || to == S::Failed || to == S::Cancelled;
        case S::CheckingDependencies: return to == S::CheckingSpace || to == S::Failed || to == S::Cancelled;
        case S::CheckingSpace: return to == S::Downloading || to == S::Failed || to == S::Cancelled;
        case S::Downloading: return to == S::Verifying || to == S::Failed || to == S::Cancelled;
        case S::Verifying: return to == S::Installing || to == S::Downloading || to == S::Failed || to == S::Cancelled;
        case S::Installing: return to == S::Succeeded || to == S::Failed;  // not cancellable once the backend is working
        default: return false;
    }
}

namespace {

std::optional<uint64_t> defaultFreeSpace(const fs::path& path) {
    std::error_code ec;
    fs::path p = path;
    while (!p.empty() && !fs::exists(p, ec)) {
        fs::path parent = p.parent_path();
        if (parent == p) break;
        p = parent;
    }
    if (p.empty()) p = ".";
    auto info = fs::space(p, ec);
    if (ec) return std::nullopt;
    return static_cast<uint64_t>(info.available);
}

bool sameDevice(const fs::path& a, const fs::path& b) {
#ifdef EXILE_HAVE_STAT
    auto existing = [](fs::path p) {
        std::error_code ec;
        while (!p.empty() && !fs::exists(p, ec)) {
            fs::path parent = p.parent_path();
            if (parent == p) break;
            p = parent;
        }
        return p.empty() ? fs::path(".") : p;
    };
    struct stat sa{}, sb{};
    if (::stat(existing(a).c_str(), &sa) != 0 || ::stat(existing(b).c_str(), &sb) != 0) return true;
    return sa.st_dev == sb.st_dev;
#else
    (void)a; (void)b;
    return true;
#endif
}

uint64_t withMargin(uint64_t bytes) { return bytes + bytes / 20; }  // +5%

}  // namespace

InstallJob::InstallJob(InstallerEnv env, std::string appId, JobKind kind)
    : env_(std::move(env)), appId_(std::move(appId)), kind_(kind) {
    if (!env_.freeSpace) env_.freeSpace = defaultFreeSpace;
    trail_.push_back(JobState::Queued);
}

JobProgress InstallJob::progress() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return progress_;
}

void InstallJob::setProgress(uint64_t done, uint64_t total) {
    std::lock_guard<std::mutex> lock(mutex_);
    progress_.done = done;
    progress_.total = total;
}

bool InstallJob::transition(JobState to, std::string message) {
    std::lock_guard<std::mutex> lock(mutex_);
    JobState from = trail_.back();
    if (!isValidTransition(from, to)) return false;
    trail_.push_back(to);
    progress_.state = to;
    progress_.message = message.empty() ? toString(to) : std::move(message);
    if (to != JobState::Downloading) { progress_.done = 0; progress_.total = 0; }
    return true;
}

JobResult InstallJob::finish(JobState terminal, std::string error, std::string detail) {
    // Terminal states are reachable from any non-terminal state except Installing->Cancelled;
    // transition() enforces that, and a rejected transition becomes a plain failure.
    if (!transition(terminal, error)) {
        std::lock_guard<std::mutex> lock(mutex_);
        trail_.push_back(JobState::Failed);
        progress_.state = JobState::Failed;
        terminal = JobState::Failed;
    }
    JobResult r;
    r.finalState = terminal;
    r.error = std::move(error);
    r.detail = std::move(detail);
    std::lock_guard<std::mutex> lock(mutex_);
    r.trail = trail_;
    return r;
}

void InstallJob::record(const std::string& outcome, const std::string& version, const std::string& detail) {
    if (!env_.library) return;
    env_.library->addHistory({util::nowIso8601(), appId_, kind_ == JobKind::Update ? "update" : "install", version, outcome, detail});
    std::string ignored;
    env_.library->save(ignored);
}

std::future<JobResult> InstallJob::runAsync() {
    return std::async(std::launch::async, [this] { return run(); });
}

JobResult InstallJob::run() {
    auto failWith = [&](std::string msg, const std::string& version = {}) {
        record("failed", version, msg);
        return finish(JobState::Failed, std::move(msg));
    };
    auto cancelled = [&](const std::string& version = {}) {
        record("cancelled", version, "cancelled by the user");
        return finish(JobState::Cancelled, "Cancelled.");
    };

    if (!env_.catalog || !env_.device || !env_.library || !env_.backend || !env_.transport || !env_.trust || !env_.verifier)
        return failWith("The store is not fully set up (internal error).");

    // ---- 1. compatibility ----
    transition(JobState::CheckingCompatibility);
    const AppEntry* app = env_.catalog->find(appId_);
    if (!app) return failWith("This app is not in the catalog.");
    Compatibility compat = resolveCompatibility(*app, *env_.device);
    if (!compat.ok()) return failWith(compat.reason);
    const PackageBuild& build = *compat.build;
    const std::string version = compat.version;

    const InstalledApp* installed = env_.library->find(appId_);
    if (kind_ == JobKind::Install && installed)
        return failWith(compareVersionStrings(version, installed->version) > 0
                            ? "Already installed. Use Update to get version " + version + "."
                            : "Already installed (version " + installed->version + ").", version);
    if (kind_ == JobKind::Update) {
        if (!installed) return failWith("This app is not installed.", version);
        if (compareVersionStrings(version, installed->version) <= 0)
            return failWith("Already up to date (version " + installed->version + ").", version);
    }
    if (cancel_.cancelled()) return cancelled(version);

    // ---- 2. dependencies ----
    transition(JobState::CheckingDependencies);
    std::vector<std::string> missing;
    for (const Dependency& dep : build.dependencies) {
        const InstalledApp* d = env_.library->find(dep.appId);
        const AppEntry* depApp = env_.catalog->find(dep.appId);
        std::string label = depApp ? depApp->name : dep.appId;
        if (!d) missing.push_back(label + " (not installed)");
        else if (!dep.minVersion.empty() && compareVersionStrings(d->version, dep.minVersion) < 0)
            missing.push_back(label + " (needs " + dep.minVersion + " or newer, you have " + d->version + ")");
    }
    if (!missing.empty()) {
        std::string msg = "This app needs other apps first: ";
        for (size_t i = 0; i < missing.size(); ++i) msg += (i ? "; " : "") + missing[i];
        return failWith(msg + ".", version);
    }

    // ---- 3. storage ----
    transition(JobState::CheckingSpace);
    std::error_code ec;
    fs::create_directories(env_.cacheDir, ec);
    fs::path part = env_.cacheDir / (build.sha256 + ".part");
    uint64_t existingPart = 0;
    if (fs::exists(part, ec)) existingPart = fs::file_size(part, ec);
    if (existingPart > build.downloadSize) existingPart = 0;
    uint64_t needDownload = build.downloadSize > existingPart ? build.downloadSize - existingPart : 0;
    uint64_t needInstall = build.packageSize;
    if (needDownload || needInstall) {
        fs::path storage = env_.backend->storagePath();
        auto freeCache = env_.freeSpace(env_.cacheDir);
        auto freeStore = env_.freeSpace(storage);
        if (!freeCache || !freeStore) return failWith("Could not check free storage space.", version);
        bool same = sameDevice(env_.cacheDir, storage);
        uint64_t required = same ? withMargin(needDownload + needInstall) : 0;
        bool enough = same ? *freeCache >= required
                           : (*freeCache >= withMargin(needDownload) && *freeStore >= withMargin(needInstall));
        if (!enough) {
            uint64_t shortBy = same ? required - std::min(required, *freeCache) : 1;
            std::string msg = "Not enough storage space";
            if (same) msg += ": about " + util::humanSize(required) + " is needed and " + util::humanSize(*freeCache) +
                             " is free (" + util::humanSize(shortBy) + " short)";
            return failWith(msg + ".", version);
        }
    }
    if (cancel_.cancelled()) return cancelled(version);

    // ---- 4 + 5. download and integrity (one retry from scratch if a resumed file is corrupt) ----
    bool verified = false;
    for (int attempt = 0; attempt < 2 && !verified; ++attempt) {
        bool resumed = attempt == 0 && fs::exists(part, ec) && fs::file_size(part, ec) > 0;
        transition(JobState::Downloading, resumed ? "Resuming download" : "Downloading");
        TransferResult tr = env_.transport->fetch(build.url, part, /*resume=*/attempt == 0,
                                                  [this](uint64_t d, uint64_t t) { setProgress(d, t); }, cancel_);
        if (tr.cancelled) return cancelled(version);
        if (!tr.ok) return failWith("Download failed: " + tr.error + ". Your progress is saved; try again to resume.", version);

        transition(JobState::Verifying, "Verifying download");
        std::string problem;
        if (build.downloadSize > 0 && tr.bytes != build.downloadSize) problem = "size mismatch";
        if (problem.empty()) {
            std::string herr;
            auto digest = sha256FileHex(part, &herr);
            if (!digest) problem = herr;
            else if (*digest != build.sha256) problem = "checksum mismatch";
        }
        if (problem.empty()) { verified = true; break; }
        fs::remove(part, ec);
        if (!resumed) return failWith("The downloaded package failed its integrity check (" + problem + ") and was discarded. It was not installed.", version);
        // Resumed partial was bad: loop once more with a clean download.
    }
    if (!verified) return failWith("The downloaded package failed its integrity check twice and was discarded. It was not installed.", version);

    // ---- 5b. publisher signature / trust ----
    VerifyOutcome vo = verifyPackageSignature(*app, build, version, *env_.trust, *env_.verifier);
    if (!vo.trusted) {
        fs::remove(part, ec);
        return failWith("This package was not installed because " + vo.reason + ".", version);
    }
    if (cancel_.cancelled()) return cancelled(version);

    // Re-verify the exact bytes about to be installed: the verified file could have
    // been replaced on disk between the integrity check above and this point.
    {
        std::string herr;
        auto digest = sha256FileHex(part, &herr);
        if (!digest) return failWith("The package could not be re-read before installing: " + herr, version);
        if (*digest != build.sha256) {
            fs::remove(part, ec);
            return failWith("The package changed on disk after it was verified, so it was discarded and not installed.", version);
        }
    }
    // ---- 6. install through the backend; success only after the backend confirms ----
    transition(JobState::Installing, "Installing");
    InstallRequest req{app->appId, version, build.id, part};
    BackendResult br = env_.backend->install(req);
    if (!br.ok) return failWith("Installation failed: " + br.error, version);
    if (!env_.backend->isInstalled(app->appId, version))
        return failWith("The system installer did not confirm the installation, so it was not recorded.", version);

    InstalledApp rec;
    rec.appId = app->appId;
    rec.name = app->name;
    rec.version = version;
    rec.packageId = build.id;
    rec.installedAt = util::nowIso8601();
    rec.publisherId = vo.publisherId;
    rec.keyId = vo.keyId;
    rec.sha256 = build.sha256;
    env_.library->upsert(std::move(rec));
    record("success", version, "");
    fs::remove(part, ec);

    std::string detail;
    std::string saveErr;
    if (!env_.library->save(saveErr)) detail = "Installed, but the library could not be saved: " + saveErr;
    return finish(JobState::Succeeded, {}, detail);
}

UninstallResult uninstallApp(InstallerEnv& env, const std::string& appId, bool force) {
    UninstallResult r;
    if (!env.library || !env.backend) { r.error = "The store is not fully set up."; return r; }
    const InstalledApp* inst = env.library->find(appId);
    if (!inst) { r.error = "This app is not installed."; return r; }

    if (env.catalog && !force) {
        for (const auto& other : env.library->installed()) {
            if (other.appId == appId) continue;
            const AppEntry* oa = env.catalog->find(other.appId);
            if (!oa) continue;
            for (const auto& b : oa->packages) {
                if (b.id != other.packageId) continue;
                for (const auto& d : b.dependencies)
                    if (d.appId == appId) r.blockedBy.push_back(other.name.empty() ? other.appId : other.name);
            }
        }
        if (!r.blockedBy.empty()) {
            r.error = "Other installed apps need this one: ";
            for (size_t i = 0; i < r.blockedBy.size(); ++i) r.error += (i ? ", " : "") + r.blockedBy[i];
            r.error += ". Uninstall them first.";
            return r;
        }
    }

    std::string version = inst->version;
    BackendResult br = env.backend->uninstall(appId);
    if (!br.ok) {
        env.library->addHistory({util::nowIso8601(), appId, "uninstall", version, "failed", br.error});
        std::string ignored;
        env.library->save(ignored);
        r.error = "Uninstall failed: " + br.error;
        return r;
    }
    env.library->remove(appId);
    env.library->addHistory({util::nowIso8601(), appId, "uninstall", version, "success", ""});
    std::string saveErr;
    env.library->save(saveErr);
    r.ok = true;
    return r;
}

}  // namespace exile
