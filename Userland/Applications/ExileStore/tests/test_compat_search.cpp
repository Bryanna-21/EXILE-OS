/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include "../src/core/appstate.h"
#include "../src/core/device.h"
#include "../src/core/search.h"
#include "testing.h"

using namespace exile;
using testing::Fixture;
using testing::PkgSpec;

namespace {

DeviceProfile device(DeviceClass d, Arch a, char const* os = "1.0.0")
{
    DeviceProfile p;
    p.deviceClass = d;
    p.arch = a;
    p.osVersion = os;
    return p;
}

Compatibility check(Fixture& f, PkgSpec s, DeviceProfile const& dev)
{
    Catalog c = f.catalog({ f.build(s).json });
    return resolveCompatibility(*c.find(s.appId), dev);
}

} // namespace

TEST(compat_matches_device_and_architecture)
{
    Fixture f;
    PkgSpec s;
    s.devices = "\"desktop\"";
    s.archs = "\"x86_64\"";
    CHECK(check(f, s, device(DeviceClass::Desktop, Arch::X86_64)).ok());
    auto wrongDevice = check(f, s, device(DeviceClass::Mobile, Arch::X86_64));
    CHECK_EQ(wrongDevice.status, CompatStatus::WrongDevice);
    CHECK(wrongDevice.build == nullptr);
    CHECK_CONTAINS(wrongDevice.reason, "Desktop");
    CHECK_CONTAINS(wrongDevice.reason, "Mobile");
    auto wrongArch = check(f, s, device(DeviceClass::Desktop, Arch::Aarch64));
    CHECK_EQ(wrongArch.status, CompatStatus::WrongArchitecture);
    CHECK_CONTAINS(wrongArch.reason, "x86_64");
}

TEST(compat_across_all_device_classes)
{
    Fixture f;
    PkgSpec s;
    s.devices = "\"mobile\",\"tablet\"";
    s.archs = "\"aarch64\"";
    Catalog c = f.catalog({ f.build(s).json });
    AppEntry const& app = *c.find(s.appId);
    CHECK(resolveCompatibility(app, device(DeviceClass::Mobile, Arch::Aarch64)).ok());
    CHECK(resolveCompatibility(app, device(DeviceClass::Tablet, Arch::Aarch64)).ok());
    CHECK(!resolveCompatibility(app, device(DeviceClass::Desktop, Arch::Aarch64)).ok());
    CHECK(!resolveCompatibility(app, device(DeviceClass::Tv, Arch::Aarch64)).ok());
    CHECK(!resolveCompatibility(app, device(DeviceClass::Other, Arch::Aarch64)).ok());
    CHECK(!resolveCompatibility(app, device(DeviceClass::Mobile, Arch::X86_64)).ok());
    CHECK(!resolveCompatibility(app, device(DeviceClass::Mobile, Arch::Riscv64)).ok());
}

TEST(compat_universal_device_build_runs_on_every_device_class)
{
    Fixture f;
    PkgSpec s;
    s.devices = "\"any\"";
    s.archs = "\"any\"";
    Catalog c = f.catalog({ f.build(s).json });
    AppEntry const& app = *c.find(s.appId);
    for (DeviceClass d : { DeviceClass::Desktop, DeviceClass::Mobile, DeviceClass::Tablet, DeviceClass::Tv, DeviceClass::Other })
        CHECK(resolveCompatibility(app, device(d, Arch::Riscv64)).ok());
    // A universal build is still subject to the other rules.
    PkgSpec old = s;
    old.extraPkg = ",\"minimumOsVersion\":\"5.0.0\"";
    Catalog c2 = f.catalog({ f.build(old).json });
    CHECK_EQ(resolveCompatibility(*c2.find(old.appId), device(DeviceClass::Tv, Arch::Aarch64, "1.0.0")).status, CompatStatus::OsTooOld);
    CHECK_EQ(toString(DeviceClass::Any), std::string_view("any"));
}

TEST(compat_any_architecture_matches_everything)
{
    Fixture f;
    PkgSpec s;
    s.archs = "\"any\"";
    CHECK(check(f, s, device(DeviceClass::Desktop, Arch::Riscv64)).ok());
    CHECK(check(f, s, device(DeviceClass::Desktop, Arch::Aarch64)).ok());
}

