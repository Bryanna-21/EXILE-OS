/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

// expkg-tool: developer utility for building development packages and the
// sample catalog. It is NOT shipped to end users and is not a publishing
// pipeline: the dev signing key is generated fresh on each run and the private
// half is written only where you ask for it.
//
//   expkg-tool pack <dir> <out.expkg>
//   expkg-tool sha256 <file>
//   expkg-tool genkey <private-key-file>        (prints the public key as hex)
//   expkg-tool sign <private-key-file> <appId> <version> <packageId> <sha256> <size>
//   expkg-tool devcatalog <outDir> [<private-key-out>]

#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>
#include <vector>

#include "../src/core/expkg.h"
#include "../src/core/json.h"
#include "../src/core/sha256.h"
#include "../src/core/trust.h"
#include "../src/core/util.h"

#ifdef EXILE_HAS_OPENSSL
#    include <openssl/evp.h>
#endif

namespace fs = std::filesystem;
using namespace exile;
using json::Value;

namespace {

#ifdef EXILE_HAS_OPENSSL
bool genKey(std::string& privHex, std::string& pubHex)
{
    EVP_PKEY_CTX* ctx = EVP_PKEY_CTX_new_id(EVP_PKEY_ED25519, nullptr);
    EVP_PKEY* key = nullptr;
    bool ok = ctx && EVP_PKEY_keygen_init(ctx) == 1 && EVP_PKEY_keygen(ctx, &key) == 1;
    if (ok) {
        uint8_t priv[32], pub[32];
        size_t pl = sizeof priv, ul = sizeof pub;
        ok = EVP_PKEY_get_raw_private_key(key, priv, &pl) == 1 && EVP_PKEY_get_raw_public_key(key, pub, &ul) == 1 && pl == 32 && ul == 32;
        if (ok) {
            privHex = util::hexEncode(priv, 32);
            pubHex = util::hexEncode(pub, 32);
        }
    }
    EVP_PKEY_free(key);
    EVP_PKEY_CTX_free(ctx);
    return ok;
}

bool signMessage(std::string const& privHex, std::string const& message, std::string& sigHex)
{
    auto priv = util::hexDecode(privHex);
    if (!priv || priv->size() != 32)
        return false;
    EVP_PKEY* key = EVP_PKEY_new_raw_private_key(EVP_PKEY_ED25519, nullptr, priv->data(), priv->size());
    if (!key)
        return false;
    EVP_MD_CTX* ctx = EVP_MD_CTX_new();
    uint8_t sig[64];
    size_t sl = sizeof sig;
    bool ok = ctx && EVP_DigestSignInit(ctx, nullptr, nullptr, nullptr, key) == 1 && EVP_DigestSign(ctx, sig, &sl, reinterpret_cast<unsigned char const*>(message.data()), message.size()) == 1 && sl == 64;
    if (ok)
        sigHex = util::hexEncode(sig, 64);
    EVP_MD_CTX_free(ctx);
    EVP_PKEY_free(key);
    return ok;
}
#endif

bool writeText(fs::path const& p, std::string const& text)
{
    std::string err;
    if (!util::writeFileAtomic(p, text, err)) {
        std::cerr << err << "\n";
        return false;
    }
    return true;
}

struct SampleApp {
    std::string appId, name, summary, description, category;
    std::vector<std::string> keywords;
    std::string version = "1.0.0";
    std::string kind = "native";
    std::vector<std::string> devices = { "desktop" };
    std::vector<std::string> archs = { "x86_64", "aarch64" };
    std::string minOs;
    std::string channel = "stable";
    std::vector<std::pair<std::string, std::string>> perms; // id, reason
    std::vector<std::string> deps;
    bool featured = false;
    bool trustedKey = true;
    std::string updatedAt = "2026-10-01";
    std::string releaseNotes = "Initial development release.";
};

Value strArray(std::vector<std::string> const& v)
{
    Value a = Value::makeArray();
    for (auto const& s : v)
        a.push(Value::makeString(s));
    return a;
}

#ifdef EXILE_HAS_OPENSSL
bool buildCatalog(fs::path const& out, std::string const& tag, std::string const& version, bool newer,
    std::string const& goodPriv, std::string const& badPriv, std::string const& goodPub)
{
    std::string const publisherId = "org.exile.devsamples";
    std::vector<SampleApp> apps;
    {
        SampleApp a;
        a.appId = "org.exile.sample.notes";
        a.name = "Sample Notes";
        a.summary = "A tiny notes app used to try out the store.";
        a.description = "This is a development sample, not a real product. It exists so every part of the store (compatibility, permissions, installing, updating) has something honest to work with.";
        a.category = "productivity";
        a.keywords = { "notes", "text", "write" };
        a.featured = true;
        a.perms = { { "documents", "Saves your notes as files." } };
        a.version = version;
        if (newer)
            a.releaseNotes = "Development update used to test the update flow.";
        apps.push_back(a);
    }
    {
        SampleApp a;
        a.appId = "org.exile.sample.codecs";
        a.name = "Sample Codecs";
        a.summary = "Shared media library that Sample Player needs.";
        a.description = "A development sample library used to test dependency checks.";
        a.category = "system-tools";
        a.keywords = { "library", "codec" };
        a.devices = { "desktop", "mobile", "tablet" };
        apps.push_back(a);
    }
    {
        SampleApp a;
        a.appId = "org.exile.sample.player";
        a.name = "Sample Player";
        a.summary = "Plays audio. Needs Sample Codecs; works on desktop and mobile.";
        a.description = "A development sample with one shared listing and two builds, showing how one app can serve several device types.";
        a.category = "music-and-audio";
        a.keywords = { "music", "audio", "player" };
        a.devices = { "desktop", "mobile" };
        a.featured = true;
        a.perms = { { "network", "Streams from the address you give it." }, { "microphone", "Records audio if you choose to." } };
        a.deps = { "org.exile.sample.codecs" };
        apps.push_back(a);
    }
    {
        SampleApp a;
        a.appId = "org.exile.sample.pocketclock";
        a.name = "Sample Pocket Clock";
        a.summary = "A phone-only clock, to show how incompatible apps are handled.";
        a.description = "Built for phones only. On a desktop it can be browsed but not installed.";
        a.category = "utilities";
        a.keywords = { "clock", "alarm" };
        a.devices = { "mobile" };
        a.archs = { "aarch64" };
        a.perms = { { "notifications", "Rings alarms." }, { "background", "Alarms must fire while closed." } };
        apps.push_back(a);
    }
    {
        SampleApp a;
        a.appId = "org.exile.sample.webtool";
        a.name = "Sample Web Tool";
        a.summary = "A web app, labelled as one rather than passed off as native.";
        a.description = "Delivered as a web app wrapper. The store says so on its page.";
        a.category = "internet-and-browsing";
        a.keywords = { "web", "pwa" };
        a.kind = "web";
        a.devices = { "any" }; // universal: adapts to any screen
        a.archs = { "any" };
        apps.push_back(a);
    }
    {
        SampleApp a;
        a.appId = "org.exile.sample.windows-tool";
        a.name = "Sample Compat Tool";
        a.summary = "Runs through a compatibility layer on desktop.";
        a.description = "Shows how an app delivered through a verified compatibility layer is labelled.";
        a.category = "utilities";
        a.keywords = { "compat", "windows" };
        a.kind = "compat-layer";
        a.archs = { "x86_64" };
        apps.push_back(a);
    }
    {
        SampleApp a;
        a.appId = "org.exile.sample.untrusted";
        a.name = "Sample Untrusted Publisher";
        a.summary = "Signed by a key this device does not trust; install is refused.";
        a.description = "Exists to demonstrate that a package with a signature from an unknown key is rejected.";
        a.category = "utilities";
        a.keywords = { "untrusted", "test" };
        a.trustedKey = false;
        apps.push_back(a);
    }
    {
        SampleApp a;
        a.appId = "org.exile.sample.futureos";
        a.name = "Sample Future OS App";
        a.summary = "Needs a newer Exile OS than this device has.";
        a.description = "Shows the 'OS too old' explanation.";
        a.category = "development";
        a.keywords = { "future" };
        a.minOs = "9.0.0";
        apps.push_back(a);
    }
    {
        SampleApp a;
        a.appId = "org.exile.sample.betaonly";
        a.name = "Sample Beta Tool";
        a.summary = "Only in the beta channel; hidden unless --beta is set.";
        a.description = "Beta channel sample.";
        a.category = "development";
        a.channel = "beta";
        apps.push_back(a);
    }

    fs::path pkgDir = out / "packages";
    std::error_code ec;
    fs::create_directories(pkgDir, ec);

    Value root = Value::makeObject();
    root.set("schemaVersion", Value::makeNumber(1));
    root.set("generatedAt", Value::makeString(newer ? "2026-10-10" : "2026-10-09"));
    Value cats = Value::makeArray();
    std::pair<char const*, char const*> const catList[] = {
        { "productivity", "Productivity" }, { "education", "Education" }, { "communication", "Communication" },
        { "internet-and-browsing", "Internet and Browsing" }, { "development", "Development" },
        { "creativity-and-design", "Creativity and Design" }, { "photos-and-video", "Photos and Video" },
        { "music-and-audio", "Music and Audio" }, { "games", "Games" }, { "security-and-privacy", "Security and Privacy" },
        { "utilities", "Utilities" }, { "system-tools", "System Tools" }
    };
    for (auto const& c : catList) {
        Value o = Value::makeObject();
        o.set("id", Value::makeString(c.first));
        o.set("name", Value::makeString(c.second));
        cats.push(std::move(o));
    }
    root.set("categories", std::move(cats));

    Value appArr = Value::makeArray();
    for (SampleApp const& a : apps) {
        // One payload per app+version; text only, clearly not a real program.
        fs::path stage = out / ".stage" / (a.appId + "-" + a.version);
        fs::remove_all(stage, ec);
        fs::create_directories(stage / "bin", ec);
        writeText(stage / "README.txt", a.name + " " + a.version + "\nThis is a development sample payload. It is not a real application.\n");
        writeText(stage / "bin" / "sample.txt", "placeholder for " + a.appId + "\n");
        std::string pkgName = a.appId + "-" + a.version + ".expkg";
        std::string perr;
        if (!packExpkg(stage, pkgDir / pkgName, perr)) {
            std::cerr << perr << "\n";
            return false;
        }
        fs::remove_all(out / ".stage", ec);

        auto digest = sha256FileHex(pkgDir / pkgName);
        uint64_t size = fs::file_size(pkgDir / pkgName, ec);

        Value pub = Value::makeObject();
        pub.set("id", Value::makeString(publisherId));
        pub.set("name", Value::makeString("Exile Development Samples"));
        Value app = Value::makeObject();
        app.set("appId", Value::makeString(a.appId));
        app.set("name", Value::makeString(a.name));
        app.set("summary", Value::makeString(a.summary));
        app.set("description", Value::makeString(a.description));
        app.set("publisher", std::move(pub));
        app.set("categories", strArray({ a.category }));
        app.set("keywords", strArray(a.keywords));
        app.set("currentVersion", Value::makeString(a.version));
        app.set("releaseNotes", Value::makeString(a.releaseNotes));
        app.set("license", Value::makeString("Sample data, no license"));
        app.set("publishedAt", Value::makeString("2026-09-01"));
        app.set("updatedAt", Value::makeString(newer && a.appId == "org.exile.sample.notes" ? "2026-10-10" : a.updatedAt));
        if (a.featured)
            app.set("featured", Value::makeBool(true));
        Value perms = Value::makeArray();
        for (auto const& [id, reason] : a.perms) {
            Value p = Value::makeObject();
            p.set("id", Value::makeString(id));
            p.set("reason", Value::makeString(reason));
            perms.push(std::move(p));
        }
        app.set("permissions", std::move(perms));

        std::vector<std::pair<std::string, std::vector<std::string>>> builds; // package id, devices
        if (a.appId == "org.exile.sample.player")
            builds = { { "desktop", { "desktop" } }, { "mobile", { "mobile" } } };
        else
            builds = { { "main", a.devices } };

        Value pkgs = Value::makeArray();
        for (auto const& [pid, devs] : builds) {
            Value p = Value::makeObject();
            p.set("id", Value::makeString(pid));
            p.set("kind", Value::makeString(a.kind));
            p.set("devices", strArray(devs));
            p.set("architectures", strArray(a.archs));
            if (!a.minOs.empty())
                p.set("minimumOsVersion", Value::makeString(a.minOs));
            p.set("channel", Value::makeString(a.channel));
            p.set("url", Value::makeString("packages/" + pkgName));
            p.set("sha256", Value::makeString(*digest));
            p.set("downloadSize", Value::makeNumber(static_cast<double>(size)));
            p.set("packageSize", Value::makeNumber(static_cast<double>(size) * 2));
            if (!a.deps.empty()) {
                Value deps = Value::makeArray();
                for (auto const& d : a.deps) {
                    Value dv = Value::makeObject();
                    dv.set("appId", Value::makeString(d));
                    dv.set("minVersion", Value::makeString("1.0.0"));
                    deps.push(std::move(dv));
                }
                p.set("dependencies", std::move(deps));
            }
            std::string sigHex;
            std::string payload = signingPayload(a.appId, a.version, pid, *digest, size);
            if (!signMessage(a.trustedKey ? goodPriv : badPriv, payload, sigHex)) {
                std::cerr << "signing failed\n";
                return false;
            }
            Value sig = Value::makeObject();
            sig.set("keyId", Value::makeString(a.trustedKey ? "exile-dev-2026" : "unknown-key-1"));
            sig.set("algorithm", Value::makeString("ed25519"));
            sig.set("signature", Value::makeString(sigHex));
            p.set("signature", std::move(sig));
            pkgs.push(std::move(p));
        }
        app.set("packages", std::move(pkgs));
        appArr.push(std::move(app));
    }
    root.set("apps", std::move(appArr));
    (void)tag;
    (void)goodPub;
    return writeText(out / (newer ? "catalog-update.json" : "catalog.json"), json::dump(root));
}
#endif

int usage()
{
    std::cerr << "usage:\n"
                 "  expkg-tool pack <dir> <out.expkg>\n"
                 "  expkg-tool sha256 <file>\n"
                 "  expkg-tool genkey <private-key-file>\n"
                 "  expkg-tool sign <private-key-file> <appId> <version> <packageId> <sha256> <size>\n"
                 "  expkg-tool devcatalog <outDir> [<private-key-out>]\n";
    return 2;
}

} // namespace

