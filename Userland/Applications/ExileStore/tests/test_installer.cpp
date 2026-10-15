#include "core/installer.h"
#include "testing.h"

using namespace exile;
namespace fs = std::filesystem;
using testing::Fixture;
using testing::PkgSpec;

TEST(job_state_transitions_are_enforced) {
    using S = JobState;
    CHECK(isValidTransition(S::Queued, S::CheckingCompatibility));
    CHECK(isValidTransition(S::CheckingSpace, S::Downloading));
    CHECK(isValidTransition(S::Verifying, S::Installing));
    CHECK(isValidTransition(S::Installing, S::Succeeded));
    CHECK(isValidTransition(S::Downloading, S::Cancelled));
    CHECK(isValidTransition(S::Verifying, S::Downloading));        // retry after a corrupt resume
    CHECK(!isValidTransition(S::Queued, S::Installing));            // cannot skip verification
    CHECK(!isValidTransition(S::Downloading, S::Installing));       // cannot skip verification
    CHECK(!isValidTransition(S::Downloading, S::Succeeded));
    CHECK(!isValidTransition(S::Verifying, S::Succeeded));
    CHECK(!isValidTransition(S::Installing, S::Cancelled));         // not cancellable once the installer is working
    for (S terminal : {S::Succeeded, S::Failed, S::Cancelled})
        for (S to : {S::Queued, S::Downloading, S::Succeeded, S::Failed, S::Cancelled}) {
            CHECK(!isValidTransition(terminal, to));
            CHECK(isTerminal(terminal));
        }
    CHECK(!isTerminal(S::Installing));
}

#ifdef EXILE_HAS_OPENSSL

namespace {

// Records what the job asked of the transport.
class RecordingTransport final : public ITransport {
public:
    explicit RecordingTransport(ITransport& inner) : inner_(inner) {}
    int calls = 0;
    std::vector<bool> resumeFlags;
    std::vector<uint64_t> partSizeOnEntry;
    TransferResult fetch(const std::string& s, const fs::path& part, bool resume, const ProgressFn& p, const CancelToken& c) override {
        ++calls;
        resumeFlags.push_back(resume);
        std::error_code ec;
        partSizeOnEntry.push_back(fs::exists(part, ec) ? fs::file_size(part, ec) : 0);
        return inner_.fetch(s, part, resume, p, c);
    }
private:
    ITransport& inner_;
};

class NoVerifier final : public ISignatureVerifier {
public:
    bool available() const override { return false; }
    std::string name() const override { return "none"; }
    bool verifyEd25519(const std::vector<uint8_t>&, std::string_view, const std::vector<uint8_t>&, std::string&) const override { return true; }
};

struct Rig {
    Fixture f;
    fs::path cache = f.dir / "cache";
    LocalPrefixBackend real{f.dir / "prefix"};
    testing::ScriptedBackend backend{real};
    LocalTransport local{f.dir.path(), false};
    testing::FlakyTransport flaky{local};
    RecordingTransport transport{flaky};
    Library lib{f.dir / "lib.json"};
    std::optional<Catalog> catalog;
    std::function<std::optional<uint64_t>(const fs::path&)> freeSpace;
    const ISignatureVerifier* verifierOverride = nullptr;

    void setCatalog(const std::vector<std::string>& apps) { catalog = f.catalog(apps); }

    InstallerEnv env() {
        InstallerEnv e;
        e.catalog = &*catalog;
        e.device = &f.device;
        e.library = &lib;
        e.backend = &backend;
        e.transport = &transport;
        e.trust = &f.trust;
        e.verifier = verifierOverride ? verifierOverride : f.verifier.get();
        e.cacheDir = cache;
        e.freeSpace = freeSpace;
        return e;
    }
    JobResult run(const std::string& id, JobKind kind = JobKind::Install) {
        InstallJob job(env(), id, kind);
        return job.run();
    }
    bool partFilesLeft() const {
        std::error_code ec;
        if (!fs::exists(cache, ec)) return false;
        for (auto& e : fs::directory_iterator(cache, ec))
            if (e.path().extension() == ".part") return true;
        return false;
    }
    size_t historyCount(const std::string& outcome) const {
        size_t n = 0;
        for (auto& h : lib.history())
            if (h.outcome == outcome) ++n;
        return n;
    }
};

}  // namespace

