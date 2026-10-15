#include "core/trust.h"

#include "core/catalog.h"
#include "core/json.h"
#include "core/util.h"

#ifdef EXILE_HAS_OPENSSL
#include <openssl/evp.h>
#endif

namespace exile {

std::optional<TrustStore> TrustStore::parse(std::string_view text, std::string& error) {
    json::Value root;
    try {
        root = json::parse(text);
    } catch (const json::ParseError& ex) {
        error = ex.what();
        return std::nullopt;
    }
    const json::Value* sv = root.find("schemaVersion");
    if (!sv || sv->unsignedInteger() != 1u) {
        error = "trust store: unsupported or missing schemaVersion";
        return std::nullopt;
    }
    const json::Value* keys = root.find("keys");
    if (!keys || !keys->isArray()) {
        error = "trust store: 'keys' must be an array";
        return std::nullopt;
    }
    TrustStore store;
    for (const json::Value& kv : keys->items()) {
        const json::Value* id = kv.find("keyId");
        const json::Value* pub = kv.find("publisherId");
        const json::Value* alg = kv.find("algorithm");
        const json::Value* pk = kv.find("publicKey");
        if (!id || !id->string() || !pub || !pub->string() || !alg || !alg->string() || !pk || !pk->string()) {
            error = "trust store: every key needs keyId, publisherId, algorithm and publicKey";
            return std::nullopt;
        }
        if (*alg->string() != "ed25519") {
            error = "trust store: unsupported algorithm '" + *alg->string() + "'";
            return std::nullopt;
        }
        auto bytes = util::hexDecode(*pk->string());
        if (!bytes || bytes->size() != 32) {
            error = "trust store: publicKey for '" + *id->string() + "' must be 32 bytes of hex";
            return std::nullopt;
        }
        if (!isValidAppId(*pub->string())) {
            error = "trust store: invalid publisherId for '" + *id->string() + "'";
            return std::nullopt;
        }
        TrustedKey k;
        k.keyId = *id->string();
        k.publisherId = *pub->string();
        k.algorithm = *alg->string();
        k.publicKey = std::move(*bytes);
        if (const json::Value* r = kv.find("revoked"); r && r->boolean()) k.revoked = *r->boolean();
        if (store.find(k.keyId)) {
            error = "trust store: duplicate keyId '" + k.keyId + "'";
            return std::nullopt;
        }
        store.add(std::move(k));
    }
    return store;
}

std::optional<TrustStore> TrustStore::loadFile(const std::string& path, std::string& error) {
    auto text = util::readFile(path, 1 << 20, &error);
    if (!text) return std::nullopt;
    return parse(*text, error);
}

void TrustStore::add(TrustedKey key) { keys_.push_back(std::move(key)); }

const TrustedKey* TrustStore::find(std::string_view keyId) const {
    for (const auto& k : keys_)
        if (k.keyId == keyId) return &k;
    return nullptr;
}

#ifdef EXILE_HAS_OPENSSL
namespace {
class OpenSslVerifier final : public ISignatureVerifier {
public:
    bool available() const override { return true; }
    std::string name() const override { return "OpenSSL Ed25519"; }
    bool verifyEd25519(const std::vector<uint8_t>& pub, std::string_view msg, const std::vector<uint8_t>& sig,
                       std::string& error) const override {
        if (pub.size() != 32 || sig.size() != 64) { error = "bad key or signature length"; return false; }
        EVP_PKEY* key = EVP_PKEY_new_raw_public_key(EVP_PKEY_ED25519, nullptr, pub.data(), pub.size());
        if (!key) { error = "could not load public key"; return false; }
        EVP_MD_CTX* ctx = EVP_MD_CTX_new();
        bool ok = false;
        if (ctx && EVP_DigestVerifyInit(ctx, nullptr, nullptr, nullptr, key) == 1 &&
            EVP_DigestVerify(ctx, sig.data(), sig.size(), reinterpret_cast<const unsigned char*>(msg.data()), msg.size()) == 1)
            ok = true;
        else
            error = "signature does not match";
        if (ctx) EVP_MD_CTX_free(ctx);
        EVP_PKEY_free(key);
        return ok;
    }
};
}  // namespace
#endif

namespace {
class UnavailableVerifier final : public ISignatureVerifier {
public:
    bool available() const override { return false; }
    std::string name() const override { return "none (built without a crypto backend)"; }
    bool verifyEd25519(const std::vector<uint8_t>&, std::string_view, const std::vector<uint8_t>&, std::string& error) const override {
        error = "no signature verifier is available";
        return false;
    }
};
}  // namespace

std::unique_ptr<ISignatureVerifier> makeDefaultVerifier() {
#ifdef EXILE_HAS_OPENSSL
    return std::make_unique<OpenSslVerifier>();
#else
    return std::make_unique<UnavailableVerifier>();
#endif
}

std::string signingPayload(std::string_view appId, std::string_view version, std::string_view packageId,
                           std::string_view sha256, uint64_t downloadSize) {
    std::string p = "exile-store-package-v1\n";
    p += "app=" + std::string(appId) + "\n";
    p += "version=" + std::string(version) + "\n";
    p += "package=" + std::string(packageId) + "\n";
    p += "sha256=" + std::string(sha256) + "\n";
    p += "size=" + std::to_string(downloadSize) + "\n";
    return p;
}

VerifyOutcome verifyPackageSignature(const AppEntry& app, const PackageBuild& build, std::string_view version,
                                     const TrustStore& store, const ISignatureVerifier& verifier) {
    VerifyOutcome out;
    const SignatureMetadata& sig = build.signature;
    if (sig.keyId.empty() || sig.signatureHex.empty()) {
        out.reason = "the package is not signed";
        return out;
    }
    if (sig.algorithm != "ed25519") {
        out.reason = "unsupported signature algorithm '" + sig.algorithm + "'";
        return out;
    }
    const TrustedKey* key = store.find(sig.keyId);
    if (!key) {
        out.reason = "the signing key '" + sig.keyId + "' is not trusted on this device";
        return out;
    }
    if (key->revoked) {
        out.reason = "the signing key '" + sig.keyId + "' has been revoked";
        return out;
    }
    if (key->publisherId != app.publisher.id) {
        out.reason = "the signing key is not authorised for publisher '" + app.publisher.id + "'";
        return out;
    }
    if (!verifier.available()) {
        out.reason = "this system cannot verify signatures yet, so installation is disabled";
        return out;
    }
    auto sigBytes = util::hexDecode(sig.signatureHex);
    if (!sigBytes || sigBytes->size() != 64) {
        out.reason = "the signature is malformed";
        return out;
    }
    std::string payload = signingPayload(app.appId, version, build.id, build.sha256, build.downloadSize);
    std::string err;
    if (!verifier.verifyEd25519(key->publicKey, payload, *sigBytes, err)) {
        out.reason = "the signature is invalid (" + err + ")";
        return out;
    }
    out.trusted = true;
    out.publisherId = key->publisherId;
    out.keyId = key->keyId;
    return out;
}

}  // namespace exile
