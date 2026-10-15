#include "core/catalog.h"
#include "core/catalogclient.h"
#include "testing.h"

using namespace exile;
using testing::Fixture;
using testing::PkgSpec;

namespace {

const char* kSha = "0000000000000000000000000000000000000000000000000000000000000000";

// A hand-written minimal valid app, with a hook to break individual fields.
std::string appJson(const std::string& id = "org.t.one", const std::string& pkgExtra = "", const std::string& appExtra = "") {
    return std::string("{\"appId\":\"") + id + "\",\"name\":\"One\",\"summary\":\"s\",\"publisher\":{\"id\":\"org.t\",\"name\":\"T\"},"
           "\"categories\":[\"utilities\"],\"currentVersion\":\"1.0.0\"" + appExtra +
           ",\"packages\":[{\"id\":\"main\",\"devices\":[\"desktop\"],\"architectures\":[\"x86_64\"],\"url\":\"p/one.expkg\",\"sha256\":\"" +
           kSha + "\"" + pkgExtra + "}]}";
}

CatalogLoadResult load(const std::vector<std::string>& apps) { return parseCatalog(Fixture::catalogJson(apps), "/base"); }

bool hasIssue(const CatalogLoadResult& r, const std::string& needle) {
    for (const auto& i : r.issues)
        if (i.message.find(needle) != std::string::npos) return true;
    return false;
}

}  // namespace

TEST(catalog_parses_valid_document) {
    auto r = load({appJson("org.t.one", ",\"downloadSize\":100,\"packageSize\":300,\"minimumOsVersion\":\"1.2.0\"",
                           ",\"rating\":{\"average\":4.5,\"count\":12},\"featured\":true,\"permissions\":[{\"id\":\"network\",\"reason\":\"sync\"}]")});
    CHECK(r.ok());
    CHECK(r.issues.empty());
    const AppEntry* a = r.catalog->find("org.t.one");
    CHECK(a != nullptr);
    CHECK_EQ(a->name, std::string("One"));
    CHECK(a->featured);
    CHECK(a->rating.has_value());
    CHECK_EQ(a->rating->count, uint64_t(12));
    CHECK_EQ(a->packages[0].downloadSize, uint64_t(100));
    CHECK_EQ(a->packages[0].minimumOsVersion, std::string("1.2.0"));
    CHECK_EQ(a->permissions[0].id, std::string("network"));
    CHECK_EQ(r.catalog->baseDir, std::string("/base"));
}

TEST(catalog_without_rating_has_no_rating) {
    auto r = load({appJson()});
    CHECK(!r.catalog->find("org.t.one")->rating.has_value());
}

TEST(catalog_malformed_json_gives_no_catalog) {
    auto r = parseCatalog("{ not json", "/b");
    CHECK(!r.ok());
    CHECK(!r.issues.empty());
    CHECK(!parseCatalog("[]", "/b").ok());
    CHECK(!parseCatalog("", "/b").ok());
}

TEST(catalog_rejects_wrong_or_missing_schema_version) {
    CHECK(!parseCatalog(R"({"schemaVersion":2,"categories":[],"apps":[]})", "/b").ok());
    CHECK(!parseCatalog(R"({"categories":[],"apps":[]})", "/b").ok());
    CHECK(!parseCatalog(R"({"schemaVersion":"1","categories":[],"apps":[]})", "/b").ok());
    CHECK(!parseCatalog(R"({"schemaVersion":1,"categories":[]})", "/b").ok());
    CHECK(!parseCatalog(R"({"schemaVersion":1,"apps":[]})", "/b").ok());
}

TEST(catalog_skips_bad_app_but_keeps_good_ones) {
    auto r = load({appJson("org.t.good"), "{\"appId\":\"org.t.bad\"}", appJson("org.t.good2"), "42"});
    CHECK(r.ok());
    CHECK_EQ(r.catalog->apps.size(), size_t(2));
    CHECK(r.catalog->find("org.t.good") != nullptr);
    CHECK(r.catalog->find("org.t.good2") != nullptr);
    CHECK(!r.issues.empty());
}

TEST(catalog_ignores_duplicate_app_ids) {
    auto r = load({appJson("org.t.dup"), appJson("org.t.dup")});
    CHECK_EQ(r.catalog->apps.size(), size_t(1));
    CHECK(hasIssue(r, "duplicate appId"));
}