TEST(compat_enforces_minimum_os_version)
{
    Fixture f;
    PkgSpec s;
    s.extraPkg = ",\"minimumOsVersion\":\"2.0.0\"";
    auto old = check(f, s, device(DeviceClass::Desktop, Arch::X86_64, "1.9.9"));
    CHECK_EQ(old.status, CompatStatus::OsTooOld);
    CHECK_CONTAINS(old.reason, "2.0.0");
    CHECK_CONTAINS(old.reason, "1.9.9");
    CHECK(check(f, s, device(DeviceClass::Desktop, Arch::X86_64, "2.0.0")).ok());
    CHECK(check(f, s, device(DeviceClass::Desktop, Arch::X86_64, "2.1.0")).ok());
    CHECK(!check(f, s, device(DeviceClass::Desktop, Arch::X86_64, "2.0.0-rc.1")).ok()); // prerelease is older
}

TEST(compat_hides_nonstable_channels_unless_enabled)
{
    Fixture f;
    PkgSpec s;
    s.extraPkg = ",\"channel\":\"beta\"";
    Catalog c = f.catalog({ f.build(s).json });
    DeviceProfile d = device(DeviceClass::Desktop, Arch::X86_64);
    CHECK_EQ(resolveCompatibility(*c.find(s.appId), d).status, CompatStatus::ChannelNotEnabled);
    CHECK(!isListedFor(*c.find(s.appId), d));
    d.enabledChannels.push_back(ReleaseChannel::Beta);
    CHECK(resolveCompatibility(*c.find(s.appId), d).ok());
    CHECK(isListedFor(*c.find(s.appId), d));
}

TEST(compat_refuses_withdrawn_release_with_reason)
{
    Fixture f;
    PkgSpec s;
    s.extraPkg = ",\"withdrawn\":true,\"withdrawnReason\":\"data loss bug\"";
    auto c = check(f, s, device(DeviceClass::Desktop, Arch::X86_64));
    CHECK_EQ(c.status, CompatStatus::Withdrawn);
    CHECK_CONTAINS(c.reason, "data loss bug");
}

TEST(compat_prefers_native_over_web_and_exact_arch_over_any)
{
    Fixture f;
    PkgSpec s;
    auto built = f.build(s); // real package file; we only reuse its digest/size/url for the extra builds
    auto pkg = [&](std::string const& id, std::string const& kind, std::string const& archs) {
        return "{\"id\":\"" + id + "\",\"kind\":\"" + kind + "\",\"devices\":[\"desktop\"],\"architectures\":[" + archs + "],\"url\":\"pkgs/" + s.appId + "-" + s.version + ".expkg\",\"sha256\":\"" + built.sha256 + "\"}";
    };
    std::string app = "{\"appId\":\"" + s.appId + "\",\"name\":\"N\",\"summary\":\"s\",\"publisher\":{\"id\":\"org.test\",\"name\":\"P\"},"
                                                  "\"categories\":[\"utilities\"],\"currentVersion\":\"1.0.0\",\"packages\":["
        + pkg("web", "web", "\"any\"") + "," + pkg("compat", "compat-layer", "\"x86_64\"") + "," + pkg("anyarch", "native", "\"any\"") + "," + pkg("exact", "native", "\"x86_64\"") + "]}";
    Catalog c = f.catalog({ app });
    Compatibility r = resolveCompatibility(*c.find(s.appId), device(DeviceClass::Desktop, Arch::X86_64));
    CHECK(r.ok());
    CHECK_EQ(r.build->id, std::string("exact")); // native + exact architecture wins
    // On another CPU the exact build no longer applies; native "any" still beats compat layer and web.
    Compatibility r2 = resolveCompatibility(*c.find(s.appId), device(DeviceClass::Desktop, Arch::Aarch64));
    CHECK_EQ(r2.build->id, std::string("anyarch"));

    // Remove the native builds: compatibility layer beats web.
    std::string noNative = "{\"appId\":\"" + s.appId + "\",\"name\":\"N\",\"summary\":\"s\",\"publisher\":{\"id\":\"org.test\",\"name\":\"P\"},"
                                                       "\"categories\":[\"utilities\"],\"currentVersion\":\"1.0.0\",\"packages\":["
        + pkg("web", "web", "\"any\"") + "," + pkg("compat", "compat-layer", "\"x86_64\"") + "]}";
    Catalog c2 = f.catalog({ noNative });
    CHECK_EQ(resolveCompatibility(*c2.find(s.appId), device(DeviceClass::Desktop, Arch::X86_64)).build->id, std::string("compat"));
    CHECK_EQ(resolveCompatibility(*c2.find(s.appId), device(DeviceClass::Desktop, Arch::Aarch64)).build->id, std::string("web"));
}

