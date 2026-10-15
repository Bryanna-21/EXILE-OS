#include <fstream>

#include "core/backend.h"
#include "core/expkg.h"
#include "core/library.h"
#include "core/transport.h"
#include "testing.h"

using namespace exile;
namespace fs = std::filesystem;
using testing::TempDir;
using testing::writeFile;

namespace {

// Hand-built package so we can craft hostile headers the packer itself would never produce.
std::string craft(const std::vector<std::pair<std::string, std::string>>& files) {
    std::string header = "EXPKG1\n", data;
    for (const auto& [path, content] : files) {
        header += "file " + std::to_string(content.size()) + " " + path + "\n";
        data += content;
    }
    return header + "data\n" + data;
}

bool rejects(TempDir& d, const std::string& bytes, const std::string& needle = "") {
    writeFile(d / "x.expkg", bytes);
    std::string err;
    auto m = parseExpkg(d / "x.expkg", ExpkgLimits{}, err);
    if (m) return false;
    return needle.empty() || err.find(needle) != std::string::npos;
}

fs::path makePackage(TempDir& d, const std::string& name, const std::string& payload) {
    writeFile(d / ("src-" + name) / "README.txt", payload);
    writeFile(d / ("src-" + name) / "bin" / "tool", "binary " + payload);
    std::string err;
    fs::path out = d / (name + ".expkg");
    packExpkg(d / ("src-" + name), out, err);
    return out;
}

}  // namespace

// ---------------- package container ----------------

TEST(expkg_pack_parse_extract_roundtrip) {
    TempDir d;
    fs::path pkg = makePackage(d, "a", "hello");
    std::string err;
    auto m = parseExpkg(pkg, ExpkgLimits{}, err);
    CHECK(m.has_value());
    CHECK_EQ(m->entries.size(), size_t(2));
    CHECK(extractExpkg(pkg, *m, d / "out", nullptr, err));
    CHECK_EQ(testing::readAll(d / "out" / "README.txt"), std::string("hello"));
    CHECK_EQ(testing::readAll(d / "out" / "bin" / "tool"), std::string("binary hello"));
}

TEST(expkg_packing_is_reproducible) {
    TempDir d;
    fs::path a = makePackage(d, "same", "x");
    std::string first = testing::readAll(a);
    std::string err;
    packExpkg(d / "src-same", d / "again.expkg", err);
    CHECK_EQ(first, testing::readAll(d / "again.expkg"));
}

TEST(expkg_rejects_path_traversal_and_unsafe_names) {
    TempDir d;
    for (const char* bad : {"../evil", "a/../../evil", "/etc/cron.d/x", "a\\b", "./x", "a//b", "dir/", "C:evil"})
        if (!rejects(d, craft({{bad, "x"}}), "unsafe path")) throw testing::Failure{std::string("accepted path: ") + bad};
}

TEST(expkg_rejects_duplicate_and_case_colliding_paths) {
    TempDir d;
    CHECK(rejects(d, craft({{"a.txt", "1"}, {"a.txt", "2"}}), "duplicate"));
    CHECK(rejects(d, craft({{"A.txt", "1"}, {"a.TXT", "2"}}), "duplicate"));
}

TEST(expkg_rejects_file_directory_conflicts) {
    TempDir d;
    CHECK(rejects(d, craft({{"a", "1"}, {"a/b", "2"}}), "conflicts"));
}

TEST(expkg_rejects_truncated_padded_and_garbage) {
    TempDir d;
    std::string good = craft({{"a", "12345"}});
    CHECK(rejects(d, good.substr(0, good.size() - 2), "size"));
    CHECK(rejects(d, good + "extra", "size"));
    CHECK(rejects(d, "NOTAPKG\ndata\n"));
    CHECK(rejects(d, ""));
    CHECK(rejects(d, "EXPKG1\nfile 5 a\n"));                // no data marker
    CHECK(rejects(d, "EXPKG1\nfile x a\ndata\n"));          // non-numeric size
    CHECK(rejects(d, "EXPKG1\nfile 5\ndata\n"));            // missing path
    CHECK(rejects(d, "EXPKG1\nsymlink a b\ndata\n"));       // unknown entry type
    CHECK(rejects(d, "EXPKG1\nfile 99999999999999999 a\ndata\n"));
}