TEST(install_succeeds_through_every_stage_and_is_confirmed) {
    Rig r;
    PkgSpec s;
    r.setCatalog({r.f.build(s).json});
    JobResult res = r.run(s.appId);
    CHECK(res.ok());
    std::vector<JobState> expected = {JobState::Queued, JobState::CheckingCompatibility, JobState::CheckingDependencies,
                                      JobState::CheckingSpace, JobState::Downloading, JobState::Verifying,
                                      JobState::Installing, JobState::Succeeded};
    CHECK(res.trail == expected);
    CHECK(r.real.isInstalled(s.appId, "1.0.0"));                       // the backend really has it
    const InstalledApp* rec = r.lib.find(s.appId);
    CHECK(rec != nullptr);
    CHECK_EQ(rec->version, std::string("1.0.0"));
    CHECK_EQ(rec->publisherId, std::string("org.test"));
    CHECK_EQ(rec->keyId, std::string("test-key"));
    CHECK_EQ(r.historyCount("success"), size_t(1));
    CHECK(!r.partFilesLeft());                                          // download cache cleaned up
    Library fromDisk(r.f.dir / "lib.json");                             // and persisted
    std::string err;
    CHECK(fromDisk.load(err));
    CHECK(fromDisk.find(s.appId) != nullptr);
}

TEST(install_refuses_incompatible_app_without_touching_anything) {
    Rig r;
    PkgSpec s;
    s.devices = "\"mobile\"";
    r.setCatalog({r.f.build(s).json});
    JobResult res = r.run(s.appId);
    CHECK(!res.ok());
    CHECK_EQ(res.finalState, JobState::Failed);
    CHECK_CONTAINS(res.error, "Not available for this device");
    CHECK_EQ(r.backend.installCalls, 0);
    CHECK_EQ(r.transport.calls, 0);                                     // nothing was even downloaded
    CHECK(r.lib.installed().empty());
}

TEST(install_unknown_app_and_double_install_fail_clearly) {
    Rig r;
    PkgSpec s;
    r.setCatalog({r.f.build(s).json});
    CHECK_CONTAINS(r.run("org.nope.nothing").error, "not in the catalog");
    CHECK(r.run(s.appId).ok());
    JobResult again = r.run(s.appId);
    CHECK(!again.ok());
    CHECK_CONTAINS(again.error, "Already installed");
    CHECK_EQ(r.backend.installCalls, 1);
}

TEST(update_flow_installs_newer_version_and_rejects_noop) {
    Rig r;
    PkgSpec v1;
    r.setCatalog({r.f.build(v1).json});
    CHECK(r.run(v1.appId).ok());
    CHECK_CONTAINS(r.run(v1.appId, JobKind::Update).error, "up to date");

    PkgSpec v2 = v1;
    v2.version = "1.1.0";
    v2.payload = "new payload";
    r.setCatalog({r.f.build(v2).json});
    CHECK_CONTAINS(r.run(v1.appId).error, "Use Update");                // install of an installed app points to Update
    JobResult res = r.run(v1.appId, JobKind::Update);
    CHECK(res.ok());
    CHECK_EQ(r.lib.find(v1.appId)->version, std::string("1.1.0"));
    CHECK(r.real.isInstalled(v1.appId, "1.1.0"));
    CHECK(!r.real.isInstalled(v1.appId, "1.0.0"));
    CHECK_EQ(testing::readAll(r.real.installDir(v1.appId) / "README.txt"), std::string("new payload"));
    CHECK_EQ(r.lib.installed().size(), size_t(1));
}

TEST(update_of_uninstalled_app_fails) {
    Rig r;
    PkgSpec s;
    r.setCatalog({r.f.build(s).json});
    CHECK_CONTAINS(r.run(s.appId, JobKind::Update).error, "not installed");
}

TEST(update_never_downgrades) {
    Rig r;
    PkgSpec newer;
    newer.version = "2.0.0";
    r.setCatalog({r.f.build(newer).json});
    CHECK(r.run(newer.appId).ok());
    PkgSpec older = newer;
    older.version = "1.0.0";
    r.setCatalog({r.f.build(older).json});                              // catalog now offers an older version
    JobResult res = r.run(newer.appId, JobKind::Update);
    CHECK(!res.ok());
    CHECK_EQ(r.lib.find(newer.appId)->version, std::string("2.0.0"));
}

TEST(dependencies_are_checked_with_clear_messages) {
    Rig r;
    PkgSpec lib;
    lib.appId = "org.test.lib"; lib.name = "Test Lib";
    PkgSpec app;
    app.extraPkg = ",\"dependencies\":[{\"appId\":\"org.test.lib\",\"minVersion\":\"2.0.0\"}]";
    r.setCatalog({r.f.build(lib).json, r.f.build(app).json});

    JobResult missing = r.run(app.appId);
    CHECK(!missing.ok());
    CHECK_CONTAINS(missing.error, "Test Lib (not installed)");
    CHECK_EQ(r.transport.calls, 0);                                     // fails before downloading anything

    CHECK(r.run(lib.appId).ok());                                       // library is 1.0.0 but 2.0.0 is required
    JobResult tooOld = r.run(app.appId);
    CHECK(!tooOld.ok());
    CHECK_CONTAINS(tooOld.error, "needs 2.0.0");
    CHECK_CONTAINS(tooOld.error, "you have 1.0.0");

    r.lib.upsert({lib.appId, "Test Lib", "2.0.0", "main", "", "", "", ""});
    CHECK(r.run(app.appId).ok());
}