TEST(compat_reports_the_most_relevant_reason_across_builds)
{
    Fixture f;
    PkgSpec s;
    s.devices = "\"desktop\"";
    s.archs = "\"aarch64\"";
    // Right device, wrong CPU: the user should hear about the CPU, not just "wrong device".
    auto r = check(f, s, device(DeviceClass::Desktop, Arch::X86_64));
    CHECK_EQ(r.status, CompatStatus::WrongArchitecture);
}

TEST(status_primary_action_for_each_state)
{
    Fixture f;
    PkgSpec s;
    Catalog c = f.catalog({ f.build(s).json });
    AppEntry const& app = *c.find(s.appId);
    Library lib(f.dir / "lib.json");
    DeviceProfile d = device(DeviceClass::Desktop, Arch::X86_64);

    AppStatus st = computeStatus(app, lib, d, false);
    CHECK_EQ(st.action, PrimaryAction::Install);
    CHECK_EQ(st.label, std::string("Install"));

    lib.upsert({ s.appId, s.name, "1.0.0", "main", "t", "p", "k", "h" });
    st = computeStatus(app, lib, d, false);
    CHECK_EQ(st.action, PrimaryAction::Installed); // cannot launch => not a fake "Open" button
    st = computeStatus(app, lib, d, true);
    CHECK_EQ(st.action, PrimaryAction::Open);

    lib.upsert({ s.appId, s.name, "0.9.0", "main", "t", "p", "k", "h" });
    st = computeStatus(app, lib, d, true);
    CHECK_EQ(st.action, PrimaryAction::Update);
    CHECK_EQ(st.label, std::string("Update"));

    Library empty(f.dir / "lib2.json");
    st = computeStatus(app, empty, device(DeviceClass::Mobile, Arch::X86_64), true);
    CHECK_EQ(st.action, PrimaryAction::Unavailable);
    CHECK_EQ(st.label, std::string("Not available for this device"));
    CHECK(!st.note.empty());
}

TEST(find_updates_only_reports_newer_compatible_versions)
{
    Fixture f;
    PkgSpec a;
    a.appId = "org.t.a";
    a.version = "2.0.0";
    PkgSpec b;
    b.appId = "org.t.b";
    b.version = "1.0.0";
    PkgSpec c2;
    c2.appId = "org.t.c";
    c2.version = "3.0.0";
    c2.devices = "\"mobile\"";
    Catalog cat = f.catalog({ f.build(a).json, f.build(b).json, f.build(c2).json });
    Library lib(f.dir / "lib.json");
    lib.upsert({ "org.t.a", "A", "1.0.0", "main", "", "", "", "" });
    lib.upsert({ "org.t.b", "B", "1.0.0", "main", "", "", "", "" });
    lib.upsert({ "org.t.c", "C", "1.0.0", "main", "", "", "", "" });       // newer exists but not for this device
    lib.upsert({ "org.t.gone", "Gone", "1.0.0", "main", "", "", "", "" }); // no longer in catalog
    auto updates = findUpdates(cat, lib, device(DeviceClass::Desktop, Arch::X86_64));
    CHECK_EQ(updates.size(), size_t(1));
    CHECK_EQ(updates[0].app->appId, std::string("org.t.a"));
    CHECK_EQ(updates[0].newVersion, std::string("2.0.0"));
}

// ---------------- search ----------------

namespace {
Catalog searchCatalogFixture(Fixture& f)
{
    std::vector<std::string> apps;
    auto add = [&](std::string const& id, std::string const& name, std::string const& summary, std::string const& cat,
                   std::string const& extraApp = "", std::string const& devices = "\"desktop\"") {
        PkgSpec s;
        s.appId = id;
        s.name = name;
        s.summary = summary;
        s.category = cat;
        s.extraApp = extraApp;
        s.devices = devices;
        apps.push_back(f.build(s).json);
    };
    add("org.t.notes", "Quick Notes", "Write short notes", "utilities", ",\"keywords\":[\"memo\",\"text\"],\"updatedAt\":\"2026-03-01\"");
    add("org.t.notepad", "Notepad Plus", "A text editor", "utilities", ",\"updatedAt\":\"2026-05-01\"");
    add("org.t.racer", "Space Racer", "Fast racing game", "games", ",\"updatedAt\":\"2026-04-01\"");
    add("org.t.phone", "Phone Dialer", "Make calls", "utilities", ",\"updatedAt\":\"2026-01-01\"", "\"mobile\"");
    add("org.t.beta", "Hidden Beta", "unstable", "games", "");
    return f.catalog(apps);
}
} // namespace

