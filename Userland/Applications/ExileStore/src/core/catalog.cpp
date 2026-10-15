#include "core/catalog.h"

#include <algorithm>
#include <set>

#include "core/json.h"
#include "core/util.h"
#include "core/version.h"

namespace exile {

using json::Value;

namespace {

constexpr size_t kMaxApps = 20000;
constexpr size_t kMaxPackagesPerApp = 32;

bool isIdentifierSegment(std::string_view s, bool allowUnderscore) {
    if (s.empty() || s.size() > 63) return false;
    auto alnum = [](char c) { return (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9'); };
    if (!alnum(s.front())) return false;
    for (char c : s)
        if (!(alnum(c) || c == '-' || (allowUnderscore && c == '_'))) return false;
    return true;
}

bool isSimpleId(std::string_view s) {  // categories, package ids, key ids
    return isIdentifierSegment(s, true);
}

bool isDateLike(std::string_view s) {
    if (s.size() < 10) return false;
    for (size_t i = 0; i < 10; ++i) {
        if (i == 4 || i == 7) { if (s[i] != '-') return false; }
        else if (!(s[i] >= '0' && s[i] <= '9')) return false;
    }
    return !util::hasControlChars(s);
}

struct Errs {
    std::vector<std::string> list;
    void add(std::string m) { list.push_back(std::move(m)); }
    bool empty() const { return list.empty(); }
};

std::string readString(const Value& obj, std::string_view key, bool required, size_t maxLen, Errs& e,
                       const std::string& ctx, bool multiline = false) {
    const Value* v = obj.find(key);
    if (!v || v->isNull()) {
        if (required) e.add(ctx + std::string(key) + ": missing");
        return {};
    }
    const std::string* s = v->string();
    if (!s) { e.add(ctx + std::string(key) + ": must be a string"); return {}; }
    if (s->size() > maxLen) { e.add(ctx + std::string(key) + ": too long"); return {}; }
    if (util::hasControlChars(*s, multiline)) { e.add(ctx + std::string(key) + ": contains control characters"); return {}; }
    if (required && s->empty()) { e.add(ctx + std::string(key) + ": must not be empty"); return {}; }
    return *s;
}

std::vector<std::string> readStringList(const Value& obj, std::string_view key, size_t maxItems, size_t maxLen,
                                        Errs& e, const std::string& ctx) {
    std::vector<std::string> out;
    const Value* v = obj.find(key);
    if (!v || v->isNull()) return out;
    if (!v->isArray()) { e.add(ctx + std::string(key) + ": must be an array of strings"); return out; }
    if (v->items().size() > maxItems) { e.add(ctx + std::string(key) + ": too many entries"); return out; }
    for (const Value& item : v->items()) {
        const std::string* s = item.string();
        if (!s || s->empty() || s->size() > maxLen || util::hasControlChars(*s)) {
            e.add(ctx + std::string(key) + ": entries must be short non-empty strings");
            return {};
        }
        out.push_back(*s);
    }
    return out;
}

bool readUnsigned(const Value& obj, std::string_view key, uint64_t& out, Errs& e, const std::string& ctx) {
    const Value* v = obj.find(key);
    if (!v || v->isNull()) return true;
    auto n = v->unsignedInteger();
    if (!n) { e.add(ctx + std::string(key) + ": must be a non-negative integer"); return false; }
    out = *n;
    return true;
}

std::optional<PackageBuild> parsePackage(const Value& v, Errs& e, const std::string& ctx) {
    if (!v.isObject()) { e.add(ctx + "must be an object"); return std::nullopt; }
    size_t before = e.list.size();
    PackageBuild p;

    p.id = readString(v, "id", true, 63, e, ctx);
    if (!p.id.empty() && !isSimpleId(p.id)) e.add(ctx + "id: invalid package id");

    if (const Value* k = v.find("kind"); k && !k->isNull()) {
        const std::string* ks = k->string();
        auto kind = ks ? parsePackageKind(*ks) : std::nullopt;
        if (!kind) e.add(ctx + "kind: must be native, compat-layer or web");
        else p.kind = *kind;
    }
    if (const Value* c = v.find("channel"); c && !c->isNull()) {
        const std::string* cs = c->string();
        auto ch = cs ? parseChannel(*cs) : std::nullopt;
        if (!ch) e.add(ctx + "channel: must be stable, beta or nightly");
        else p.channel = *ch;
    }

    const Value* devs = v.find("devices");
    if (!devs || !devs->isArray() || devs->items().empty()) e.add(ctx + "devices: required non-empty array");
    else for (const Value& d : devs->items()) {
        const std::string* ds = d.string();
        auto dc = ds ? parseDeviceClass(*ds) : std::nullopt;
        if (!dc) e.add(ctx + "devices: unknown device class");
        else if (std::find(p.devices.begin(), p.devices.end(), *dc) == p.devices.end()) p.devices.push_back(*dc);
    }
    const Value* archs = v.find("architectures");
    if (!archs || !archs->isArray() || archs->items().empty()) e.add(ctx + "architectures: required non-empty array");
    else for (const Value& a : archs->items()) {
        const std::string* as = a.string();
        auto ar = as ? parseArch(*as) : std::nullopt;
        if (!ar) e.add(ctx + "architectures: unknown architecture");
        else if (std::find(p.architectures.begin(), p.architectures.end(), *ar) == p.architectures.end()) p.architectures.push_back(*ar);
    }

    p.minimumOsVersion = readString(v, "minimumOsVersion", false, 100, e, ctx);
    if (!p.minimumOsVersion.empty() && !parseVersion(p.minimumOsVersion)) e.add(ctx + "minimumOsVersion: not a valid version");
    p.version = readString(v, "version", false, 100, e, ctx);
    if (!p.version.empty() && !parseVersion(p.version)) e.add(ctx + "version: not a valid version");

    p.url = readString(v, "url", true, 2048, e, ctx);
    if (!p.url.empty()) {
        std::string why;
        if (!isAcceptableSourceUrl(p.url, &why)) e.add(ctx + "url: " + why);
    }
    p.sha256 = util::toLower(readString(v, "sha256", true, 64, e, ctx));
    if (!p.sha256.empty() && !util::isLowerHex(p.sha256, 64)) e.add(ctx + "sha256: must be 64 hex characters");

    readUnsigned(v, "downloadSize", p.downloadSize, e, ctx);
    readUnsigned(v, "packageSize", p.packageSize, e, ctx);

    if (const Value* sig = v.find("signature"); sig && !sig->isNull()) {
        if (!sig->isObject()) e.add(ctx + "signature: must be an object");
        else {
            std::string sctx = ctx + "signature.";
            p.signature.keyId = readString(*sig, "keyId", true, 63, e, sctx);
            p.signature.algorithm = readString(*sig, "algorithm", true, 32, e, sctx);
            p.signature.signatureHex = util::toLower(readString(*sig, "signature", true, 256, e, sctx));
            if (!p.signature.keyId.empty() && !isSimpleId(p.signature.keyId)) e.add(sctx + "keyId: invalid");
            if (!p.signature.signatureHex.empty() && !util::isLowerHex(p.signature.signatureHex, p.signature.signatureHex.size()))
                e.add(sctx + "signature: must be hex");
        }
    }

    if (const Value* deps = v.find("dependencies"); deps && !deps->isNull()) {
        if (!deps->isArray() || deps->items().size() > 64) e.add(ctx + "dependencies: must be an array (max 64)");
        else for (const Value& d : deps->items()) {
            if (!d.isObject()) { e.add(ctx + "dependencies: entries must be objects"); continue; }
            Dependency dep;
            dep.appId = readString(d, "appId", true, 128, e, ctx + "dependencies.");
            dep.minVersion = readString(d, "minVersion", false, 100, e, ctx + "dependencies.");
            if (!dep.appId.empty() && !isValidAppId(dep.appId)) e.add(ctx + "dependencies: invalid appId");
            if (!dep.minVersion.empty() && !parseVersion(dep.minVersion)) e.add(ctx + "dependencies: invalid minVersion");
            p.dependencies.push_back(std::move(dep));
        }
    }

    if (const Value* w = v.find("withdrawn"); w && !w->isNull()) {
        auto b = w->boolean();
        if (!b) e.add(ctx + "withdrawn: must be a boolean");
        else p.withdrawn = *b;
    }
    p.withdrawnReason = readString(v, "withdrawnReason", false, 500, e, ctx);

    if (e.list.size() != before) return std::nullopt;
    return p;
}

std::optional<AppEntry> parseApp(const Value& v, const Catalog& cat, Errs& e) {
    if (!v.isObject()) { e.add("must be an object"); return std::nullopt; }
    size_t before = e.list.size();
    AppEntry a;
    const std::string ctx;

    a.appId = readString(v, "appId", true, 128, e, ctx);
    if (!a.appId.empty() && !isValidAppId(a.appId)) e.add("appId: invalid identifier");
    a.name = readString(v, "name", true, 100, e, ctx);
    a.summary = readString(v, "summary", true, 200, e, ctx);
    a.description = readString(v, "description", false, 10000, e, ctx, true);
    a.icon = readString(v, "icon", false, 2048, e, ctx);
    if (!a.icon.empty()) { std::string why; if (!isAcceptableSourceUrl(a.icon, &why)) e.add("icon: " + why); }
    a.currentVersion = readString(v, "currentVersion", true, 100, e, ctx);
    if (!a.currentVersion.empty() && !parseVersion(a.currentVersion)) e.add("currentVersion: not a valid version");
    a.releaseNotes = readString(v, "releaseNotes", false, 10000, e, ctx, true);
    a.license = readString(v, "license", false, 200, e, ctx);
    a.privacyPolicy = readString(v, "privacyPolicy", false, 2048, e, ctx);
    a.sourceRepository = readString(v, "sourceRepository", false, 2048, e, ctx);
    a.publishedAt = readString(v, "publishedAt", false, 40, e, ctx);
    a.updatedAt = readString(v, "updatedAt", false, 40, e, ctx);
    if (!a.publishedAt.empty() && !isDateLike(a.publishedAt)) e.add("publishedAt: expected an ISO-8601 date");
    if (!a.updatedAt.empty() && !isDateLike(a.updatedAt)) e.add("updatedAt: expected an ISO-8601 date");

    if (const Value* pub = v.find("publisher"); pub && pub->isObject()) {
        a.publisher.id = readString(*pub, "id", true, 128, e, "publisher.");
        a.publisher.name = readString(*pub, "name", true, 100, e, "publisher.");
        a.publisher.website = readString(*pub, "website", false, 2048, e, "publisher.");
        if (!a.publisher.id.empty() && !isValidAppId(a.publisher.id)) e.add("publisher.id: invalid identifier");
    } else {
        e.add("publisher: required object");
    }

    a.screenshots = readStringList(v, "screenshots", 20, 2048, e, ctx);
    for (const auto& s : a.screenshots) {
        std::string why;
        if (!isAcceptableSourceUrl(s, &why)) { e.add("screenshots: " + why); break; }
    }
    a.keywords = readStringList(v, "keywords", 30, 50, e, ctx);
    a.categories = readStringList(v, "categories", 8, 63, e, ctx);
    if (a.categories.empty()) e.add("categories: at least one category is required");
    for (const auto& c : a.categories)
        if (!cat.findCategory(c)) e.add("categories: unknown category '" + c + "'");

    if (const Value* perms = v.find("permissions"); perms && !perms->isNull()) {
        if (!perms->isArray() || perms->items().size() > 64) e.add("permissions: must be an array (max 64)");
        else for (const Value& pv : perms->items()) {
            if (!pv.isObject()) { e.add("permissions: entries must be objects"); continue; }
            Permission perm;
            perm.id = readString(pv, "id", true, 63, e, "permissions.");
            perm.reason = readString(pv, "reason", false, 300, e, "permissions.");
            if (!perm.id.empty() && !isSimpleId(perm.id)) e.add("permissions: invalid id");
            a.permissions.push_back(std::move(perm));
        }
    }

    if (const Value* f = v.find("featured"); f && !f->isNull()) {
        auto b = f->boolean();
        if (!b) e.add("featured: must be a boolean");
        else a.featured = *b;
    }

    if (const Value* r = v.find("rating"); r && !r->isNull()) {
        if (!r->isObject()) e.add("rating: must be an object");
        else {
            const Value* avg = r->find("average");
            const Value* cnt = r->find("count");
            auto avgN = avg ? avg->number() : std::nullopt;
            auto cntN = cnt ? cnt->unsignedInteger() : std::nullopt;
            if (!avgN || !cntN || *avgN < 0 || *avgN > 5 || *cntN == 0) e.add("rating: needs average in [0,5] and a positive count");
            else a.rating = Rating{*avgN, *cntN};
        }
    }

    const Value* pkgs = v.find("packages");
    if (!pkgs || !pkgs->isArray() || pkgs->items().empty()) e.add("packages: at least one package is required");
    else if (pkgs->items().size() > kMaxPackagesPerApp) e.add("packages: too many packages");
    else {
        std::set<std::string> seen;
        size_t i = 0;
        for (const Value& pv : pkgs->items()) {
            auto p = parsePackage(pv, e, "packages[" + std::to_string(i++) + "].");
            if (!p) continue;
            if (!seen.insert(p->id).second) { e.add("packages: duplicate package id '" + p->id + "'"); continue; }
            a.packages.push_back(std::move(*p));
        }
    }

    if (e.list.size() != before) return std::nullopt;
    return a;
}

}  // namespace

bool isValidAppId(std::string_view id) {
    if (id.empty() || id.size() > 128) return false;
    size_t segments = 0;
    size_t start = 0;
    while (true) {
        size_t dot = id.find('.', start);
        std::string_view seg = id.substr(start, dot == std::string_view::npos ? std::string_view::npos : dot - start);
        if (!isIdentifierSegment(seg, false)) return false;
        ++segments;
        if (dot == std::string_view::npos) break;
        start = dot + 1;
    }
    return segments >= 2;
}

bool isAcceptableSourceUrl(std::string_view url, std::string* reason) {
    auto no = [&](const char* r) { if (reason) *reason = r; return false; };
    if (url.empty() || url.size() > 2048) return no("empty or overlong URL");
    if (util::hasControlChars(url) || url.find(' ') != std::string_view::npos || url.find('\\') != std::string_view::npos)
        return no("URL contains forbidden characters");

    if (url.substr(0, 8) == "https://") {
        std::string_view rest = url.substr(8);
        size_t slash = rest.find('/');
        std::string_view authority = rest.substr(0, slash);
        if (authority.empty()) return no("https URL has no host");
        if (authority.find('@') != std::string_view::npos) return no("URL credentials are not allowed");
        return true;
    }
    if (url.substr(0, 7) == "file://") {
        if (url.size() <= 8 || url[7] != '/') return no("file URL must be absolute");
        if (url.find("..") != std::string_view::npos) return no("file URL contains '..'");
        return true;
    }
    if (url.find("://") != std::string_view::npos || url.substr(0, 5) == "data:" || url.substr(0, 11) == "javascript:")
        return no("only https:// and file:// sources are allowed");
    if (!util::isSafeRelativePath(url)) return no("unsafe relative path");
    return true;
}

CatalogLoadResult parseCatalog(std::string_view text, const std::filesystem::path& baseDir) {
    CatalogLoadResult result;
    auto fatal = [&](std::string msg) {
        result.catalog.reset();
        result.issues.push_back({"catalog", std::move(msg)});
        return result;
    };

    Value root;
    try {
        root = json::parse(text);
    } catch (const json::ParseError& ex) {
        return fatal(ex.what());
    }
    if (!root.isObject()) return fatal("top-level value must be an object");

    const Value* sv = root.find("schemaVersion");
    auto schema = sv ? sv->unsignedInteger() : std::nullopt;
    if (!schema || *schema != static_cast<uint64_t>(kCatalogSchemaVersion))
        return fatal("unsupported or missing schemaVersion (expected " + std::to_string(kCatalogSchemaVersion) + ")");

    Catalog cat;
    cat.schemaVersion = kCatalogSchemaVersion;
    cat.baseDir = baseDir.string();
    if (const Value* g = root.find("generatedAt"); g && g->string()) cat.generatedAt = *g->string();

    const Value* cats = root.find("categories");
    const Value* apps = root.find("apps");
    if (!cats || !cats->isArray()) return fatal("'categories' must be an array");
    if (!apps || !apps->isArray()) return fatal("'apps' must be an array");
    if (apps->items().size() > kMaxApps) return fatal("too many apps");

    std::set<std::string> catIds;
    for (const Value& c : cats->items()) {
        const Value* id = c.find("id");
        const Value* name = c.find("name");
        if (!id || !id->string() || !name || !name->string() || !isSimpleId(*id->string()) || name->string()->empty() ||
            name->string()->size() > 60 || util::hasControlChars(*name->string()) || !catIds.insert(*id->string()).second) {
            result.issues.push_back({"categories", "skipped an invalid or duplicate category"});
            continue;
        }
        cat.categories.push_back({*id->string(), *name->string()});
    }

    std::set<std::string> seenApps;
    size_t index = 0;
    for (const Value& av : apps->items()) {
        Errs errs;
        auto app = parseApp(av, cat, errs);
        std::string label = "apps[" + std::to_string(index) + "]";
        if (const Value* id = av.find("appId"); id && id->string() && id->string()->size() < 129) label += " (" + *id->string() + ")";
        ++index;
        if (!app) {
            for (auto& m : errs.list) result.issues.push_back({label, m});
            continue;
        }
        if (!seenApps.insert(app->appId).second) {
            result.issues.push_back({label, "duplicate appId; entry ignored"});
            continue;
        }
        cat.apps.push_back(std::move(*app));
    }

    result.catalog = std::move(cat);
    return result;
}

CatalogLoadResult loadCatalogFile(const std::filesystem::path& file) {
    std::string err;
    auto text = util::readFile(file, kMaxCatalogBytes, &err);
    if (!text) {
        CatalogLoadResult r;
        r.issues.push_back({"catalog", err});
        return r;
    }
    std::filesystem::path base = file.has_parent_path() ? file.parent_path() : std::filesystem::path(".");
    return parseCatalog(*text, base);
}

}  // namespace exile
