#include "server/api.h"

#include <algorithm>
#include <cstdio>
#include <cstring>
#include <fstream>

#include "core/appstate.h"
#include "core/catalog.h"
#include "core/json.h"
#include "core/search.h"
#include "core/util.h"
#include "server/assets.h"
#include "ui/render.h"

namespace exile::server {

using json::Value;

namespace {

const char* kCsp =
    "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; "
    "base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

void harden(Response& r) {
    r.headers["Content-Security-Policy"] = kCsp;
    r.headers["X-Content-Type-Options"] = "nosniff";
    r.headers["Referrer-Policy"] = "no-referrer";
    r.headers["Cross-Origin-Resource-Policy"] = "same-origin";
    r.headers["Cross-Origin-Opener-Policy"] = "same-origin";
    r.headers["X-Frame-Options"] = "DENY";
}

Response jsonResponse(const Value& v, int status = 200) {
    Response r;
    r.status = status;
    r.contentType = "application/json; charset=utf-8";
    r.body = json::dump(v, 0);
    r.headers["Cache-Control"] = "no-store";
    harden(r);
    return r;
}

Response errorResponse(int status, const std::string& message) {
    Value o = Value::makeObject();
    o.set("error", Value::makeString(message));
    return jsonResponse(o, status);
}

Value str(const std::string& s) { return Value::makeString(s); }
Value num(double n) { return Value::makeNumber(n); }
Value boolean(bool b) { return Value::makeBool(b); }

bool constantTimeEquals(const std::string& a, const std::string& b) {
    unsigned char diff = a.size() == b.size() ? 0 : 1;
    size_t n = std::min(a.size(), b.size());
    for (size_t i = 0; i < n; ++i) diff |= static_cast<unsigned char>(a[i] ^ b[i]);
    return diff == 0;
}

std::map<std::string, std::string> parseQuery(const std::string& q) {
    std::map<std::string, std::string> out;
    size_t pos = 0;
    while (pos <= q.size() && !q.empty()) {
        size_t amp = q.find('&', pos);
        std::string pair = q.substr(pos, amp == std::string::npos ? std::string::npos : amp - pos);
        size_t eq = pair.find('=');
        std::string k = pair.substr(0, eq);
        std::string v = eq == std::string::npos ? std::string() : pair.substr(eq + 1);
        for (char& c : k) if (c == '+') c = ' ';
        for (char& c : v) if (c == '+') c = ' ';
        bool ok1 = true, ok2 = true;
        k = percentDecode(k, ok1);
        v = percentDecode(v, ok2);
        if (ok1 && ok2 && !k.empty()) out[k] = v;
        if (amp == std::string::npos) break;
        pos = amp + 1;
    }
    return out;
}

std::string actionName(PrimaryAction a) {
    switch (a) {
        case PrimaryAction::Install: return "install";
        case PrimaryAction::Open: return "open";
        case PrimaryAction::Installed: return "installed";
        case PrimaryAction::Update: return "update";
        case PrimaryAction::Unavailable: return "unavailable";
    }
    return "unavailable";
}

struct Ctx {
    const app::Engine& engine;
    app::Snapshot snap;
    const Catalog* cat;
    const Library& lib;
    const DeviceProfile& dev;
};

Value deviceNames(const AppEntry& a) {
    Value v = Value::makeArray();
    for (DeviceClass d : a.supportedDevices()) v.push(str(displayName(d)));
    return v;
}

Value card(const Ctx& c, const AppEntry& a) {
    AppStatus st = computeStatus(a, c.lib, c.dev, c.engine.canLaunch());
    Value o = Value::makeObject();
    o.set("appId", str(a.appId));
    o.set("name", str(a.name));
    o.set("summary", str(a.summary));
    Value pub = Value::makeObject();
    pub.set("id", str(a.publisher.id));
    pub.set("name", str(a.publisher.name));
    o.set("publisher", pub);
    Value cats = Value::makeArray();
    for (const auto& id : a.categories) cats.push(str(id));
    o.set("categories", cats);
    o.set("version", str(a.currentVersion));
    o.set("featured", boolean(a.featured));
    o.set("updatedAt", str(a.updatedAt));
    if (a.rating) {
        Value r = Value::makeObject();
        r.set("average", num(a.rating->average));
        r.set("count", num(static_cast<double>(a.rating->count)));
        o.set("rating", r);
    }
    Value comp = Value::makeObject();
    comp.set("ok", boolean(st.compat.ok()));
    comp.set("reason", str(st.compat.reason));
    comp.set("devices", deviceNames(a));
    if (st.compat.build) {
        comp.set("kind", str(std::string(toString(st.compat.build->kind))));
        comp.set("kindLabel", str(displayName(st.compat.build->kind)));
        comp.set("universal", boolean(std::find(st.compat.build->devices.begin(), st.compat.build->devices.end(), DeviceClass::Any) !=
                                      st.compat.build->devices.end()));
    }
    o.set("compat", comp);
    Value state = Value::makeObject();
    state.set("action", str(actionName(st.action)));
    state.set("label", str(st.label));
    state.set("note", str(st.note));
    if (st.installed) state.set("installedVersion", str(st.installed->version));
    o.set("state", state);
    return o;
}

Value details(const Ctx& c, const AppEntry& a) {
    AppStatus st = computeStatus(a, c.lib, c.dev, c.engine.canLaunch());
    Value o = card(c, a);
    o.set("description", str(a.description));
    o.set("releaseNotes", str(a.releaseNotes));
    Value archs = Value::makeArray();
    for (Arch ar : a.supportedArchitectures()) archs.push(str(std::string(toString(ar))));
    o.set("architectures", archs);
    o.set("minimumOs", str(a.minimumOsVersion()));
    o.set("license", str(a.license));
    o.set("privacyPolicy", str(a.privacyPolicy));
    o.set("sourceRepository", str(a.sourceRepository));
    o.set("screenshotCount", num(static_cast<double>(a.screenshots.size())));

    Value perms = Value::makeArray();
    for (const auto& p : a.permissions) {
        Value pv = Value::makeObject();
        pv.set("id", str(p.id));
        pv.set("text", str(ui::describePermission(p.id)));
        pv.set("reason", str(p.reason));
        perms.push(pv);
    }
    o.set("permissions", perms);

    if (st.compat.build) {
        const PackageBuild& b = *st.compat.build;
        o.set("downloadSize", num(static_cast<double>(b.downloadSize)));
        o.set("installedSize", num(static_cast<double>(b.packageSize)));
        Value deps = Value::makeArray();
        for (const auto& d : b.dependencies) {
            const AppEntry* da = c.cat->find(d.appId);
            Value dv = Value::makeObject();
            dv.set("appId", str(d.appId));
            dv.set("name", str(da ? da->name : d.appId));
            dv.set("minVersion", str(d.minVersion));
            dv.set("installed", boolean(c.lib.find(d.appId) != nullptr));
            deps.push(dv);
        }
        o.set("dependencies", deps);

        Value trust = Value::makeObject();
        VerifyOutcome vo = verifyPackageSignature(a, b, st.compat.version, c.engine.trust(), c.engine.verifier());
        trust.set("trusted", boolean(vo.trusted));
        trust.set("keyId", str(vo.keyId));
        trust.set("reason", str(vo.reason));
        trust.set("caveat", str("A valid signature shows who published an app, not that the app is safe."));
        o.set("trust", trust);
    }
    return o;
}

Value taskJson(const app::Engine& engine, const app::TaskInfo& t) {
    Value o = Value::makeObject();
    o.set("id", num(static_cast<double>(t.id)));
    o.set("kind", str(app::toString(t.kind)));
    o.set("appId", str(t.appId));
    o.set("appName", str(t.appName));
    o.set("state", str(toString(t.state)));
    o.set("done", num(static_cast<double>(t.done)));
    o.set("total", num(static_cast<double>(t.total)));
    o.set("message", str(t.message));
    o.set("error", str(t.error));
    o.set("detail", str(t.detail));
    o.set("finished", boolean(t.finished));
    o.set("ok", boolean(t.ok));
    o.set("cancelRequested", boolean(t.cancelRequested));
    o.set("cancellable", boolean(!t.finished && t.state != JobState::Installing && !t.cancelRequested));
    Value blocked = Value::makeArray();
    auto snap = engine.snapshot();
    for (const auto& id : t.blockedBy) {
        const AppEntry* a = snap.cat() ? snap.cat()->find(id) : nullptr;
        blocked.push(str(a ? a->name : id));
    }
    o.set("blockedBy", blocked);
    return o;
}

SortOrder parseSort(const std::string& s) {
    if (s == "updated") return SortOrder::RecentlyUpdated;
    if (s == "name") return SortOrder::Name;
    return SortOrder::Relevance;
}

bool readAppId(const Request& req, std::string& appId, std::string& error) {
    try {
        Value v = json::parse(req.body);
        const Value* id = v.find("appId");
        if (!id || !id->string() || !isValidAppId(*id->string())) { error = "A valid appId is required."; return false; }
        appId = *id->string();
        return true;
    } catch (const json::ParseError&) {
        error = "The request body must be JSON.";
        return false;
    }
}

}  // namespace

Api::Api(app::Engine& engine, std::string token) : engine_(engine), token_(std::move(token)) {}

std::string Api::makeToken(std::string& error) {
    std::ifstream f("/dev/urandom", std::ios::binary);
    unsigned char buf[32];
    if (!f || !f.read(reinterpret_cast<char*>(buf), sizeof buf)) {
        error = "No secure random source is available, so the local service will not start.";
        return {};
    }
    return util::hexEncode(buf, sizeof buf);
}

Response Api::handle(const Request& req) {
    // 1. Only our own loopback origin may talk to us (DNS rebinding defence).
    std::string host = req.header("host");
    std::string p = std::to_string(port_);
    if (host != "127.0.0.1:" + p && host != "localhost:" + p) {
        Response r = errorResponse(403, "Unexpected Host header.");
        return r;
    }
    if (req.path.compare(0, 5, "/api/") == 0 || req.path == "/api") return dispatchApi(req);
    return serveAsset(req);
}

Response Api::serveAsset(const Request& req) {
    if (req.method != "GET" && req.method != "HEAD") return errorResponse(405, "Method not allowed.");
    const Asset* a = findAsset(req.path);
    if (!a) {
        Response r;
        r.status = 404;
        r.body = "Not found\n";
        harden(r);
        return r;
    }
    Response r;
    r.contentType = a->mime;
    r.body.assign(reinterpret_cast<const char*>(a->data), a->size);
    r.headers["Cache-Control"] = "no-cache";
    harden(r);
    return r;
}

Response Api::dispatchApi(const Request& req) {
    // 2. Browser-supplied provenance must be ours.
    std::string origin = req.header("origin");
    std::string myHost = req.header("host");
    if (!origin.empty() && origin != "http://" + myHost) return errorResponse(403, "Cross-origin request refused.");
    std::string sfs = req.header("sec-fetch-site");
    if (!sfs.empty() && sfs != "same-origin" && sfs != "none") return errorResponse(403, "Cross-site request refused.");

    // 3. Bearer token.
    std::string auth = req.header("authorization");
    const std::string prefix = "Bearer ";
    if (auth.compare(0, prefix.size(), prefix) != 0 || !constantTimeEquals(auth.substr(prefix.size()), token_))
        return errorResponse(401, "Missing or wrong access token. Open the store from the link it printed.");

    const bool isPost = req.method == "POST";
    if (req.method != "GET" && req.method != "HEAD" && !isPost) return errorResponse(405, "Method not allowed.");
    if (isPost) {
        std::string ct = util::toLower(req.header("content-type"));
        if (ct.compare(0, 16, "application/json") != 0) return errorResponse(415, "POST bodies must be application/json.");
    }

    const std::string& path = req.path;
    auto query = parseQuery(req.query);
    app::Snapshot snap = engine_.snapshot();
    const Catalog* cat = snap.cat();
    const Library& lib = *snap.library;
    const DeviceProfile& dev = engine_.device();
    Ctx c{engine_, snap, cat, lib, dev};

    // ----- session -----
    if (path == "/api/session" && !isPost) {
        Value o = Value::makeObject();
        Value d = Value::makeObject();
        d.set("class", str(std::string(toString(dev.deviceClass))));
        d.set("classLabel", str(displayName(dev.deviceClass)));
        d.set("arch", str(std::string(toString(dev.arch))));
        d.set("os", str(dev.osVersion));
        d.set("describe", str(dev.describe()));
        o.set("device", d);
        Value s = Value::makeObject();
        s.set("version", str("0.1.0"));
        s.set("backend", str(engine_.backendName()));
        s.set("canLaunch", boolean(engine_.canLaunch()));
        s.set("verifier", str(engine_.verifier().name()));
        s.set("verifierAvailable", boolean(engine_.verifier().available()));
        s.set("trustedKeys", num(static_cast<double>(engine_.trust().size())));
        s.set("productionSecure", boolean(false));
        s.set("posture", str("Development build. The installer is unprivileged with no sandbox or permission enforcement, "
                             "and the publisher keys come from a local file. Do not treat an install as proof an app is safe."));
        o.set("store", s);
        Value cs = Value::makeObject();
        cs.set("available", boolean(cat != nullptr));
        cs.set("fromCache", boolean(snap.catalog->fromCache));
        cs.set("notice", str(snap.catalog->notice));
        cs.set("apps", num(cat ? static_cast<double>(cat->apps.size()) : 0));
        cs.set("skipped", num(static_cast<double>(snap.catalog->issues.size())));
        o.set("catalog", cs);
        Value notices = Value::makeArray();
        for (const auto& n : engine_.startupNotices()) notices.push(str(n));
        o.set("notices", notices);
        return jsonResponse(o);
    }

    // ----- home -----
    if (path == "/api/home" && !isPost) {
        if (!cat) return errorResponse(503, snap.catalog->notice.empty() ? "The catalog is not available." : snap.catalog->notice);
        Value o = Value::makeObject();
        SearchQuery q;
        q.sort = SortOrder::RecentlyUpdated;
        auto hits = searchCatalog(*cat, q, dev);
        Value featured = Value::makeArray(), recent = Value::makeArray();
        for (const auto& h : hits) {
            if (h.app->featured && featured.items().size() < 8) featured.push(card(c, *h.app));
            if (recent.items().size() < 12) recent.push(card(c, *h.app));
        }
        o.set("featured", featured);
        o.set("recent", recent);
        Value ups = Value::makeArray();
        for (const auto& u : findUpdates(*cat, lib, dev)) {
            Value uv = card(c, *u.app);
            uv.set("from", str(u.installedVersion));
            uv.set("to", str(u.newVersion));
            ups.push(uv);
        }
        o.set("updates", ups);
        Value cats = Value::makeArray();
        SearchQuery all;
        auto everything = searchCatalog(*cat, all, dev);
        for (const auto& cg : cat->categories) {
            size_t n = 0;
            for (const auto& h : everything)
                if (std::find(h.app->categories.begin(), h.app->categories.end(), cg.id) != h.app->categories.end()) ++n;
            Value cv = Value::makeObject();
            cv.set("id", str(cg.id));
            cv.set("name", str(cg.name));
            cv.set("count", num(static_cast<double>(n)));
            cats.push(cv);
        }
        o.set("categories", cats);
        return jsonResponse(o);
    }

    // ----- apps -----
    if (path == "/api/apps" && !isPost) {
        if (!cat) return errorResponse(503, snap.catalog->notice.empty() ? "The catalog is not available." : snap.catalog->notice);
        SearchQuery q;
        if (query.count("q")) q.text = query["q"].substr(0, 200);
        if (query.count("category")) q.categoryId = query["category"].substr(0, 100);
        q.onlyCompatible = query.count("compatible") && query["compatible"] == "1";
        q.sort = parseSort(query.count("sort") ? query["sort"] : "");
        Value list = Value::makeArray();
        for (const auto& h : searchCatalog(*cat, q, dev)) list.push(card(c, *h.app));
        Value o = Value::makeObject();
        o.set("apps", list);
        o.set("total", num(static_cast<double>(list.items().size())));
        return jsonResponse(o);
    }
    if (path.compare(0, 10, "/api/apps/") == 0 && !isPost) {
        std::string id = path.substr(10);
        if (!cat) return errorResponse(503, "The catalog is not available.");
        const AppEntry* a = isValidAppId(id) ? cat->find(id) : nullptr;
        if (!a) return errorResponse(404, "That app is not in the catalog.");
        return jsonResponse(details(c, *a));
    }

    // ----- library -----
    if (path == "/api/library" && !isPost) {
        Value o = Value::makeObject();
        Value inst = Value::makeArray();
        for (const auto& i : lib.installed()) {
            const AppEntry* a = cat ? cat->find(i.appId) : nullptr;
            Value iv = a ? card(c, *a) : Value::makeObject();
            if (!a) {
                iv.set("appId", str(i.appId));
                iv.set("name", str(i.name.empty() ? i.appId : i.name));
                iv.set("summary", str("No longer listed in the catalog."));
            }
            iv.set("installedVersion", str(i.version));
            iv.set("installedAt", str(i.installedAt));
            iv.set("publisherId", str(i.publisherId));
            inst.push(iv);
        }
        o.set("installed", inst);
        Value ups = Value::makeArray();
        if (cat)
            for (const auto& u : findUpdates(*cat, lib, dev)) {
                Value uv = card(c, *u.app);
                uv.set("from", str(u.installedVersion));
                uv.set("to", str(u.newVersion));
                uv.set("releaseNotes", str(u.releaseNotes));
                ups.push(uv);
            }
        o.set("updates", ups);
        Value hist = Value::makeArray();
        const auto& h = lib.history();
        for (size_t k = h.size(); k > 0 && hist.items().size() < 20; --k) {
            const auto& e = h[k - 1];
            Value hv = Value::makeObject();
            hv.set("time", str(e.time));
            hv.set("appId", str(e.appId));
            hv.set("action", str(e.action));
            hv.set("version", str(e.version));
            hv.set("outcome", str(e.outcome));
            hv.set("detail", str(e.detail));
            hist.push(hv);
        }
        o.set("history", hist);
        return jsonResponse(o);
    }

    // ----- tasks -----
    if (path == "/api/tasks" && !isPost) {
        Value list = Value::makeArray();
        for (const auto& t : engine_.tasks()) list.push(taskJson(engine_, t));
        Value o = Value::makeObject();
        o.set("tasks", list);
        return jsonResponse(o);
    }
    if (path.compare(0, 11, "/api/tasks/") == 0) {
        std::string rest = path.substr(11);
        bool isCancel = false;
        if (rest.size() > 7 && rest.compare(rest.size() - 7, 7, "/cancel") == 0) { isCancel = true; rest.resize(rest.size() - 7); }
        if (rest.empty() || rest.size() > 9 || rest.find_first_not_of("0123456789") != std::string::npos)
            return errorResponse(404, "No such task.");
        uint64_t id = std::stoull(rest);
        if (isCancel) {
            if (!isPost) return errorResponse(405, "Use POST to cancel.");
            if (!engine_.cancel(id)) return errorResponse(409, "That task can't be cancelled right now.");
            auto t = engine_.task(id);
            return jsonResponse(t ? taskJson(engine_, *t) : Value::makeObject());
        }
        if (isPost) return errorResponse(405, "Method not allowed.");
        auto t = engine_.task(id);
        if (!t) return errorResponse(404, "No such task.");
        return jsonResponse(taskJson(engine_, *t));
    }

    // ----- actions -----
    if (isPost && (path == "/api/install" || path == "/api/update" || path == "/api/uninstall")) {
        std::string appId, err;
        if (!readAppId(req, appId, err)) return errorResponse(400, err);
        app::TaskKind kind = path == "/api/install" ? app::TaskKind::Install
                           : path == "/api/update"  ? app::TaskKind::Update
                                                    : app::TaskKind::Uninstall;
        uint64_t id = engine_.submit(kind, appId, err);
        if (!id) return errorResponse(409, err);
        auto t = engine_.task(id);
        return jsonResponse(taskJson(engine_, *t), 202);
    }
    if (isPost && path == "/api/update-all") {
        Value ids = Value::makeArray();
        if (cat)
            for (const auto& u : findUpdates(*cat, lib, dev)) {
                std::string err;
                if (uint64_t id = engine_.submit(app::TaskKind::Update, u.app->appId, err)) ids.push(num(static_cast<double>(id)));
            }
        Value o = Value::makeObject();
        o.set("count", num(static_cast<double>(ids.items().size())));
        o.set("tasks", ids);
        return jsonResponse(o, 202);
    }
    if (isPost && path == "/api/refresh") {
        engine_.refreshCatalog();
        Value o = Value::makeObject();
        o.set("ok", boolean(true));
        return jsonResponse(o);
    }
    return errorResponse(404, "No such endpoint.");
}

}  // namespace exile::server