TEST(install_checks_storage_before_downloading) {
    Rig r;
    PkgSpec s;
    r.setCatalog({r.f.build(s).json});
    r.freeSpace = [](const fs::path&) -> std::optional<uint64_t> { return 10; };
    JobResult res = r.run(s.appId);
    CHECK(!res.ok());
    CHECK_CONTAINS(res.error, "Not enough storage");
    CHECK_EQ(r.transport.calls, 0);
    CHECK(r.lib.installed().empty());

    r.freeSpace = [](const fs::path&) -> std::optional<uint64_t> { return std::nullopt; };
    CHECK_CONTAINS(r.run(s.appId).error, "Could not check free storage");

    r.freeSpace = [](const fs::path&) -> std::optional<uint64_t> { return 1ull << 40; };
    CHECK(r.run(s.appId).ok());
}

TEST(failed_download_reports_cleanly_and_changes_nothing) {
    Rig r;
    PkgSpec s;
    r.setCatalog({r.f.build(s).json});
    r.flaky.failNext = true;
    JobResult res = r.run(s.appId);
    CHECK(!res.ok());
    CHECK_EQ(res.finalState, JobState::Failed);
    CHECK_CONTAINS(res.error, "Download failed");
    CHECK_CONTAINS(res.error, "connection reset");
    CHECK_CONTAINS(res.error, "resume");
    CHECK_EQ(r.backend.installCalls, 0);
    CHECK(r.lib.installed().empty());
    CHECK_EQ(r.historyCount("failed"), size_t(1));
    CHECK(r.run(s.appId).ok());                                         // and a retry just works
}

TEST(interrupted_download_resumes_instead_of_restarting) {
    Rig r;
    PkgSpec s;
    s.payload = std::string(400000, 'p');                               // big enough to be interrupted mid-transfer
    auto built = r.f.build(s);
    r.setCatalog({built.json});

    r.flaky.interruptNext = true;
    JobResult first = r.run(s.appId);
    CHECK_EQ(first.finalState, JobState::Cancelled);
    CHECK(r.lib.installed().empty());
    CHECK(r.partFilesLeft());                                           // partial file kept for resuming
    CHECK_EQ(r.historyCount("cancelled"), size_t(1));

    JobResult second = r.run(s.appId);
    CHECK(second.ok());
    CHECK_EQ(r.transport.calls, 2);
    CHECK(r.transport.resumeFlags[1]);
    CHECK(r.transport.partSizeOnEntry[1] > 0);                          // second attempt started from existing bytes
    CHECK(r.transport.partSizeOnEntry[1] < built.size);
    CHECK(r.real.isInstalled(s.appId, "1.0.0"));
}

TEST(corrupt_resumed_download_is_discarded_and_retried_once) {
    Rig r;
    PkgSpec s;
    s.payload = std::string(100000, 'c');
    r.setCatalog({r.f.build(s).json});
    r.flaky.corruptPartial = true;
    // Seed a stale part file so the job attempts a resume, then corrupt it.
    fs::create_directories(r.cache);
    testing::writeFile(r.cache / (r.catalog->find(s.appId)->packages[0].sha256 + ".part"), "xx");
    JobResult res = r.run(s.appId);
    CHECK(res.ok());
    CHECK_EQ(r.transport.calls, 2);                                     // resume failed verification, clean retry succeeded
    CHECK(!r.transport.resumeFlags.empty() && r.transport.resumeFlags[0]);
    CHECK(!r.transport.resumeFlags[1]);                                 // the retry is a clean download, not a resume
    CHECK_EQ(r.transport.partSizeOnEntry[1], uint64_t(0));              // the bad partial had been deleted first
    CHECK(r.real.isInstalled(s.appId, "1.0.0"));
}

