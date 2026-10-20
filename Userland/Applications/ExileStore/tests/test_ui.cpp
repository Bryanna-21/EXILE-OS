/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include <sstream>

#include "../src/ui/controller.h"
#include "../src/ui/render.h"
#include "../src/ui/text.h"
#include "testing.h"

using namespace exile;
using namespace exile::ui;
using testing::Fixture;
using testing::PkgSpec;

namespace {

struct View {
    Fixture f;
    Catalog catalog;
    Library lib { f.dir / "lib.json" };
    ViewContext ctx;
    explicit View(std::vector<PkgSpec> const& specs, bool canLaunch = false)
    {
        std::vector<std::string> apps;
        for (auto const& s : specs)
            apps.push_back(f.build(s).json);
        catalog = f.catalog(apps);
        ctx.catalog = &catalog;
        ctx.library = &lib;
        ctx.device = &f.device;
        ctx.trust = &f.trust;
        ctx.verifier = f.verifier.get();
        ctx.backendCanLaunch = canLaunch;
    }
    std::string page(Page const& p)
    {
        std::string out;
        for (auto const& l : p.lines)
            out += l + "\n";
        return out;
    }
};

RenderOptions opts(size_t width = 80)
{
    RenderOptions o;
    o.width = width;
    return o;
}

PkgSpec spec(std::string const& id, std::string const& name, std::string const& devices = "\"desktop\"")
{
    PkgSpec s;
    s.appId = id;
    s.name = name;
    s.devices = devices;
    return s;
}

} // namespace

// ---------------- text helpers ----------------

TEST(text_truncate_respects_utf8_and_width)
{
    CHECK_EQ(exile::ui::truncate("short", 10), std::string("short"));
    CHECK_EQ(exile::ui::truncate("abcdefghij", 8), std::string("abcde..."));
    std::string t = exile::ui::truncate("caf\xC3\xA9 au lait tr\xC3\xA8s long", 10);
    CHECK_EQ(displayWidth(t), size_t(10));
    CHECK(exile::util::hasControlChars(t) == false);
    CHECK_EQ(exile::ui::truncate("abc", 2), std::string(".."));
}

TEST(text_display_width_ignores_ansi_sequences)
{
    Theme color(true);
    CHECK_EQ(displayWidth(color.green("hello")), size_t(5));
    CHECK_EQ(displayWidth("plain"), size_t(5));
    Theme none(false);
    CHECK_EQ(none.green("x"), std::string("x")); // no escape codes when color is off
}

TEST(text_wrap_never_exceeds_width_and_keeps_words)
{
    auto lines = wrapText("the quick brown fox jumps over the lazy dog again and again", 20);
    for (auto const& l : lines)
        CHECK(displayWidth(l) <= 20);
    CHECK(lines.size() >= 3);
    auto longWord = wrapText(std::string(100, 'x'), 20);
    for (auto const& l : longWord)
        CHECK(displayWidth(l) <= 20);
    auto paras = wrapText("one\n\ntwo", 20);
    CHECK_EQ(paras.size(), size_t(3));
}

TEST(text_progress_bar_is_bounded)
{
    CHECK_EQ(progressBar(0.0, 10), std::string("[--------]"));
    CHECK_EQ(progressBar(1.0, 10), std::string("[########]"));
    CHECK_EQ(progressBar(5.0, 10), std::string("[########]"));
    CHECK_EQ(progressBar(-1.0, 10), std::string("[--------]"));
}

// ---------------- pages ----------------

TEST(card_for_incompatible_app_is_never_shown_as_installable)
{
    View v({ spec("org.t.ok", "Works App"), spec("org.t.phone", "Phone Only", "\"mobile\"") });
    Page home = renderSearch(v.ctx, opts(), SearchQuery {});
    SearchQuery q;
    q.text = "phone";
    std::string out = v.page(renderSearch(v.ctx, opts(), q));
    CHECK_CONTAINS(out, "Phone Only");
    CHECK_CONTAINS(out, "[Not for this device]");
    CHECK_NOT_CONTAINS(out, "[Install]");
    q.text = "works";
    CHECK_CONTAINS(v.page(renderSearch(v.ctx, opts(), q)), "[Install]");
    (void)home;
}