int main(int argc, char** argv)
{
    if (argc < 2)
        return usage();
    std::string cmd = argv[1];

    if (cmd == "pack" && argc == 4) {
        std::string err;
        if (!packExpkg(argv[2], argv[3], err)) {
            std::cerr << err << "\n";
            return 1;
        }
        std::cout << "wrote " << argv[3] << "\n";
        return 0;
    }
    if (cmd == "sha256" && argc == 3) {
        std::string err;
        auto d = sha256FileHex(argv[2], &err);
        if (!d) {
            std::cerr << err << "\n";
            return 1;
        }
        std::cout << *d << "\n";
        return 0;
    }
#ifdef EXILE_HAS_OPENSSL
    if (cmd == "genkey" && argc == 3) {
        std::string priv, pub;
        if (!genKey(priv, pub)) {
            std::cerr << "key generation failed\n";
            return 1;
        }
        if (!writeText(argv[2], priv + "\n"))
            return 1;
        std::error_code ec;
        fs::permissions(argv[2], fs::perms::owner_read | fs::perms::owner_write, fs::perm_options::replace, ec);
        std::cout << pub << "\n";
        return 0;
    }
    if (cmd == "sign" && argc == 8) {
        auto keyText = util::readFile(argv[2], 4096);
        if (!keyText) {
            std::cerr << "cannot read key file\n";
            return 1;
        }
        std::string priv = keyText->substr(0, keyText->find('\n'));
        std::string sig;
        if (!signMessage(priv, signingPayload(argv[3], argv[4], argv[5], argv[6], std::stoull(argv[7])), sig)) {
            std::cerr << "signing failed\n";
            return 1;
        }
        std::cout << sig << "\n";
        return 0;
    }
    if (cmd == "devcatalog" && (argc == 3 || argc == 4)) {
        fs::path out = argv[2];
        std::string goodPriv, goodPub, badPriv, badPub;
        if (!genKey(goodPriv, goodPub) || !genKey(badPriv, badPub)) {
            std::cerr << "key generation failed\n";
            return 1;
        }

        std::error_code ec;
        fs::remove_all(out / "catalog", ec);
        fs::create_directories(out / "catalog", ec);
        if (!buildCatalog(out / "catalog", "v1", "1.0.0", false, goodPriv, badPriv, goodPub))
            return 1;
        // Same packages for everything except Sample Notes, which gets a newer version (to test updates).
        {
            fs::path updDir = out / "catalog-update";
            fs::remove_all(updDir, ec);
            if (!buildCatalog(updDir, "v2", "1.1.0", true, goodPriv, badPriv, goodPub))
                return 1;
            // Keep the catalog next to the packages it references.
            for (auto& e : fs::directory_iterator(updDir / "packages")) {
                fs::path dest = out / "catalog" / "packages" / e.path().filename();
                if (!fs::exists(dest, ec))
                    fs::copy_file(e.path(), dest, ec);
            }
            fs::copy_file(updDir / "catalog-update.json", out / "catalog" / "catalog-update.json", fs::copy_options::overwrite_existing, ec);
            fs::remove_all(updDir, ec);
        }

        Value trust = Value::makeObject();
        trust.set("schemaVersion", Value::makeNumber(1));
        Value keys = Value::makeArray();
        Value k = Value::makeObject();
        k.set("keyId", Value::makeString("exile-dev-2026"));
        k.set("publisherId", Value::makeString("org.exile.devsamples"));
        k.set("algorithm", Value::makeString("ed25519"));
        k.set("publicKey", Value::makeString(goodPub));
        keys.push(std::move(k));
        trust.set("keys", std::move(keys));
        fs::create_directories(out / "trust", ec);
        if (!writeText(out / "trust" / "trusted-keys.json", json::dump(trust)))
            return 1;

        if (argc == 4 && !writeText(argv[3], goodPriv + "\n"))
            return 1;
        std::cout << "development catalog written to " << out.string() << "\n";
        return 0;
    }
#else
    if (cmd == "genkey" || cmd == "sign" || cmd == "devcatalog") {
        std::cerr << "this build has no crypto backend (OpenSSL); " << cmd << " is unavailable\n";
        return 1;
    }
#endif
    return usage();
}
