/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include "render.h"

#include <algorithm>
#include <cstdio>

#include "../core/util.h"

namespace exile::ui {

bool isWideLayout(size_t totalWidth) { return totalWidth >= kWideLayoutMinWidth; }
size_t contentWidth(size_t totalWidth)
{
    size_t w = isWideLayout(totalWidth) ? totalWidth - kSidebarWidth - 3 : totalWidth;
    return std::max<size_t>(w, 20);
}

std::string describePermission(std::string const& id)
{
    static const struct {
        char const* id;
        char const* text;
    } table[] = {
        { "network", "Connect to the internet" },
        { "camera", "Use the camera" },
        { "microphone", "Use the microphone" },
        { "location", "See where this device is" },
        { "documents", "Read and change files in your Documents folder" },
        { "photos", "Read and change your photos" },
        { "contacts", "Read your contacts" },
        { "notifications", "Show notifications" },
        { "background", "Keep running in the background" },
        { "bluetooth", "Use Bluetooth" },
        { "usb", "Talk to USB devices" },
        { "clipboard", "Read what you copy and paste" },
        { "system-settings", "Change system settings (sensitive)" },
    };
    for (auto const& e : table)
        if (id == e.id)
            return e.text;
    return "Uses \"" + id + "\" (not recognised by this version of the store)";
}

namespace {

std::string badge(AppStatus const& s)
{
    switch (s.action) {
    case PrimaryAction::Install:
        return "[Install]";
    case PrimaryAction::Update:
        return "[Update]";
    case PrimaryAction::Open:
    case PrimaryAction::Installed:
        return "[Installed]";
    case PrimaryAction::Unavailable:
        return "[Not for this device]";
    }
    return "";
}

std::string ratingText(AppEntry const& app)
{
    if (!app.rating)
        return "";
    char buf[64];
    std::snprintf(buf, sizeof buf, "%.1f/5 (%llu ratings)", app.rating->average, static_cast<unsigned long long>(app.rating->count));
    return buf;
}

void addCard(Page& page, size_t number, AppEntry const& app, ViewContext const& ctx, RenderOptions const& o)
{
    AppStatus st = computeStatus(app, *ctx.library, *ctx.device, ctx.backendCanLaunch);
    std::string b = badge(st);
    std::string num = (number < 10 ? " " : "") + std::to_string(number) + ". ";
    size_t avail = o.width > num.size() + b.size() + 2 ? o.width - num.size() - b.size() - 2 : 8;
    std::string name = truncate(app.name, avail);
    std::string gap(std::max<size_t>(2, o.width - num.size() - displayWidth(name) - b.size()), ' ');
    std::string badgeStyled = st.action == PrimaryAction::Unavailable ? o.theme.dim(b) : o.theme.green(b);
    page.lines.push_back(num + o.theme.bold(name) + gap + badgeStyled);

    std::string indent(num.size(), ' ');
    page.lines.push_back(indent + truncate(app.summary, o.width - num.size()));

    // Compatibility comes first so it is the last thing to be cut on narrow screens.
    std::string meta = st.compat.ok() ? "Works on this device" : "Not available for this device";
    if (auto r = ratingText(app); !r.empty())
        meta += "  |  " + r;
    meta += "  |  by " + app.publisher.name;
    page.lines.push_back(indent + o.theme.dim(truncate(meta, o.width - num.size())));
    page.lines.push_back("");
    page.items.push_back(app.appId);
}

void heading(Page& page, RenderOptions const& o, std::string const& text)
{
    page.lines.push_back(o.theme.blue(o.theme.bold(text)));
}

std::string categoryName(Catalog const& cat, std::string const& id)
{
    Category const* c = cat.findCategory(id);
    return c ? c->name : id;
}

std::string joinNames(std::vector<std::string> const& v, char const* sep = ", ")
{
    std::string out;
    for (size_t i = 0; i < v.size(); ++i)
        out += (i ? sep : "") + v[i];
    return out;
}

void addWrapped(Page& page, std::string const& text, size_t width, std::string const& indent = "")
{
    for (auto const& l : wrapText(text, width - indent.size()))
        page.lines.push_back(indent + l);
}

template<typename Style>
void addStyled(Page& page, std::string const& text, size_t width, Style style)
{
    for (auto const& l : wrapText(text, width))
        page.lines.push_back(style(l));
}

Page noCatalogPage(ViewContext const& ctx, RenderOptions const& o, NavTab tab)
{
    Page p;
    p.tab = tab;
    for (auto const& l : wrapText(ctx.catalogNotice.empty() ? "The app catalog is not available." : ctx.catalogNotice, o.width))
        p.lines.push_back(l);
    p.lines.push_back("");
    addWrapped(p, "Type 'l' to see the apps already installed on this device.", o.width);
    return p;
}

} // namespace

Page renderHome(ViewContext const& ctx, RenderOptions const& o)
{
    if (!ctx.catalog)
        return noCatalogPage(ctx, o, NavTab::Home);
    Page p;
    p.tab = NavTab::Home;
    p.kind = ItemKind::App;
    p.lines.push_back(o.theme.bold(o.theme.blue("Exile Store")));
    addStyled(p, "Apps for " + ctx.device->describe(), o.width, [&](std::string const& l) { return o.theme.dim(l); });
    p.lines.push_back("");
    addWrapped(p, "Search: type  / followed by what you're looking for", o.width);
    p.lines.push_back("");
    if (!ctx.catalogNotice.empty()) {
        for (auto const& l : wrapText(ctx.catalogNotice, o.width))
            p.lines.push_back(o.theme.bold(l));
        p.lines.push_back("");
    }

    std::vector<AppEntry const*> featured, recent;
    for (auto const& a : ctx.catalog->apps)
        if (a.featured && isListedFor(a, *ctx.device))
            featured.push_back(&a);
    std::vector<AppEntry const*> byDate;
    for (auto const& a : ctx.catalog->apps)
        if (!a.updatedAt.empty() && isListedFor(a, *ctx.device) && std::find(featured.begin(), featured.end(), &a) == featured.end())
            byDate.push_back(&a);
    std::stable_sort(byDate.begin(), byDate.end(), [](AppEntry const* x, AppEntry const* y) { return x->updatedAt > y->updatedAt; });
    for (size_t i = 0; i < byDate.size() && i < 4; ++i)
        recent.push_back(byDate[i]);
    if (featured.size() > 4)
        featured.resize(4);

    size_t n = 1;
    if (!featured.empty()) {
        heading(p, o, "Featured");
        for (AppEntry const* a : featured)
            addCard(p, n++, *a, ctx, o);
    }
    if (!recent.empty()) {
        heading(p, o, "Recently updated");
        for (AppEntry const* a : recent)
            addCard(p, n++, *a, ctx, o);
    }
    if (featured.empty() && recent.empty())
        p.lines.push_back("No apps are listed yet. Try Explore.");

    std::vector<std::string> cats;
    for (auto const& c : ctx.catalog->categories) {
        SearchQuery q;
        q.categoryId = c.id;
        size_t count = searchCatalog(*ctx.catalog, q, *ctx.device).size();
        if (count)
            cats.push_back(c.name);
        if (cats.size() >= 6)
            break;
    }
    if (!cats.empty()) {
        heading(p, o, "Categories");
        for (auto const& l : wrapText(joinNames(cats, "  -  ") + "   (e: see all)", o.width))
            p.lines.push_back(l);
        p.lines.push_back("");
    }

    size_t installed = ctx.library->installed().size();
    size_t updates = findUpdates(*ctx.catalog, *ctx.library, *ctx.device).size();
    std::string lib = "Your library: " + std::to_string(installed) + " installed";
    if (updates)
        lib += ", " + std::to_string(updates) + (updates == 1 ? " update" : " updates") + " available";
    addWrapped(p, lib + "   (l: open library)", o.width);
    return p;
}

Page renderExplore(ViewContext const& ctx, RenderOptions const& o)
{
    if (!ctx.catalog)
        return noCatalogPage(ctx, o, NavTab::Explore);
    Page p;
    p.tab = NavTab::Explore;
    p.kind = ItemKind::Category;
    heading(p, o, "Explore");
    addWrapped(p, "Pick a category by number.", o.width);
    p.lines.push_back("");
    size_t n = 1;
    for (auto const& c : ctx.catalog->categories) {
        SearchQuery q;
        q.categoryId = c.id;
        size_t count = searchCatalog(*ctx.catalog, q, *ctx.device).size();
        if (count == 0)
            continue; // categories with nothing to show stay out of the way
        std::string num = (n < 10 ? " " : "") + std::to_string(n) + ". ";
        p.lines.push_back(num + o.theme.bold(c.name) + o.theme.dim("  (" + std::to_string(count) + (count == 1 ? " app)" : " apps)")));
        p.items.push_back(c.id);
        ++n;
    }
    if (p.items.empty())
        addWrapped(p, "No categories have apps yet.", o.width);
    return p;
}

Page renderSearch(ViewContext const& ctx, RenderOptions const& o, SearchQuery const& q)
{
    if (!ctx.catalog)
        return noCatalogPage(ctx, o, NavTab::Search);
    Page p;
    p.tab = NavTab::Search;
    p.kind = ItemKind::App;

    std::string title = "Search";
    if (!q.categoryId.empty())
        title = categoryName(*ctx.catalog, q.categoryId);
    heading(p, o, title);
    std::string filters = "Showing: ";
    filters += q.text.empty() ? "all apps" : "\"" + q.text + "\"";
    filters += q.onlyCompatible ? "  |  works on this device only (c: show all)" : "  |  includes apps for other devices (c: filter)";
    for (auto const& l : wrapText(filters, o.width))
        p.lines.push_back(o.theme.dim(l));
    p.lines.push_back("");

    if (q.text.empty() && q.categoryId.empty()) {
        addWrapped(p, "Type  / followed by a name, purpose or developer to search.", o.width);
        return p;
    }
    auto hits = searchCatalog(*ctx.catalog, q, *ctx.device);
    if (hits.empty()) {
        addWrapped(p, "Nothing matches that.", o.width);
        addWrapped(p, "Try fewer or different words, or browse Explore (e).", o.width);
        return p;
    }
    size_t n = 1;
    for (auto const& h : hits)
        addCard(p, n++, *h.app, ctx, o);
    return p;
}

Page renderLibrary(ViewContext const& ctx, RenderOptions const& o)
{
    Page p;
    p.tab = NavTab::Library;
    p.kind = ItemKind::App;
    heading(p, o, "Library");
    if (!ctx.catalogNotice.empty() && ctx.catalog == nullptr) {
        addWrapped(p, "The catalog is unavailable, so updates can't be checked right now.", o.width);
    }
    p.lines.push_back("");

    std::vector<UpdateInfo> updates;
    if (ctx.catalog)
        updates = findUpdates(*ctx.catalog, *ctx.library, *ctx.device);

    if (ctx.library->installed().empty()) {
        addWrapped(p, "You haven't installed anything yet. Find something in Explore (e) or search with /.", o.width);
    } else {
        if (!updates.empty()) {
            addStyled(p, std::to_string(updates.size()) + (updates.size() == 1 ? " update available" : " updates available") + "   (ua: update all)", o.width, [&](std::string const& l) { return o.theme.green(l); });
            p.lines.push_back("");
        }
        size_t n = 1;
        for (auto const& a : ctx.library->installed()) {
            std::string num = (n < 10 ? " " : "") + std::to_string(n) + ". ";
            std::string state = "up to date";
            for (auto const& u : updates)
                if (u.app->appId == a.appId)
                    state = "update to " + u.newVersion;
            if (ctx.catalog && !ctx.catalog->find(a.appId))
                state = "no longer in the catalog";
            std::string label = (a.name.empty() ? a.appId : a.name) + "  " + a.version;
            p.lines.push_back(num + o.theme.bold(truncate(label, o.width - num.size())));
            p.lines.push_back(std::string(num.size(), ' ') + o.theme.dim(truncate(state, o.width - num.size())));
            p.items.push_back(a.appId);
            ++n;
        }
    }

    auto const& hist = ctx.library->history();
    if (!hist.empty()) {
        p.lines.push_back("");
        heading(p, o, "Recent activity");
        size_t shown = 0;
        for (auto it = hist.rbegin(); it != hist.rend() && shown < 5; ++it, ++shown) {
            std::string line = it->time.substr(0, 10) + "  " + it->action + " " + it->appId + " " + it->version + " - " + it->outcome;
            p.lines.push_back(o.theme.dim(truncate(line, o.width)));
        }
    }
    return p;
}

Page renderDetails(ViewContext const& ctx, RenderOptions const& o, std::string const& appId)
{
    Page p;
    p.tab = NavTab::None;
    AppEntry const* app = ctx.catalog ? ctx.catalog->find(appId) : nullptr;
    if (!app) {
        addWrapped(p, "This app is not in the catalog.", o.width);
        InstalledApp const* inst = ctx.library->find(appId);
        if (inst)
            addWrapped(p, "It is installed on this device as " + inst->name + " " + inst->version + ".", o.width);
        return p;
    }
    AppStatus st = computeStatus(*app, *ctx.library, *ctx.device, ctx.backendCanLaunch);

    p.lines.push_back(o.theme.bold(o.theme.blue(truncate(app->name, o.width))));
    std::string byline = "by " + app->publisher.name + "  |  Version " + (st.compat.ok() ? st.compat.version : app->currentVersion);
    std::vector<std::string> cats;
    for (auto const& c : app->categories)
        cats.push_back(categoryName(*ctx.catalog, c));
    if (!cats.empty())
        byline += "  |  " + joinNames(cats);
    for (auto const& l : wrapText(byline, o.width))
        p.lines.push_back(o.theme.dim(l));
    if (auto r = ratingText(*app); !r.empty())
        p.lines.push_back(o.theme.dim("Rated " + r));
    p.lines.push_back("");
    addWrapped(p, app->summary, o.width);
    p.lines.push_back("");

    // The single obvious primary action.
    std::string key;
    switch (st.action) {
    case PrimaryAction::Install:
        key = "   (i)";
        break;
    case PrimaryAction::Update:
        key = "   (u)";
        break;
    case PrimaryAction::Open:
        key = "   (o)";
        break;
    default:
        break;
    }
    if (st.action == PrimaryAction::Unavailable)
        p.lines.push_back(o.theme.bold(truncate("[ " + st.label + " ]", o.width)));
    else
        p.lines.push_back(o.theme.green("[ " + st.label + " ]") + key);
    if (!st.note.empty())
        addWrapped(p, st.note, o.width);
    if (st.installed)
        p.lines.push_back(o.theme.dim("Uninstall: x"));
    p.lines.push_back("");

    if (!app->description.empty()) {
        heading(p, o, "About");
        addWrapped(p, app->description, o.width);
        p.lines.push_back("");
    }
    if (!app->releaseNotes.empty()) {
        heading(p, o, "What's new in " + app->currentVersion);
        addWrapped(p, app->releaseNotes, o.width);
        p.lines.push_back("");
    }

    heading(p, o, "Works on");
    std::vector<std::string> devs, archs;
    for (DeviceClass d : app->supportedDevices())
        devs.push_back(displayName(d));
    for (Arch a : app->supportedArchitectures())
        archs.push_back(std::string(toString(a)));
    addWrapped(p, "Devices: " + joinNames(devs), o.width);
    addWrapped(p, "Processors: " + joinNames(archs), o.width);
    std::string minOs = app->minimumOsVersion();
    addWrapped(p, "Exile OS: " + (minOs.empty() ? std::string("any version") : minOs + " or newer"), o.width);
    if (st.compat.build)
        addWrapped(p, "Type for this device: " + displayName(st.compat.build->kind), o.width);
    p.lines.push_back("");

    if (st.compat.build) {
        PackageBuild const& b = *st.compat.build;
        std::string sizes;
        if (b.downloadSize)
            sizes += "Download " + util::humanSize(b.downloadSize);
        if (b.packageSize)
            sizes += std::string(sizes.empty() ? "" : "  |  ") + "Installed size " + util::humanSize(b.packageSize);
        if (!sizes.empty()) {
            heading(p, o, "Size");
            p.lines.push_back(sizes);
            p.lines.push_back("");
        }

        if (!b.dependencies.empty()) {
            heading(p, o, "Needs");
            for (auto const& d : b.dependencies) {
                AppEntry const* da = ctx.catalog->find(d.appId);
                InstalledApp const* di = ctx.library->find(d.appId);
                std::string line = "- " + (da ? da->name : d.appId);
                if (!d.minVersion.empty())
                    line += " " + d.minVersion + " or newer";
                line += di ? "  (installed)" : "  (not installed)";
                p.lines.push_back(truncate(line, o.width));
            }
            p.lines.push_back("");
        }
    }

    heading(p, o, "Permissions");
    if (app->permissions.empty())
        addWrapped(p, "This app asks for no special permissions.", o.width);
    for (auto const& perm : app->permissions) {
        addWrapped(p, "- " + describePermission(perm.id), o.width);
        if (!perm.reason.empty())
            addWrapped(p, "Publisher says: " + perm.reason, o.width, "    ");
    }
    p.lines.push_back("");

    heading(p, o, "Publisher");
    p.lines.push_back(truncate(app->publisher.name + "  (" + app->publisher.id + ")", o.width));
    if (st.compat.build && ctx.trust && ctx.verifier) {
        VerifyOutcome vo = verifyPackageSignature(*app, *st.compat.build, st.compat.version, *ctx.trust, *ctx.verifier);
        if (vo.trusted)
            addWrapped(p, "Signature: valid, made with trusted key '" + vo.keyId + "' for this publisher. "
                                                                                   "The package itself is checked again before installing.",
                o.width);
        else
            addWrapped(p, "Signature: not verified because " + vo.reason + ". Installation is blocked.", o.width);
    }
    addWrapped(p, "A valid signature shows who published an app, not that the app is safe.", o.width);
    if (!app->license.empty())
        addWrapped(p, "License: " + app->license, o.width);
    if (!app->privacyPolicy.empty())
        addWrapped(p, "Privacy policy: " + app->privacyPolicy, o.width);
    if (!app->sourceRepository.empty())
        addWrapped(p, "Source code: " + app->sourceRepository, o.width);
    if (!app->screenshots.empty())
        addStyled(p, std::to_string(app->screenshots.size()) + " screenshot(s) listed (not shown in the terminal view)", o.width,
            [&](std::string const& l) { return o.theme.dim(l); });
    return p;
}

std::string renderFrame(Page const& page, size_t totalWidth, Theme const& theme, std::string const& message)
{
    struct Nav {
        NavTab tab;
        char const* key;
        char const* label;
    };
    static Nav const navs[] = { { NavTab::Home, "h", "Home" }, { NavTab::Explore, "e", "Explore" },
        { NavTab::Search, "/", "Search" }, { NavTab::Library, "l", "Library" } };
    std::string out;
    auto emit = [&](std::string const& line) { out += line; out += '\n'; };

    std::vector<std::string> msg;
    if (!message.empty())
        for (auto const& l : wrapText(message, contentWidth(totalWidth)))
            msg.push_back(theme.bold(l));

    std::string hint = "number: open  |  b: back  |  ?: help  |  q: quit";

    if (isWideLayout(totalWidth)) {
        size_t cw = contentWidth(totalWidth);
        std::vector<std::string> side;
        side.push_back(theme.bold(theme.blue("EXILE STORE")));
        side.push_back("");
        for (auto const& n : navs) {
            std::string label = std::string(page.tab == n.tab ? "> " : "  ") + "[" + n.key + "] " + n.label;
            side.push_back(page.tab == n.tab ? theme.green(label) : label);
        }
        std::vector<std::string> content = msg;
        if (!msg.empty())
            content.push_back("");
        content.insert(content.end(), page.lines.begin(), page.lines.end());
        size_t rows = std::max(content.size(), side.size());
        for (size_t i = 0; i < rows; ++i) {
            std::string left = padRight(i < side.size() ? side[i] : "", kSidebarWidth);
            std::string right = i < content.size() ? content[i] : "";
            emit(left + " | " + right);
        }
        emit(padRight("", kSidebarWidth) + " | " + theme.dim(truncate(hint, cw)));
    } else {
        for (auto const& l : msg)
            emit(l);
        if (!msg.empty())
            emit("");
        for (auto const& l : page.lines)
            emit(l);
        emit(theme.dim(repeat('-', totalWidth)));
        std::string bar;
        for (auto const& n : navs) {
            std::string item = std::string("[") + n.key + "]" + n.label;
            bar += (bar.empty() ? "" : " ") + (page.tab == n.tab ? theme.green(item) : item);
        }
        emit(bar);
        emit(theme.dim(truncate("number: open | b: back | ?: help | q: quit", totalWidth)));
    }
    return out;
}

} // namespace exile::ui