TEST(details_page_explains_incompatibility_and_has_no_install_action)
{
    View v({ spec("org.t.phone", "Phone Only", "\"mobile\"") });
    std::string out = v.page(renderDetails(v.ctx, opts(), "org.t.phone"));
    CHECK_CONTAINS(out, "[ Not available for this device ]");
    CHECK_CONTAINS(out, "built for Mobile");
    CHECK_CONTAINS(out, "this is a Desktop device");
    CHECK_NOT_CONTAINS(out, "[ Install ]");
}

TEST(details_primary_action_matches_state)
{
    View v({ spec("org.t.app", "App") }, /*canLaunch=*/true);
    CHECK_CONTAINS(v.page(renderDetails(v.ctx, opts(), "org.t.app")), "[ Install ]");
    v.lib.upsert({ "org.t.app", "App", "0.5.0", "main", "", "", "", "" });
    std::string upd = v.page(renderDetails(v.ctx, opts(), "org.t.app"));
    CHECK_CONTAINS(upd, "[ Update ]");
    CHECK_NOT_CONTAINS(upd, "[ Install ]");
    v.lib.upsert({ "org.t.app", "App", "1.0.0", "main", "", "", "", "" });
    std::string open = v.page(renderDetails(v.ctx, opts(), "org.t.app"));
    CHECK_CONTAINS(open, "[ Open ]");
    CHECK_CONTAINS(open, "Uninstall");
}

TEST(details_does_not_offer_open_when_the_system_cannot_launch)
{
    View v({ spec("org.t.app", "App") }, /*canLaunch=*/false);
    v.lib.upsert({ "org.t.app", "App", "1.0.0", "main", "", "", "", "" });
    std::string out = v.page(renderDetails(v.ctx, opts(), "org.t.app"));
    CHECK_NOT_CONTAINS(out, "[ Open ]");
    CHECK_CONTAINS(out, "[ Installed ]");
}

TEST(details_shows_required_information_sections)
{
    PkgSpec s = spec("org.t.full", "Full App");
    s.extraApp = ",\"description\":\"Long text about the app.\",\"releaseNotes\":\"Fixed things.\",\"license\":\"MIT\","
                 "\"privacyPolicy\":\"https://example.com/privacy\",\"sourceRepository\":\"https://example.com/src\","
                 "\"permissions\":[{\"id\":\"camera\",\"reason\":\"Scans documents.\"},{\"id\":\"zzz-unknown\"}],"
                 "\"screenshots\":[\"shots/a.png\"]";
    s.extraPkg = ",\"minimumOsVersion\":\"0.5.0\"";
    View v({ s });
    std::string out = v.page(renderDetails(v.ctx, opts(), "org.t.full"));
    for (char const* want : { "Full App", "Test Publisher", "Version 1.0.0", "Long text about the app.", "What's new in 1.0.0", "Fixed things.",
             "Desktop", "x86_64", "0.5.0 or newer", "Download", "Installed size", "Use the camera",
             "Publisher says: Scans documents.", "not recognised", "License: MIT", "Privacy policy: https://example.com/privacy",
             "Source code: https://example.com/src", "1 screenshot(s)", "Signature: valid" })
        CHECK_CONTAINS(out, want);
}

TEST(details_states_that_a_signature_is_not_a_safety_guarantee)
{
    View v({ spec("org.t.app", "App") });
    CHECK_CONTAINS(v.page(renderDetails(v.ctx, opts(), "org.t.app")), "not that the app is safe");
}

TEST(details_blocks_and_explains_untrusted_signature)
{
    PkgSpec s = spec("org.t.app", "App");
    s.signWithUntrustedKey = true;
    View v({ s });
    std::string out = v.page(renderDetails(v.ctx, opts(), "org.t.app"));
    CHECK_CONTAINS(out, "Signature: not verified");
    CHECK_CONTAINS(out, "Installation is blocked");
}