TEST(search_matches_name_keyword_summary_and_publisher)
{
    Fixture f;
    Catalog c = searchCatalogFixture(f);
    DeviceProfile d = device(DeviceClass::Desktop, Arch::X86_64);
    auto names = [&](std::string const& q) {
        SearchQuery sq;
        sq.text = q;
        std::vector<std::string> out;
        for (auto& h : searchCatalog(c, sq, d))
            out.push_back(h.app->appId);
        return out;
    };
    CHECK_EQ(names("racer").size(), size_t(1));
    CHECK_EQ(names("RACER")[0], std::string("org.t.racer"));    // case-insensitive
    CHECK_EQ(names("memo")[0], std::string("org.t.notes"));     // keyword
    CHECK_EQ(names("editor")[0], std::string("org.t.notepad")); // summary
    CHECK_EQ(names("test publisher").size(), size_t(5));        // publisher name, both words
    CHECK_EQ(names("games").size(), size_t(2));                 // category name
    CHECK(names("zzzzzz").empty());
}

TEST(search_requires_every_word_to_match)
{
    Fixture f;
    Catalog c = searchCatalogFixture(f);
    DeviceProfile d = device(DeviceClass::Desktop, Arch::X86_64);
    SearchQuery q;
    q.text = "quick racer";
    CHECK(searchCatalog(c, q, d).empty());
    q.text = "quick notes";
    CHECK_EQ(searchCatalog(c, q, d).size(), size_t(1));
}

TEST(search_ranks_name_prefix_above_summary_match)
{
    Fixture f;
    Catalog c = searchCatalogFixture(f);
    DeviceProfile d = device(DeviceClass::Desktop, Arch::X86_64);
    SearchQuery q;
    q.text = "note";
    auto hits = searchCatalog(c, q, d);
    CHECK(hits.size() >= 2);
    CHECK(hits[0].score >= hits[1].score);
    CHECK(hits[0].app->name.find("Note") != std::string::npos);
}

TEST(search_category_filter_and_compatibility_filter)
{
    Fixture f;
    Catalog c = searchCatalogFixture(f);
    DeviceProfile d = device(DeviceClass::Desktop, Arch::X86_64);
    SearchQuery q;
    q.categoryId = "utilities";
    CHECK_EQ(searchCatalog(c, q, d).size(), size_t(3)); // browsing includes the phone-only app...
    q.onlyCompatible = true;
    CHECK_EQ(searchCatalog(c, q, d).size(), size_t(2)); // ...unless the filter is on
    q.categoryId = "games";
    q.onlyCompatible = false;
    CHECK_EQ(searchCatalog(c, q, d).size(), size_t(2));
    q.categoryId = "no-such-category";
    CHECK(searchCatalog(c, q, d).empty());
}

TEST(search_incompatible_apps_are_listed_but_marked)
{
    Fixture f;
    Catalog c = searchCatalogFixture(f);
    SearchQuery q;
    q.text = "phone";
    auto hits = searchCatalog(c, q, device(DeviceClass::Desktop, Arch::X86_64));
    CHECK_EQ(hits.size(), size_t(1));
    CHECK(!hits[0].compat.ok());
    CHECK(!hits[0].compat.reason.empty());
    auto onMobile = searchCatalog(c, q, device(DeviceClass::Mobile, Arch::X86_64));
    CHECK(onMobile[0].compat.status != CompatStatus::WrongDevice);
}

TEST(search_sorting_orders)
{
    Fixture f;
    Catalog c = searchCatalogFixture(f);
    DeviceProfile d = device(DeviceClass::Desktop, Arch::X86_64);
    SearchQuery q;
    q.categoryId = "utilities";
    q.sort = SortOrder::Name;
    auto byName = searchCatalog(c, q, d);
    CHECK_EQ(byName[0].app->name, std::string("Notepad Plus"));
    q.sort = SortOrder::RecentlyUpdated;
    auto byDate = searchCatalog(c, q, d);
    CHECK_EQ(byDate[0].app->appId, std::string("org.t.notepad"));   // 2026-05-01
    CHECK_EQ(byDate.back().app->appId, std::string("org.t.phone")); // 2026-01-01
}

TEST(search_empty_query_lists_everything_listed)
{
    Fixture f;
    Catalog c = searchCatalogFixture(f);
    SearchQuery q;
    CHECK_EQ(searchCatalog(c, q, device(DeviceClass::Desktop, Arch::X86_64)).size(), size_t(5));
}

TEST(search_tokenizer)
{
    CHECK_EQ(tokenize("  Hello,   World-42! ").size(), size_t(3));
    CHECK_EQ(tokenize("Hello")[0], std::string("hello"));
    CHECK(tokenize("!!!  ").empty());
    CHECK_EQ(tokenize("caf\xC3\xA9").size(), size_t(1));
}