TEST(tampered_package_fails_integrity_and_is_never_installed) {
    Rig r;
    PkgSpec s;
    s.tamperPackageAfterSigning = true;                                 // same size, one flipped bit
    r.setCatalog({r.f.build(s).json});
    JobResult res = r.run(s.appId);
    CHECK(!res.ok());
    CHECK_CONTAINS(res.error, "integrity check");
    CHECK_CONTAINS(res.error, "not installed");
    CHECK_EQ(r.backend.installCalls, 0);
    CHECK(r.lib.installed().empty());
    CHECK(!r.partFilesLeft());                                          // the bad file was discarded
}

TEST(wrong_size_package_fails_integrity) {
    Rig r;
    PkgSpec s;
    auto built = r.f.build(s);
    std::string json = built.json;
    auto pos = json.find("\"downloadSize\":" + std::to_string(built.size));
    json.replace(pos, std::string("\"downloadSize\":").size() + std::to_string(built.size).size(),
                 "\"downloadSize\":" + std::to_string(built.size + 5));
    r.setCatalog({json});
    JobResult res = r.run(s.appId);
    CHECK(!res.ok());
    CHECK_EQ(r.backend.installCalls, 0);
}

TEST(untrusted_or_unsigned_packages_are_never_installed) {
    {
        Rig r;
        PkgSpec s;
        s.signWithUntrustedKey = true;
        r.setCatalog({r.f.build(s).json});
        JobResult res = r.run(s.appId);
        CHECK(!res.ok());
        CHECK_CONTAINS(res.error, "not trusted");
        CHECK_EQ(r.backend.installCalls, 0);
        CHECK(r.lib.installed().empty());
        CHECK(!r.partFilesLeft());
    }
    {
        Rig r;
        PkgSpec s;
        s.omitSignature = true;
        r.setCatalog({r.f.build(s).json});
        JobResult res = r.run(s.appId);
        CHECK(!res.ok());
        CHECK_CONTAINS(res.error, "not signed");
        CHECK_EQ(r.backend.installCalls, 0);
    }
}

TEST(swapping_the_package_and_digest_in_metadata_is_caught_by_the_signature) {
    // An attacker controlling catalog metadata (not the signing key) replaces the package and updates the
    // digest/size so the integrity check passes. The publisher's signature must still reject it.
    Rig r;
    PkgSpec s;
    auto built = r.f.build(s);
    std::string evil = "attacker controlled bytes";
    testing::writeFile(built.file, evil);
    // A real attacker would ship a valid container; use one so the failure is the signature, not the format.
    testing::writeFile(r.f.dir / "evil-src" / "README.txt", "evil");
    std::string err;
    packExpkg(r.f.dir / "evil-src", built.file, err);
    std::string newSha = *sha256FileHex(built.file);
    uint64_t newSize = fs::file_size(built.file);
    std::string json = built.json;
    json.replace(json.find(built.sha256), 64, newSha);
    json.replace(json.find("\"downloadSize\":" + std::to_string(built.size)), std::string("\"downloadSize\":").size() + std::to_string(built.size).size(),
                 "\"downloadSize\":" + std::to_string(newSize));
    r.setCatalog({json});
    JobResult res = r.run(s.appId);
    CHECK(!res.ok());
    CHECK_CONTAINS(res.error, "signature is invalid");
    CHECK_EQ(r.backend.installCalls, 0);
    CHECK(r.lib.installed().empty());
}

TEST(no_signature_verifier_means_no_installs) {
    Rig r;
    NoVerifier none;
    r.verifierOverride = &none;
    PkgSpec s;
    r.setCatalog({r.f.build(s).json});
    JobResult res = r.run(s.appId);
    CHECK(!res.ok());
    CHECK_CONTAINS(res.error, "cannot verify signatures");
    CHECK_EQ(r.backend.installCalls, 0);
}

TEST(backend_failure_is_reported_and_not_recorded) {
    Rig r;
    PkgSpec s;
    r.setCatalog({r.f.build(s).json});
    r.backend.failInstall = true;
    JobResult res = r.run(s.appId);
    CHECK(!res.ok());
    CHECK_EQ(res.finalState, JobState::Failed);
    CHECK_CONTAINS(res.error, "simulated installer failure");
    CHECK(r.lib.installed().empty());
    CHECK(!r.real.isInstalled(s.appId, "1.0.0"));
    CHECK_EQ(r.historyCount("failed"), size_t(1));
}

TEST(success_is_claimed_only_after_the_backend_confirms) {
    Rig r;
    PkgSpec s;
    r.setCatalog({r.f.build(s).json});
    r.backend.installButDoNotConfirm = true;                            // backend says "ok" but installed nothing
    JobResult res = r.run(s.appId);
    CHECK(!res.ok());
    CHECK_CONTAINS(res.error, "did not confirm");
    CHECK(r.lib.installed().empty());                                   // the UI must not show it as installed
    CHECK_EQ(r.historyCount("success"), size_t(0));
}