TEST(expkg_enforces_limits) {
    TempDir d;
    std::vector<std::pair<std::string, std::string>> many;
    for (int i = 0; i < 20; ++i) many.push_back({"f" + std::to_string(i), "x"});
    writeFile(d / "m.expkg", craft(many));
    std::string err;
    ExpkgLimits lim;
    lim.maxEntries = 10;
    CHECK(!parseExpkg(d / "m.expkg", lim, err).has_value());
    CHECK_CONTAINS(err, "too many");
    lim = ExpkgLimits{};
    lim.maxTotalBytes = 5;
    writeFile(d / "big.expkg", craft({{"a", "123456"}}));
    CHECK(!parseExpkg(d / "big.expkg", lim, err).has_value());
    CHECK_CONTAINS(err, "too large");
}

TEST(expkg_extract_refuses_to_write_through_symlinks) {
    TempDir d;
    fs::path pkg = makePackage(d, "s", "payload");
    std::string err;
    auto m = parseExpkg(pkg, ExpkgLimits{}, err);
    fs::create_directories(d / "out");
    fs::create_directories(d / "outside");
    fs::create_symlink(d / "outside", d / "out" / "bin");           // directory symlink pointing out of the tree
    CHECK(!extractExpkg(pkg, *m, d / "out", nullptr, err));
    CHECK(!fs::exists(d / "outside" / "tool"));                     // nothing escaped

    fs::remove(d / "out" / "bin");
    fs::remove(d / "out" / "README.txt");                           // the first (safe) file was extracted before the failure
    writeFile(d / "outside" / "victim", "keep me");
    fs::create_symlink(d / "outside" / "victim", d / "out" / "README.txt");  // file symlink
    CHECK(!extractExpkg(pkg, *m, d / "out", nullptr, err));
    CHECK_EQ(testing::readAll(d / "outside" / "victim"), std::string("keep me"));
}

TEST(expkg_pack_refuses_symlinks) {
    TempDir d;
    writeFile(d / "src" / "a.txt", "a");
    fs::create_symlink(d / "a.txt", d / "src" / "link");
    std::string err;
    CHECK(!packExpkg(d / "src", d / "o.expkg", err));
    CHECK_CONTAINS(err, "symlink");
}

// ---------------- local backend ----------------

TEST(backend_installs_and_confirms) {
    TempDir d;
    LocalPrefixBackend be(d / "prefix");
    fs::path pkg = makePackage(d, "app", "v1");
    CHECK(!be.isInstalled("org.t.app", "1.0.0"));
    BackendResult r = be.install({"org.t.app", "1.0.0", "main", pkg});
    CHECK(r.ok);
    CHECK(be.isInstalled("org.t.app", "1.0.0"));
    CHECK(!be.isInstalled("org.t.app", "2.0.0"));                  // confirms the right version only
    CHECK_EQ(testing::readAll(be.installDir("org.t.app") / "README.txt"), std::string("v1"));
}

TEST(backend_upgrade_replaces_atomically_and_cleans_up) {
    TempDir d;
    LocalPrefixBackend be(d / "prefix");
    CHECK(be.install({"org.t.app", "1.0.0", "main", makePackage(d, "v1", "one")}).ok);
    CHECK(be.install({"org.t.app", "2.0.0", "main", makePackage(d, "v2", "two")}).ok);
    CHECK(be.isInstalled("org.t.app", "2.0.0"));
    CHECK_EQ(testing::readAll(be.installDir("org.t.app") / "README.txt"), std::string("two"));
    CHECK(!fs::exists(d / "prefix" / "apps" / "org.t.app" / "previous"));
    CHECK(!fs::exists(d / "prefix" / "staging" / "org.t.app-2.0.0"));
}

TEST(backend_failed_install_leaves_previous_version_untouched) {
    TempDir d;
    LocalPrefixBackend be(d / "prefix");
    CHECK(be.install({"org.t.app", "1.0.0", "main", makePackage(d, "v1", "one")}).ok);
    writeFile(d / "bad.expkg", craft({{"../evil", "x"}}));
    BackendResult r = be.install({"org.t.app", "2.0.0", "main", d / "bad.expkg"});
    CHECK(!r.ok);
    CHECK(be.isInstalled("org.t.app", "1.0.0"));
    CHECK_EQ(testing::readAll(be.installDir("org.t.app") / "README.txt"), std::string("one"));
    CHECK(!fs::exists(d / "evil"));
}