TEST(ratings_appear_only_when_genuine_data_exists)
{
    PkgSpec with = spec("org.t.rated", "Rated App");
    with.extraApp = ",\"rating\":{\"average\":4.5,\"count\":120}";
    View v({ with, spec("org.t.plain", "Plain App") });
    SearchQuery q;
    q.text = "rated";
    CHECK_CONTAINS(v.page(renderSearch(v.ctx, opts(), q)), "4.5/5 (120 ratings)");
    q.text = "plain";
    std::string out = v.page(renderSearch(v.ctx, opts(), q));
    CHECK_NOT_CONTAINS(out, "/5");
    CHECK_NOT_CONTAINS(out, "ratings");
    std::string details = v.page(renderDetails(v.ctx, opts(), "org.t.plain"));
    CHECK_NOT_CONTAINS(details, "Rated ");
    // No invented popularity figures anywhere.
    CHECK_NOT_CONTAINS(v.page(renderHome(v.ctx, opts())), "downloads");
}

TEST(permissions_are_explained_in_plain_language)
{
    CHECK_EQ(describePermission("camera"), std::string("Use the camera"));
    CHECK_CONTAINS(describePermission("system-settings"), "sensitive");
    CHECK_CONTAINS(describePermission("made-up"), "not recognised");
}

TEST(web_and_compat_builds_are_labelled_honestly)
{
    PkgSpec web = spec("org.t.web", "Web Thing");
    web.extraPkg = ",\"kind\":\"web\"";
    PkgSpec compat = spec("org.t.compat", "Compat Thing");
    compat.extraPkg = ",\"kind\":\"compat-layer\"";
    View v({ web, compat });
    CHECK_CONTAINS(v.page(renderDetails(v.ctx, opts(), "org.t.web")), "Web app");
    CHECK_CONTAINS(v.page(renderDetails(v.ctx, opts(), "org.t.compat")), "compatibility layer");
}

TEST(home_is_useful_and_uncluttered)
{
    PkgSpec feat = spec("org.t.feat", "Featured One");
    feat.extraApp = ",\"featured\":true";
    View v({ feat, spec("org.t.b", "Bee"), spec("org.t.c", "Cee") });
    std::string out = v.page(renderHome(v.ctx, opts()));
    CHECK_CONTAINS(out, "Search:");
    CHECK_CONTAINS(out, "Featured");
    CHECK_CONTAINS(out, "Featured One");
    CHECK_CONTAINS(out, "Categories");
    CHECK_CONTAINS(out, "Your library: 0 installed");
    CHECK_NOT_CONTAINS(out, "Sponsored");
    CHECK_NOT_CONTAINS(out, "Limited time");
}

TEST(home_mentions_available_updates_and_installed_count)
{
    View v({ spec("org.t.app", "App") });
    v.lib.upsert({ "org.t.app", "App", "0.1.0", "main", "", "", "", "" });
    CHECK_CONTAINS(v.page(renderHome(v.ctx, opts())), "1 installed, 1 update available");
}

TEST(explore_lists_only_categories_that_have_apps)
{
    PkgSpec game = spec("org.t.g", "Game");
    game.category = "games";
    View v({ spec("org.t.u", "Util"), game });
    Page p = renderExplore(v.ctx, opts());
    std::string out = v.page(p);
    CHECK_CONTAINS(out, "Utilities");
    CHECK_CONTAINS(out, "Games");
    CHECK_NOT_CONTAINS(out, "System Tools"); // empty category stays hidden
    CHECK_EQ(p.items.size(), size_t(2));
    CHECK(p.kind == ItemKind::Category);
}

TEST(search_page_has_useful_empty_states)
{
    View v({ spec("org.t.app", "App") });
    SearchQuery q;
    CHECK_CONTAINS(v.page(renderSearch(v.ctx, opts(), q)), "Type / followed by");
    q.text = "qqqqzzzz";
    std::string out = v.page(renderSearch(v.ctx, opts(), q));
    CHECK_CONTAINS(out, "Nothing matches that");
    CHECK_CONTAINS(out, "Explore");
}

TEST(library_page_empty_and_populated_and_history)
{
    View v({ spec("org.t.app", "App") });
    CHECK_CONTAINS(v.page(renderLibrary(v.ctx, opts())), "haven't installed anything");
    v.lib.upsert({ "org.t.app", "App", "0.1.0", "main", "", "", "", "" });
    v.lib.addHistory({ "2026-10-01T00:00:00Z", "org.t.app", "install", "0.1.0", "success", "" });
    Page p = renderLibrary(v.ctx, opts());
    std::string out = v.page(p);
    CHECK_CONTAINS(out, "update to 1.0.0");
    CHECK_CONTAINS(out, "1 update available");
    CHECK_CONTAINS(out, "Recent activity");
    CHECK_CONTAINS(out, "install org.t.app 0.1.0 - success");
    CHECK_EQ(p.items.size(), size_t(1));
}

