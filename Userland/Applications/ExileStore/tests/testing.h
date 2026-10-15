#pragma once
// Tiny self-registering test framework plus fixtures that build REAL signed
// packages and catalogs, so installer/trust tests exercise the real code paths.

#include <chrono>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <functional>
#include <iostream>
#include <memory>
#include <optional>
#include <sstream>
#include <string>
#include <vector>

#include "core/backend.h"
#include "core/catalog.h"
#include "core/device.h"
#include "core/expkg.h"
#include "core/installer.h"
#include "core/library.h"
#include "core/sha256.h"
#include "core/transport.h"
#include "core/trust.h"
#include "core/util.h"

#ifdef EXILE_HAS_OPENSSL
#include <openssl/evp.h>
#endif

namespace testing {

struct Failure {
    std::string message;
};

struct Case {
    std::string name;
    std::function<void()> fn;
};

inline std::vector<Case>& registry() {
    static std::vector<Case> r;
    return r;
}

struct Registrar {
    Registrar(const char* name, std::function<void()> fn) { registry().push_back({name, std::move(fn)}); }
};

template <typename T>
std::string show(const T& v) {
    if constexpr (std::is_enum_v<T>) {
        return std::to_string(static_cast<long long>(v));
    } else if constexpr (std::is_convertible_v<T, std::string>) {
        return "\"" + std::string(v) + "\"";
    } else {
        std::ostringstream os;
        os << v;
        return os.str();
    }
}

}  // namespace testing

#define TEST(name)                                         \
    static void name();                                    \
    static testing::Registrar registrar_##name(#name, name); \
    static void name()