TEST(backend_rejects_hostile_ids_and_reserved_names) {
    TempDir d;
    LocalPrefixBackend be(d / "prefix");
    fs::path pkg = makePackage(d, "app", "x");
    CHECK(!be.install({"../../etc", "1.0.0", "main", pkg}).ok);
    CHECK(!be.install({"org.t.app", "not-a-version", "main", pkg}).ok);
    CHECK(!be.uninstall("../../important").ok);
    CHECK(!be.isInstalled("../x", "1.0.0"));
    writeFile(d / "res.expkg", craft({{".exile-install.json", "{\"appId\":\"org.t.app\",\"version\":\"9.9.9\"}"}}));
    BackendResult r = be.install({"org.t.app", "1.0.0", "main", d / "res.expkg"});
    CHECK(!r.ok);                                       // a package cannot forge the install receipt
    CHECK(!be.isInstalled("org.t.app", "9.9.9"));
}

TEST(backend_uninstall_removes_only_that_app) {
    TempDir d;
    LocalPrefixBackend be(d / "prefix");
    CHECK(be.install({"org.t.one", "1.0.0", "main", makePackage(d, "a", "a")}).ok);
    CHECK(be.install({"org.t.two", "1.0.0", "main", makePackage(d, "b", "b")}).ok);
    CHECK(be.uninstall("org.t.one").ok);
    CHECK(!be.isInstalled("org.t.one", "1.0.0"));
    CHECK(be.isInstalled("org.t.two", "1.0.0"));
    CHECK(!be.uninstall("org.t.one").ok);               // already gone: clear failure, not a silent success
}

TEST(backend_recovers_from_interrupted_install) {
    TempDir d;
    LocalPrefixBackend be(d / "prefix");
    CHECK(be.install({"org.t.app", "1.0.0", "main", makePackage(d, "v1", "one")}).ok);
    // Simulate a crash between "current -> previous" and "staging -> current".
    fs::rename(be.installDir("org.t.app"), d / "prefix" / "apps" / "org.t.app" / "previous");
    fs::create_directories(d / "prefix" / "staging" / "org.t.app-2.0.0");
    writeFile(d / "prefix" / "staging" / "org.t.app-2.0.0" / "half", "partial");
    CHECK(!be.isInstalled("org.t.app", "1.0.0"));
    be.recoverInterrupted();
    CHECK(be.isInstalled("org.t.app", "1.0.0"));        // old version restored
    CHECK(!fs::exists(d / "prefix" / "staging"));        // half-finished staging discarded
}

TEST(backend_cannot_launch_and_says_so) {
    TempDir d;
    LocalPrefixBackend be(d / "prefix");
    CHECK(!be.canLaunch());
    BackendResult r = be.launch("org.t.app");
    CHECK(!r.ok);
    CHECK(!r.error.empty());
}

// ---------------- transport ----------------

TEST(transport_resolves_only_inside_catalog_directory) {
    TempDir d;
    writeFile(d / "cat" / "pkgs" / "a.expkg", "data");
    writeFile(d / "secret.txt", "secret");
    std::string err;
    CHECK(resolveLocalSource("pkgs/a.expkg", d / "cat", false, err).has_value());
    CHECK(!resolveLocalSource("../secret.txt", d / "cat", false, err).has_value());
    CHECK(!resolveLocalSource("/etc/passwd", d / "cat", false, err).has_value());
    CHECK(!resolveLocalSource("http://x/y", d / "cat", false, err).has_value());
    CHECK(!resolveLocalSource("file:///etc/passwd", d / "cat", false, err).has_value());   // disabled by default
    CHECK(resolveLocalSource("file:///etc/passwd", d / "cat", true, err).has_value());     // explicit opt-in only
    fs::create_symlink(d / "secret.txt", d / "cat" / "pkgs" / "link.expkg");               // symlink pointing out
    CHECK(!resolveLocalSource("pkgs/link.expkg", d / "cat", false, err).has_value());
    CHECK_CONTAINS(err, "escapes");
}

TEST(transport_copies_resumes_and_reports_progress) {
    TempDir d;
    std::string big(300000, 'z');
    writeFile(d / "cat" / "f.bin", big);
    LocalTransport t(d / "cat", false);
    CancelToken never;
    uint64_t lastDone = 0, lastTotal = 0;
    auto r = t.fetch("f.bin", d / "part", false, [&](uint64_t a, uint64_t b) { lastDone = a; lastTotal = b; }, never);
    CHECK(r.ok);
    CHECK_EQ(lastDone, uint64_t(300000));
    CHECK_EQ(lastTotal, uint64_t(300000));
    CHECK_EQ(testing::readAll(d / "part"), big);

    // Resume: keep the first 100000 bytes, fetch the rest.
    writeFile(d / "part2", big.substr(0, 100000));
    uint64_t firstSeen = 0;
    bool first = true;
    r = t.fetch("f.bin", d / "part2", true, [&](uint64_t a, uint64_t) { if (first) { firstSeen = a; first = false; } }, never);
    CHECK(r.ok);
    CHECK_EQ(firstSeen, uint64_t(100000));              // started from the existing offset, did not restart
    CHECK_EQ(testing::readAll(d / "part2"), big);
}