TEST(library_still_works_when_the_catalog_is_missing)
{
    Fixture f;
    Library lib(f.dir / "lib.json");
    lib.upsert({ "org.t.app", "App", "1.0.0", "main", "", "", "", "" });
    ViewContext ctx;
    ctx.catalog = nullptr;
    ctx.library = &lib;
    ctx.device = &f.device;
    ctx.catalogNotice = "Catalog unreachable.";
    Page p = renderLibrary(ctx, opts());
    std::string out;
    for (auto& l : p.lines)
        out += l + "\n";
    CHECK_CONTAINS(out, "App  1.0.0");
    CHECK_CONTAINS(out, "catalog is unavailable");
    // Other screens explain instead of crashing.
    Page h = renderHome(ctx, opts());
    CHECK_CONTAINS(h.lines[0], "Catalog unreachable.");
    CHECK(!renderSearch(ctx, opts(), SearchQuery {}).lines.empty());
    CHECK(!renderExplore(ctx, opts()).lines.empty());
    CHECK(!renderDetails(ctx, opts(), "org.t.app").lines.empty());
}

// ---------------- responsive layout ----------------

TEST(layout_switches_between_sidebar_and_bottom_navigation)
{
    CHECK(isWideLayout(100));
    CHECK(isWideLayout(kWideLayoutMinWidth));
    CHECK(!isWideLayout(kWideLayoutMinWidth - 1));
    View v({ spec("org.t.app", "App") });
    Theme t(false);
    Page p = renderHome(v.ctx, opts(contentWidth(100)));
    std::string wide = renderFrame(p, 100, t, "");
    CHECK_CONTAINS(wide, "EXILE STORE");
    CHECK_CONTAINS(wide, "[e] Explore");
    CHECK_CONTAINS(wide, "> [h] Home"); // current destination is marked in the sidebar
    std::string narrow = renderFrame(renderHome(v.ctx, opts(contentWidth(50))), 50, t, "");
    CHECK_NOT_CONTAINS(narrow, "EXILE STORE");                         // no sidebar
    CHECK_CONTAINS(narrow, "[h]Home [e]Explore [/]Search [l]Library"); // bottom navigation bar
}

TEST(narrow_layout_never_overflows_the_screen)
{
    PkgSpec long1 = spec("org.t.long", std::string(60, 'N') + " Very Long Application Name");
    long1.summary = std::string(150, 's') + " summary that is long";
    long1.extraApp = ",\"description\":\"" + std::string(400, 'd') + " " + std::string(30, 'e') + "\"";
    View v({ long1, spec("org.t.b", "Bee") });
    for (size_t width : { 40u, 50u, 60u, 71u }) {
        Theme t(false);
        size_t cw = contentWidth(width);
        std::vector<Page> pages = { renderHome(v.ctx, opts(cw)), renderExplore(v.ctx, opts(cw)),
            renderLibrary(v.ctx, opts(cw)), renderDetails(v.ctx, opts(cw), "org.t.long") };
        SearchQuery q;
        q.text = "n";
        pages.push_back(renderSearch(v.ctx, opts(cw), q));
        for (Page const& p : pages) {
            std::string frame = renderFrame(p, width, t, "A status message that is rather long and must wrap properly too.");
            std::istringstream in(frame);
            std::string line;
            while (std::getline(in, line))
                if (displayWidth(line) > width)
                    throw testing::Failure { "line wider than " + std::to_string(width) + " columns: " + line };
        }
    }
}

TEST(wide_layout_never_overflows_either)
{
    View v({ spec("org.t.app", std::string(80, 'W')) });
    Theme t(false);
    for (size_t width : { 72u, 80u, 100u, 140u }) {
        Page p = renderDetails(v.ctx, opts(contentWidth(width)), "org.t.app");
        std::istringstream in(renderFrame(p, width, t, ""));
        std::string line;
        while (std::getline(in, line))
            if (displayWidth(line) > width)
                throw testing::Failure { "wide line overflow at " + std::to_string(width) };
    }
}