TEST(cancelling_before_work_starts_cancels_cleanly) {
    Rig r;
    PkgSpec s;
    r.setCatalog({r.f.build(s).json});
    InstallJob job(r.env(), s.appId, JobKind::Install);
    job.cancelToken().cancel();
    JobResult res = job.run();
    CHECK_EQ(res.finalState, JobState::Cancelled);
    CHECK_EQ(r.transport.calls, 0);
    CHECK(r.lib.installed().empty());
}

TEST(async_run_reports_progress_and_final_state) {
    Rig r;
    PkgSpec s;
    r.setCatalog({r.f.build(s).json});
    InstallJob job(r.env(), s.appId, JobKind::Install);
    auto fut = job.runAsync();
    JobResult res = fut.get();
    CHECK(res.ok());
    CHECK_EQ(job.progress().state, JobState::Succeeded);
}

TEST(uninstall_removes_app_and_records_history) {
    Rig r;
    PkgSpec s;
    r.setCatalog({r.f.build(s).json});
    CHECK(r.run(s.appId).ok());
    InstallerEnv env = r.env();
    UninstallResult ur = uninstallApp(env, s.appId);
    CHECK(ur.ok);
    CHECK(r.lib.find(s.appId) == nullptr);
    CHECK(!r.real.isInstalled(s.appId, "1.0.0"));
    CHECK(!uninstallApp(env, s.appId).ok);                              // second time: clear failure
    Library fromDisk(r.f.dir / "lib.json");
    std::string err;
    fromDisk.load(err);
    CHECK(fromDisk.find(s.appId) == nullptr);
}

TEST(uninstall_failure_keeps_the_library_record) {
    Rig r;
    PkgSpec s;
    r.setCatalog({r.f.build(s).json});
    CHECK(r.run(s.appId).ok());
    r.backend.failUninstall = true;
    InstallerEnv env = r.env();
    UninstallResult ur = uninstallApp(env, s.appId);
    CHECK(!ur.ok);
    CHECK(r.lib.find(s.appId) != nullptr);                              // still shown as installed because it still is
    CHECK(r.real.isInstalled(s.appId, "1.0.0"));
}

TEST(uninstall_is_blocked_when_other_apps_depend_on_it) {
    Rig r;
    PkgSpec lib;
    lib.appId = "org.test.lib"; lib.name = "Test Lib";
    PkgSpec app;
    app.extraPkg = ",\"dependencies\":[{\"appId\":\"org.test.lib\"}]";
    r.setCatalog({r.f.build(lib).json, r.f.build(app).json});
    CHECK(r.run(lib.appId).ok());
    CHECK(r.run(app.appId).ok());
    InstallerEnv env = r.env();
    UninstallResult blocked = uninstallApp(env, lib.appId);
    CHECK(!blocked.ok);
    CHECK_EQ(blocked.blockedBy.size(), size_t(1));
    CHECK_CONTAINS(blocked.error, "Test App");
    CHECK(r.real.isInstalled(lib.appId, "1.0.0"));
    CHECK(uninstallApp(env, app.appId).ok);
    CHECK(uninstallApp(env, lib.appId).ok);                             // fine once the dependent is gone
}

TEST(existing_apps_and_configuration_are_preserved) {
    Rig r;
    // Pre-existing state the store did not create: another installed app, its files, a settings file.
    PkgSpec old;
    old.appId = "org.test.legacy"; old.name = "Legacy";
    r.setCatalog({r.f.build(old).json});
    CHECK(r.run(old.appId).ok());
    testing::writeFile(r.f.dir / "settings.conf", "theme=dark\n");
    testing::writeFile(r.f.dir / "prefix" / "unrelated.txt", "mine");
    std::string legacyFile = testing::readAll(r.real.installDir(old.appId) / "README.txt");

    PkgSpec fresh;
    r.setCatalog({r.f.build(old).json, r.f.build(fresh).json});
    CHECK(r.run(fresh.appId).ok());
    InstallerEnv env = r.env();
    CHECK(uninstallApp(env, fresh.appId).ok);

    CHECK(r.lib.find(old.appId) != nullptr);
    CHECK(r.real.isInstalled(old.appId, "1.0.0"));
    CHECK_EQ(testing::readAll(r.real.installDir(old.appId) / "README.txt"), legacyFile);
    CHECK_EQ(testing::readAll(r.f.dir / "settings.conf"), std::string("theme=dark\n"));
    CHECK_EQ(testing::readAll(r.f.dir / "prefix" / "unrelated.txt"), std::string("mine"));
}

#endif  // EXILE_HAS_OPENSSL