TEST(transport_cancellation_keeps_partial_file_for_resume) {
    TempDir d;
    writeFile(d / "cat" / "f.bin", std::string(500000, 'q'));
    LocalTransport t(d / "cat", false);
    CancelToken stop;
    auto r = t.fetch("f.bin", d / "part", false, [&](uint64_t done, uint64_t) { if (done > 100000) stop.cancel(); }, stop);
    CHECK(!r.ok);
    CHECK(r.cancelled);
    uint64_t kept = fs::file_size(d / "part");
    CHECK(kept > 0 && kept < 500000);
}

TEST(transport_discards_stale_oversized_partial) {
    TempDir d;
    writeFile(d / "cat" / "f.bin", "short");
    writeFile(d / "part", std::string(1000, 'x'));       // larger than the real file: cannot be a valid prefix
    LocalTransport t(d / "cat", false);
    CancelToken never;
    auto r = t.fetch("f.bin", d / "part", true, nullptr, never);
    CHECK(r.ok);
    CHECK_EQ(testing::readAll(d / "part"), std::string("short"));
}

TEST(transport_missing_source_and_https_are_clear_errors) {
    TempDir d;
    LocalTransport local(d.path(), false);
    UnavailableHttpsTransport https;
    RoutingTransport router(local, https);
    CancelToken never;
    auto r = router.fetch("nope.bin", d / "p", false, nullptr, never);
    CHECK(!r.ok);
    CHECK_CONTAINS(r.error, "not found");
    r = router.fetch("https://example.com/a.expkg", d / "p", false, nullptr, never);
    CHECK(!r.ok);
    CHECK_CONTAINS(r.error, "not available");
    CHECK(!fs::exists(d / "p"));                         // nothing was written
}

// ---------------- library ----------------

TEST(library_save_load_roundtrip_and_history_limit) {
    TempDir d;
    Library lib(d / "lib.json");
    lib.upsert({"org.t.a", "A", "1.0.0", "main", "2026-01-01T00:00:00Z", "org.t", "k", "ff"});
    for (int i = 0; i < 250; ++i) lib.addHistory({"t", "org.t.a", "install", "1.0.0", "success", std::to_string(i)});
    std::string err;
    CHECK(lib.save(err));
    Library again(d / "lib.json");
    CHECK(again.load(err));
    CHECK_EQ(again.installed().size(), size_t(1));
    CHECK_EQ(again.find("org.t.a")->publisherId, std::string("org.t"));
    CHECK_EQ(again.history().size(), size_t(200));       // bounded
    CHECK_EQ(again.history().back().detail, std::string("249"));
}

TEST(library_missing_file_is_a_clean_start) {
    TempDir d;
    Library lib(d / "none.json");
    std::string err;
    CHECK(lib.load(err));
    CHECK(lib.installed().empty());
}

TEST(library_corrupt_file_is_quarantined_not_destroyed) {
    TempDir d;
    writeFile(d / "lib.json", "{ this is not json");
    Library lib(d / "lib.json");
    std::string err;
    CHECK(!lib.load(err));
    CHECK_CONTAINS(err, "lib.json.corrupt");
    CHECK(lib.installed().empty());
    CHECK(fs::exists(d / "lib.json.corrupt"));
    CHECK_EQ(testing::readAll(d / "lib.json.corrupt"), std::string("{ this is not json"));
}

TEST(library_upsert_remove_find) {
    TempDir d;
    Library lib(d / "lib.json");
    lib.upsert({"org.t.a", "A", "1.0.0", "", "", "", "", ""});
    lib.upsert({"org.t.a", "A", "2.0.0", "", "", "", "", ""});
    CHECK_EQ(lib.installed().size(), size_t(1));
    CHECK_EQ(lib.find("org.t.a")->version, std::string("2.0.0"));
    CHECK(lib.remove("org.t.a"));
    CHECK(!lib.remove("org.t.a"));
    CHECK(lib.find("org.t.a") == nullptr);
}