TEST(color_output_uses_only_brand_colors_and_never_purple)
{
    View v({ spec("org.t.app", "App") });
    Page p = renderHome(v.ctx, [] { RenderOptions o; o.width = 70; o.theme = Theme(true); return o; }());
    std::string frame = renderFrame(p, 100, Theme(true), "");
    // ANSI codes allowed: bold/dim/reset, green (32), blue (34). Purple/magenta (35, 95, 45...) must not appear.
    for (size_t i = 0; i + 1 < frame.size(); ++i) {
        if (frame[i] == '\x1b' && frame[i + 1] == '[') {
            size_t end = frame.find('m', i);
            std::string code = frame.substr(i + 2, end - i - 2);
            for (char const* banned : { "35", "45", "95", "105", "38;5;93", "38;5;129" })
                CHECK(code != banned);
        }
    }
    CHECK_CONTAINS(frame, "\x1b[1;32m"); // green used for primary actions
    CHECK_CONTAINS(frame, "\x1b[34m");   // blue used for headings
}

// ---------------- controller: navigation and keyboard commands ----------------

namespace {
struct Nav {
    View v;
    UiController ui;
    explicit Nav(std::vector<PkgSpec> specs, bool canLaunch = false)
        : v(specs, canLaunch)
        , ui(v.ctx, 80, Theme(false))
    {
    }
};
} // namespace

TEST(controller_navigates_between_main_destinations)
{
    Nav n({ spec("org.t.app", "App") });
    CHECK(n.ui.screen() == Screen::Home);
    n.ui.handle("e");
    CHECK(n.ui.screen() == Screen::Explore);
    n.ui.handle("l");
    CHECK(n.ui.screen() == Screen::Library);
    n.ui.handle("h");
    CHECK(n.ui.screen() == Screen::Home);
    n.ui.handle("/ app");
    CHECK(n.ui.screen() == Screen::Search);
    CHECK_EQ(n.ui.query().text, std::string("app"));
    n.ui.handle("/notes"); // no space needed
    CHECK_EQ(n.ui.query().text, std::string("notes"));
    n.ui.handle("search hello world");
    CHECK_EQ(n.ui.query().text, std::string("hello world"));
}

TEST(controller_opens_items_by_number_and_goes_back)
{
    Nav n({ spec("org.t.one", "Alpha"), spec("org.t.two", "Beta") });
    n.ui.handle("/ a"); // both match "a"? Alpha and Beta contain 'a'
    Page p = n.ui.currentPage();
    CHECK(p.items.size() >= 1);
    n.ui.handle("1");
    CHECK(n.ui.screen() == Screen::Details);
    CHECK(!n.ui.selectedApp().empty());
    n.ui.handle("b");
    CHECK(n.ui.screen() == Screen::Search);
    n.ui.handle("b");
    CHECK(n.ui.screen() == Screen::Home);
    n.ui.handle("b");
    CHECK_CONTAINS(n.ui.render(), "Nothing to go back to");
}

TEST(controller_category_selection_opens_filtered_search)
{
    PkgSpec g = spec("org.t.g", "Game");
    g.category = "games";
    Nav n({ spec("org.t.u", "Util"), g });
    n.ui.handle("e");
    Page p = n.ui.currentPage();
    size_t gamesIdx = 0;
    for (size_t i = 0; i < p.items.size(); ++i)
        if (p.items[i] == "games")
            gamesIdx = i + 1;
    CHECK(gamesIdx > 0);
    n.ui.handle(std::to_string(gamesIdx));
    CHECK(n.ui.screen() == Screen::Search);
    CHECK_EQ(n.ui.query().categoryId, std::string("games"));
    std::string out = n.ui.render();
    CHECK_CONTAINS(out, "Game");
    CHECK_NOT_CONTAINS(out, "Util");
}

TEST(controller_out_of_range_and_unknown_input_are_handled_politely)
{
    Nav n({ spec("org.t.app", "App") });
    n.ui.handle("99");
    CHECK_CONTAINS(n.ui.render(), "no item 99");
    n.ui.handle("0");
    CHECK_CONTAINS(n.ui.render(), "no item 0");
    n.ui.handle("flibbertigibbet");
    CHECK_CONTAINS(n.ui.render(), "Unknown command");
    n.ui.handle("");
    n.ui.handle("   ");
    n.ui.handle("99999999999999999999"); // absurdly long number must not throw
    n.ui.handle("?");
    CHECK_CONTAINS(n.ui.render(), "install");
    CHECK(n.ui.handle("q").kind == Command::Kind::Quit);
    CHECK(n.ui.handle("quit").kind == Command::Kind::Quit);
}

