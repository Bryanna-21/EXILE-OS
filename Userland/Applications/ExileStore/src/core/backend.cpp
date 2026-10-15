#include "core/backend.h"

#include <system_error>

#include "core/catalog.h"
#include "core/expkg.h"
#include "core/json.h"
#include "core/util.h"
#include "core/version.h"

namespace fs = std::filesystem;

namespace exile {

namespace {
constexpr const char* kMetaName = ".exile-install.json";

BackendResult fail(std::string msg) { return {false, std::move(msg)}; }
}  // namespace

LocalPrefixBackend::LocalPrefixBackend(fs::path prefix) : prefix_(std::move(prefix)) {
    std::error_code ec;
    fs::create_directories(prefix_, ec);
}

fs::path LocalPrefixBackend::installDir(const std::string& appId) const { return prefix_ / "apps" / appId / "current"; }

BackendResult LocalPrefixBackend::install(const InstallRequest& req) {
    // Defense in depth: the store validated these already, the backend does not rely on that.
    if (!isValidAppId(req.appId)) return fail("invalid application id");
    if (!parseVersion(req.version)) return fail("invalid version");

    std::string err;
    auto manifest = parseExpkg(req.packageFile, ExpkgLimits{}, err);
    if (!manifest) return fail("package rejected: " + err);
    for (const auto& e : manifest->entries)
        if (e.path.rfind(".exile-", 0) == 0) return fail("package rejected: reserved file name " + e.path);

    std::error_code ec;
    fs::path appDir = prefix_ / "apps" / req.appId;
    fs::path stage = prefix_ / "staging" / (req.appId + "-" + req.version);
    fs::remove_all(stage, ec);
    fs::create_directories(appDir, ec);
    if (ec) return fail("cannot create install folder: " + ec.message());

    if (!extractExpkg(req.packageFile, *manifest, stage, nullptr, err)) {
        fs::remove_all(stage, ec);
        return fail("could not unpack package: " + err);
    }

    json::Value meta = json::Value::makeObject();
    meta.set("appId", json::Value::makeString(req.appId));
    meta.set("version", json::Value::makeString(req.version));
    meta.set("packageId", json::Value::makeString(req.packageId));
    if (!util::writeFileAtomic(stage / kMetaName, json::dump(meta), err)) {
        fs::remove_all(stage, ec);
        return fail(err);
    }

    fs::path current = appDir / "current";
    fs::path previous = appDir / "previous";
    fs::remove_all(previous, ec);
    bool hadCurrent = fs::exists(current, ec);
    if (hadCurrent) {
        fs::rename(current, previous, ec);
        if (ec) { fs::remove_all(stage, ec); return fail("cannot replace the existing version: " + ec.message()); }
    }
    fs::rename(stage, current, ec);
    if (ec) {
        std::string why = ec.message();
        if (hadCurrent) fs::rename(previous, current, ec);  // roll back
        fs::remove_all(stage, ec);
        return fail("could not activate the new version (previous version restored): " + why);
    }
    if (hadCurrent) fs::remove_all(previous, ec);
    return {true, {}};
}

BackendResult LocalPrefixBackend::uninstall(const std::string& appId) {
    if (!isValidAppId(appId)) return fail("invalid application id");
    std::error_code ec;
    fs::path appDir = prefix_ / "apps" / appId;
    if (!fs::exists(appDir, ec)) return fail("not installed");
    fs::remove_all(appDir, ec);
    if (ec) return fail("could not remove files: " + ec.message());
    return {true, {}};
}

bool LocalPrefixBackend::isInstalled(const std::string& appId, const std::string& version) const {
    if (!isValidAppId(appId)) return false;
    auto text = util::readFile(installDir(appId) / kMetaName, 64 * 1024);
    if (!text) return false;
    try {
        json::Value v = json::parse(*text);
        const json::Value* id = v.find("appId");
        const json::Value* ver = v.find("version");
        return id && id->string() && *id->string() == appId && ver && ver->string() && *ver->string() == version;
    } catch (const json::ParseError&) {
        return false;
    }
}

BackendResult LocalPrefixBackend::launch(const std::string&) {
    return fail("The development backend cannot start apps; launching needs the Exile OS app service.");
}

void LocalPrefixBackend::recoverInterrupted() {
    std::error_code ec;
    fs::remove_all(prefix_ / "staging", ec);
    fs::path apps = prefix_ / "apps";
    if (!fs::is_directory(apps, ec)) return;
    for (const auto& dir : fs::directory_iterator(apps, ec)) {
        fs::path current = dir.path() / "current";
        fs::path previous = dir.path() / "previous";
        if (!fs::exists(previous, ec)) continue;
        if (!fs::exists(current, ec)) fs::rename(previous, current, ec);  // crash between the two renames
        else fs::remove_all(previous, ec);
    }
}

}  // namespace exile