TEST(catalog_rejects_invalid_identifiers) {
    for (const char* id : {"noDots", "Upper.Case", "a..b", ".a.b", "a.b.", "a.b c", "a.b/c", "a._b", "../x.y"}) {
        auto r = load({appJson(id)});
        if (!r.catalog->apps.empty()) throw testing::Failure{std::string("accepted app id: ") + id};
    }
    CHECK(isValidAppId("org.exile.notes"));
    CHECK(isValidAppId("a.b"));
    CHECK(!isValidAppId(""));
}

TEST(catalog_rejects_unknown_category_and_missing_fields) {
    auto bad = appJson();
    size_t pos = bad.find("\"utilities\"");
    bad.replace(pos, 11, "\"nonexistent\"");
    CHECK(load({bad}).catalog->apps.empty());
    CHECK(load({"{\"appId\":\"org.t.x\",\"name\":\"x\"}"}).catalog->apps.empty());
}

TEST(catalog_rejects_unsafe_package_urls) {
    for (const char* url : {"http://example.com/a.expkg", "ftp://x/a", "javascript:alert(1)", "data:text/plain,hi", "../escape.expkg",
                            "/abs/path.expkg", "a/../../b.expkg", "https://user:pw@host/a", "https:///nohost", "file://relative",
                            "file:///a/../b", "a b.expkg"}) {
        std::string pkg = std::string(",\"url\":\"") + url + "\"";
        std::string json = appJson();
        size_t p = json.find("\"url\":\"p/one.expkg\"");
        json.replace(p, std::string("\"url\":\"p/one.expkg\"").size(), pkg.substr(1));
        auto r = load({json});
        if (!r.catalog->apps.empty()) throw testing::Failure{std::string("accepted unsafe url: ") + url};
    }
}

TEST(source_url_validator_accepts_expected_forms) {
    CHECK(isAcceptableSourceUrl("https://store.exile.example/pkg/a.expkg"));
    CHECK(isAcceptableSourceUrl("packages/a.expkg"));
    CHECK(isAcceptableSourceUrl("file:///opt/pkgs/a.expkg"));
    std::string why;
    CHECK(!isAcceptableSourceUrl("http://x/y", &why));
    CHECK(!why.empty());
}

TEST(catalog_rejects_bad_digest_version_and_numbers) {
    auto badSha = appJson();
    badSha.replace(badSha.find(kSha), 64, "XYZ");
    CHECK(load({badSha}).catalog->apps.empty());
    auto upper = appJson();
    upper.replace(upper.find(kSha), 64, std::string(64, 'A'));
    CHECK(load({upper}).catalog->apps.size() == 1);  // uppercase hex is normalised, not rejected
    CHECK(load({appJson("org.t.x", ",\"downloadSize\":-5")}).catalog->apps.empty());
    CHECK(load({appJson("org.t.x", ",\"downloadSize\":1.5")}).catalog->apps.empty());
    CHECK(load({appJson("org.t.x", ",\"minimumOsVersion\":\"banana\"")}).catalog->apps.empty());
    CHECK(load({appJson("org.t.x", ",\"channel\":\"weekly\"")}).catalog->apps.empty());
    CHECK(load({appJson("org.t.x", ",\"kind\":\"magic\"")}).catalog->apps.empty());
}

TEST(catalog_rejects_fake_or_inconsistent_ratings) {
    CHECK(load({appJson("org.t.a", "", ",\"rating\":{\"average\":9,\"count\":3}")}).catalog->apps.empty());
    CHECK(load({appJson("org.t.b", "", ",\"rating\":{\"average\":4,\"count\":0}")}).catalog->apps.empty());
    CHECK(load({appJson("org.t.c", "", ",\"rating\":{\"average\":4}")}).catalog->apps.empty());
}

TEST(catalog_rejects_control_characters_and_oversized_text) {
    std::string withCtl = appJson();
    withCtl.replace(withCtl.find("\"One\""), 5, "\"On\\u0001e\"");
    CHECK(load({withCtl}).catalog->apps.empty());
    std::string longName = appJson();
    longName.replace(longName.find("\"One\""), 5, "\"" + std::string(500, 'x') + "\"");
    CHECK(load({longName}).catalog->apps.empty());
}