TEST(controller_install_only_when_primary_action_is_install)
{
    Nav n({ spec("org.t.app", "App"), spec("org.t.phone", "Phone", "\"mobile\"") });
    n.ui.handle("i");
    CHECK_CONTAINS(n.ui.render(), "Open an app first"); // commands need an app in context

    n.ui.handle("/ phone");
    n.ui.handle("1");
    Command cmd = n.ui.handle("i");
    CHECK(cmd.kind == Command::Kind::None); // incompatible: refused
    CHECK_CONTAINS(n.ui.render(), "Not available for this device");

    n.ui.handle("h");
    n.ui.handle("/ app");
    n.ui.handle("1");
    cmd = n.ui.handle("i");
    CHECK(cmd.kind == Command::Kind::Install);
    CHECK_EQ(cmd.appId, std::string("org.t.app"));
    CHECK(n.ui.handle("u").kind == Command::Kind::None); // nothing to update
    CHECK_CONTAINS(n.ui.render(), "No update");
}

TEST(controller_update_uninstall_open_follow_state)
{
    Nav n({ spec("org.t.app", "App") }, /*canLaunch=*/true);
    n.v.lib.upsert({ "org.t.app", "App", "0.5.0", "main", "", "", "", "" });
    n.ui.handle("/ app");
    n.ui.handle("1");
    CHECK(n.ui.handle("u").kind == Command::Kind::Update);
    CHECK(n.ui.handle("i").kind == Command::Kind::None);
    CHECK_CONTAINS(n.ui.render(), "Use u to update");
    CHECK(n.ui.handle("x").kind == Command::Kind::Uninstall);
    CHECK(n.ui.handle("o").kind == Command::Kind::Open);
}

TEST(controller_open_is_refused_when_launching_is_not_supported)
{
    Nav n({ spec("org.t.app", "App") }, /*canLaunch=*/false);
    n.v.lib.upsert({ "org.t.app", "App", "1.0.0", "main", "", "", "", "" });
    n.ui.handle("/ app");
    n.ui.handle("1");
    CHECK(n.ui.handle("o").kind == Command::Kind::None);
    CHECK_CONTAINS(n.ui.render(), "isn't available");
}

TEST(controller_update_all_only_from_library)
{
    Nav n({ spec("org.t.app", "App") });
    CHECK(n.ui.handle("ua").kind == Command::Kind::None);
    CHECK_CONTAINS(n.ui.render(), "Library");
    n.ui.handle("l");
    CHECK(n.ui.handle("ua").kind == Command::Kind::UpdateAll);
}

TEST(controller_search_filter_and_sort_commands)
{
    Nav n({ spec("org.t.app", "Apple"), spec("org.t.phone", "Phone", "\"mobile\"") });
    n.ui.handle("/ ");
    CHECK(!n.ui.query().onlyCompatible);
    n.ui.handle("c");
    CHECK(n.ui.query().onlyCompatible);
    n.ui.handle("sort name");
    CHECK(n.ui.query().sort == SortOrder::Name);
    n.ui.handle("sort updated");
    CHECK(n.ui.query().sort == SortOrder::RecentlyUpdated);
    n.ui.handle("sort nonsense");
    CHECK_CONTAINS(n.ui.render(), "Sort by");
    n.ui.handle("/ a"); // the filter survives a new search
    CHECK(n.ui.query().onlyCompatible);
}

TEST(controller_message_is_shown_once)
{
    Nav n({ spec("org.t.app", "App") });
    n.ui.setMessage("hello there");
    CHECK_CONTAINS(n.ui.render(), "hello there");
    CHECK_NOT_CONTAINS(n.ui.render(), "hello there");
}

TEST(controller_every_screen_renders_at_every_width)
{
    Nav n({ spec("org.t.app", "App") });
    for (size_t w : { 40u, 45u, 72u, 120u }) {
        n.ui.setWidth(w);
        for (char const* cmd : { "h", "e", "l", "/ app", "1" }) {
            n.ui.handle(cmd);
            CHECK(!n.ui.render().empty());
        }
    }
}