#define CHECK(cond)                                                                                      \
    do {                                                                                                 \
        if (!(cond)) throw testing::Failure{std::string(__FILE__) + ":" + std::to_string(__LINE__) + ": CHECK(" #cond ")"}; \
    } while (0)

#define CHECK_EQ(a, b)                                                                                    \
    do {                                                                                                  \
        const auto va_ = (a); /* by value: operands are often temporaries or elements of temporaries */   \
        const auto vb_ = (b);                                                                             \
        if (!(va_ == vb_))                                                                                \
            throw testing::Failure{std::string(__FILE__) + ":" + std::to_string(__LINE__) + ": " #a " == " #b \
                                   "  (got " + testing::show(va_) + " vs " + testing::show(vb_) + ")"};   \
    } while (0)

#define CHECK_CONTAINS(haystack, needle)                                                                  \
    do {                                                                                                  \
        std::string h_ = (haystack);                                                                      \
        std::string n_ = (needle);                                                                        \
        if (h_.find(n_) == std::string::npos)                                                             \
            throw testing::Failure{std::string(__FILE__) + ":" + std::to_string(__LINE__) + ": expected to find \"" + n_ + "\" in \"" + h_ + "\""}; \
    } while (0)

#define CHECK_NOT_CONTAINS(haystack, needle)                                                              \
    do {                                                                                                  \
        std::string h_ = (haystack);                                                                      \
        std::string n_ = (needle);                                                                        \
        if (h_.find(n_) != std::string::npos)                                                             \
            throw testing::Failure{std::string(__FILE__) + ":" + std::to_string(__LINE__) + ": did not expect \"" + n_ + "\" in \"" + h_ + "\""}; \
    } while (0)

namespace testing {

namespace fs = std::filesystem;

class TempDir {
public:
    TempDir() {
        static int counter = 0;
        std::error_code ec;
        path_ = fs::temp_directory_path(ec) / ("exile-store-test-" + std::to_string(static_cast<long long>(std::chrono::steady_clock::now().time_since_epoch().count())) + "-" + std::to_string(++counter));
        fs::remove_all(path_, ec);
        fs::create_directories(path_, ec);
    }
    ~TempDir() {
        std::error_code ec;
        fs::remove_all(path_, ec);
    }
    TempDir(const TempDir&) = delete;
    TempDir& operator=(const TempDir&) = delete;
    const fs::path& path() const { return path_; }
    fs::path operator/(const std::string& s) const { return path_ / s; }

private:
    fs::path path_;
};

inline void writeFile(const fs::path& p, const std::string& data) {
    std::error_code ec;
    fs::create_directories(p.parent_path(), ec);
    std::ofstream out(p, std::ios::binary | std::ios::trunc);
    out.write(data.data(), static_cast<std::streamsize>(data.size()));
}

inline std::string readAll(const fs::path& p) {
    auto s = exile::util::readFile(p, 1ull << 30);
    return s ? *s : std::string();
}

struct Keys {
    std::string privHex, pubHex;
};

#ifdef EXILE_HAS_OPENSSL
inline Keys makeKeys() {
    EVP_PKEY_CTX* ctx = EVP_PKEY_CTX_new_id(EVP_PKEY_ED25519, nullptr);
    EVP_PKEY* key = nullptr;
    EVP_PKEY_keygen_init(ctx);
    EVP_PKEY_keygen(ctx, &key);
    uint8_t priv[32], pub[32];
    size_t pl = 32, ul = 32;
    EVP_PKEY_get_raw_private_key(key, priv, &pl);
    EVP_PKEY_get_raw_public_key(key, pub, &ul);
    EVP_PKEY_free(key);
    EVP_PKEY_CTX_free(ctx);
    return {exile::util::hexEncode(priv, 32), exile::util::hexEncode(pub, 32)};
}

inline std::string sign(const std::string& privHex, const std::string& message) {
    auto priv = *exile::util::hexDecode(privHex);
    EVP_PKEY* key = EVP_PKEY_new_raw_private_key(EVP_PKEY_ED25519, nullptr, priv.data(), priv.size());
    EVP_MD_CTX* ctx = EVP_MD_CTX_new();
    uint8_t sig[64];
    size_t sl = 64;
    EVP_DigestSignInit(ctx, nullptr, nullptr, nullptr, key);
    EVP_DigestSign(ctx, sig, &sl, reinterpret_cast<const unsigned char*>(message.data()), message.size());
    EVP_MD_CTX_free(ctx);
    EVP_PKEY_free(key);
    return exile::util::hexEncode(sig, 64);
}
#endif

// What a test wants in a package.
struct PkgSpec {
    std::string appId = "org.test.app";
    std::string name = "Test App";
    std::string version = "1.0.0";
    std::string publisher = "org.test";
    std::string pkgId = "main";
    std::string payload = "hello world";
    std::string devices = "\"desktop\"";
    std::string archs = "\"x86_64\"";
    std::string category = "utilities";
    std::string extraPkg;   // raw JSON fragment appended inside the package, e.g. ,"minimumOsVersion":"2.0.0"
    std::string extraApp;   // raw JSON fragment appended inside the app
    bool signWithUntrustedKey = false;
    bool tamperPackageAfterSigning = false;
    bool omitSignature = false;
    std::string keyIdOverride;
    std::string summary = "A test app.";
};

struct BuiltApp {
    std::string json;
    fs::path file;
    std::string sha256;
    uint64_t size = 0;
};

struct Fixture {
    TempDir dir;
    Keys good;
    Keys bad;
    exile::TrustStore trust;
    std::unique_ptr<exile::ISignatureVerifier> verifier = exile::makeDefaultVerifier();
    exile::DeviceProfile device;

    Fixture() {
#ifdef EXILE_HAS_OPENSSL
        good = makeKeys();
        bad = makeKeys();
        exile::TrustedKey k;
        k.keyId = "test-key";
        k.publisherId = "org.test";
        k.algorithm = "ed25519";
        k.publicKey = *exile::util::hexDecode(good.pubHex);
        trust.add(std::move(k));
#endif
        device.deviceClass = exile::DeviceClass::Desktop;
        device.arch = exile::Arch::X86_64;
        device.osVersion = "1.0.0";
    }

    BuiltApp build(const PkgSpec& s) {
        BuiltApp b;
        fs::path stage = dir / ("stage-" + s.appId + "-" + s.version);
        writeFile(stage / "README.txt", s.payload);
        writeFile(stage / "bin" / "run.txt", "run " + s.appId);
        std::string name = s.appId + "-" + s.version + ".expkg";
        b.file = dir / "pkgs" / name;
        std::string err;
        exile::packExpkg(stage, b.file, err);
        b.sha256 = *exile::sha256FileHex(b.file);
        b.size = fs::file_size(b.file);
        std::string sig;
#ifdef EXILE_HAS_OPENSSL
        sig = sign(s.signWithUntrustedKey ? bad.privHex : good.privHex,
                   exile::signingPayload(s.appId, s.version, s.pkgId, b.sha256, b.size));
#endif
        if (s.tamperPackageAfterSigning) {
            // Same size, different bytes: only the digest check can notice.
            std::string data = readAll(b.file);
            data.back() = static_cast<char>(data.back() ^ 0x01);
            writeFile(b.file, data);
        }
        std::string keyId = !s.keyIdOverride.empty() ? s.keyIdOverride : (s.signWithUntrustedKey ? "other-key" : "test-key");
        std::ostringstream o;
        o << "{\"appId\":\"" << s.appId << "\",\"name\":\"" << s.name << "\",\"summary\":\"" << s.summary
          << "\",\"publisher\":{\"id\":\"" << s.publisher << "\",\"name\":\"Test Publisher\"},\"categories\":[\"" << s.category
          << "\"],\"currentVersion\":\"" << s.version << "\"" << s.extraApp << ",\"packages\":[{\"id\":\"" << s.pkgId
          << "\",\"devices\":[" << s.devices << "],\"architectures\":[" << s.archs << "],\"url\":\"pkgs/" << name
          << "\",\"sha256\":\"" << b.sha256 << "\",\"downloadSize\":" << b.size << ",\"packageSize\":" << b.size * 2;
        if (!s.omitSignature)
            o << ",\"signature\":{\"keyId\":\"" << keyId << "\",\"algorithm\":\"ed25519\",\"signature\":\"" << sig << "\"}";
        o << s.extraPkg << "}]}";
        b.json = o.str();
        return b;
    }

    static std::string catalogJson(const std::vector<std::string>& apps) {
        std::string out = "{\"schemaVersion\":1,\"generatedAt\":\"2026-01-01\",\"categories\":["
                          "{\"id\":\"utilities\",\"name\":\"Utilities\"},{\"id\":\"games\",\"name\":\"Games\"},"
                          "{\"id\":\"system-tools\",\"name\":\"System Tools\"}],\"apps\":[";
        for (size_t i = 0; i < apps.size(); ++i) out += (i ? "," : "") + apps[i];
        return out + "]}";
    }

    exile::Catalog catalog(const std::vector<std::string>& apps) {
        auto r = exile::parseCatalog(catalogJson(apps), dir.path());
        if (!r.catalog) throw Failure{"fixture catalog failed to parse: " + (r.issues.empty() ? std::string("?") : r.issues[0].message)};
        for (const auto& i : r.issues) throw Failure{"fixture catalog issue: " + i.where + ": " + i.message};
        return std::move(*r.catalog);
    }
};

// Wraps the real LocalTransport but can misbehave on demand.
class FlakyTransport final : public exile::ITransport {
public:
    explicit FlakyTransport(exile::ITransport& inner) : inner_(inner) {}
    bool failNext = false;          // fail with an error before sending anything
    bool interruptNext = false;     // write half the file, then report cancellation
    bool corruptPartial = false;    // put garbage in the partial file before a resume
    int calls = 0;

    exile::TransferResult fetch(const std::string& source, const fs::path& part, bool resume,
                                const exile::ProgressFn& progress, const exile::CancelToken& cancel) override {
        ++calls;
        if (failNext) {
            failNext = false;
            exile::TransferResult r;
            r.error = "connection reset";
            return r;
        }
        if (interruptNext) {
            interruptNext = false;
            exile::CancelToken stop;
            uint64_t seen = 0;
            exile::TransferResult r = inner_.fetch(source, part, resume,
                [&](uint64_t d, uint64_t t) {
                    seen = d;
                    if (d * 2 >= t) stop.cancel();
                    if (progress) progress(d, t);
                }, stop);
            (void)seen;
            return r;
        }
        if (corruptPartial) {
            corruptPartial = false;
            writeFile(part, "garbage");
        }
        return inner_.fetch(source, part, resume, progress, cancel);
    }

private:
    exile::ITransport& inner_;
};

// Backend double that can fail or lie, used to prove the store trusts only real confirmation.
class ScriptedBackend final : public exile::IPackageBackend {
public:
    explicit ScriptedBackend(exile::IPackageBackend& inner) : inner_(inner) {}
    bool failInstall = false;
    bool installButDoNotConfirm = false;
    bool failUninstall = false;
    int installCalls = 0;
    std::string name() const override { return "scripted"; }
    bool canLaunch() const override { return inner_.canLaunch(); }
    fs::path storagePath() const override { return inner_.storagePath(); }
    exile::BackendResult install(const exile::InstallRequest& r) override {
        ++installCalls;
        if (failInstall) return {false, "simulated installer failure"};
        if (installButDoNotConfirm) return {true, {}};  // claims success, installs nothing
        return inner_.install(r);
    }
    exile::BackendResult uninstall(const std::string& id) override {
        if (failUninstall) return {false, "simulated failure"};
        return inner_.uninstall(id);
    }
    bool isInstalled(const std::string& id, const std::string& v) const override { return inner_.isInstalled(id, v); }
    exile::BackendResult launch(const std::string& id) override { return inner_.launch(id); }
    void recoverInterrupted() override { inner_.recoverInterrupted(); }

private:
    exile::IPackageBackend& inner_;
};

}  // namespace testing