TEST(catalog_requires_packages_with_devices_and_architectures) {
    auto noPkgs = appJson();
    noPkgs = noPkgs.substr(0, noPkgs.find(",\"packages\"")) + ",\"packages\":[]}";
    CHECK(load({noPkgs}).catalog->apps.empty());
    auto noArch = appJson();
    noArch.replace(noArch.find("[\"x86_64\"]"), 10, "[]");
    CHECK(load({noArch}).catalog->apps.empty());
    auto badDev = appJson();
    badDev.replace(badDev.find("\"desktop\""), 9, "\"toaster\"");
    CHECK(load({badDev}).catalog->apps.empty());
}

TEST(catalog_derived_views_merge_builds) {
    Fixture f;
    PkgSpec s;
    s.devices = "\"desktop\",\"mobile\"";
    s.archs = "\"x86_64\",\"aarch64\"";
    Catalog c = f.catalog({f.build(s).json});
    const AppEntry* a = c.find(s.appId);
    CHECK_EQ(a->supportedDevices().size(), size_t(2));
    CHECK_EQ(a->supportedArchitectures().size(), size_t(2));
    CHECK_EQ(a->minimumOsVersion(), std::string());
}

// ---- catalog client: offline behaviour ----

namespace {
class FakeSource final : public ICatalogSource {
public:
    std::string text;
    bool reachable = true;
    bool fetch(std::string& out, std::string& error) override {
        if (!reachable) { error = "network unreachable"; return false; }
        out = text;
        return true;
    }
    std::filesystem::path baseDir() const override { return "/base"; }
    std::string describe() const override { return "fake"; }
};
}  // namespace

TEST(catalog_client_online_loads_and_caches) {
    testing::TempDir d;
    FakeSource src;
    src.text = Fixture::catalogJson({appJson()});
    CatalogState st = loadCatalog(src, d / "cache.json");
    CHECK(st.catalog.has_value());
    CHECK(!st.fromCache);
    CHECK(st.notice.empty());
    CHECK(std::filesystem::exists(d / "cache.json"));
}

TEST(catalog_client_offline_falls_back_to_cache_with_notice) {
    testing::TempDir d;
    FakeSource src;
    src.text = Fixture::catalogJson({appJson()});
    loadCatalog(src, d / "cache.json");
    src.reachable = false;
    CatalogState st = loadCatalog(src, d / "cache.json");
    CHECK(st.catalog.has_value());
    CHECK(st.fromCache);
    CHECK_CONTAINS(st.notice, "saved catalog data");
    CHECK_CONTAINS(st.notice, "network unreachable");
    CHECK(st.catalog->find("org.t.one") != nullptr);
}

TEST(catalog_client_offline_without_cache_explains_and_has_no_catalog) {
    testing::TempDir d;
    FakeSource src;
    src.reachable = false;
    CatalogState st = loadCatalog(src, d / "cache.json");
    CHECK(!st.catalog.has_value());
    CHECK_CONTAINS(st.notice, "could not be loaded");
    CHECK_CONTAINS(st.notice, "Library");
}

TEST(catalog_client_invalid_fresh_catalog_never_overwrites_good_cache) {
    testing::TempDir d;
    FakeSource src;
    src.text = Fixture::catalogJson({appJson()});
    loadCatalog(src, d / "cache.json");
    std::string good = testing::readAll(d / "cache.json");
    src.text = "{ corrupt";
    CatalogState st = loadCatalog(src, d / "cache.json");
    CHECK(st.fromCache);
    CHECK(st.catalog.has_value());
    CHECK_EQ(testing::readAll(d / "cache.json"), good);
}

TEST(catalog_client_corrupt_cache_and_dead_source_gives_clear_failure) {
    testing::TempDir d;
    testing::writeFile(d / "cache.json", "garbage");
    FakeSource src;
    src.reachable = false;
    CatalogState st = loadCatalog(src, d / "cache.json");
    CHECK(!st.catalog.has_value());
    CHECK(!st.notice.empty());
}

TEST(file_catalog_source_reads_and_reports_missing_file) {
    testing::TempDir d;
    testing::writeFile(d / "c.json", Fixture::catalogJson({appJson()}));
    auto r = loadCatalogFile(d / "c.json");
    CHECK(r.ok());
    CHECK_EQ(r.catalog->baseDir, d.path().string());
    CHECK(!loadCatalogFile(d / "nope.json").ok());
}
