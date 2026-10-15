#pragma once
// Package trust: a trust store of publisher keys (kept separate from catalog
// metadata) and Ed25519 signature verification.
//
// What is signed: a canonical payload that binds appId, version, package id,
// SHA-256 digest and size. The digest is re-checked against the downloaded
// file, so a valid signature + matching digest proves the bytes came from a
// holder of a trusted key for that exact app and version.

#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

#include "core/model.h"

namespace exile {

struct TrustedKey {
    std::string keyId;
    std::string publisherId;  // the only publisher this key may sign for
    std::string algorithm;    // "ed25519"
    std::vector<uint8_t> publicKey;
    bool revoked = false;
};

class TrustStore {
public:
    static std::optional<TrustStore> parse(std::string_view jsonText, std::string& error);
    static std::optional<TrustStore> loadFile(const std::string& path, std::string& error);

    void add(TrustedKey key);
    const TrustedKey* find(std::string_view keyId) const;
    size_t size() const { return keys_.size(); }

private:
    std::vector<TrustedKey> keys_;
};

class ISignatureVerifier {
public:
    virtual ~ISignatureVerifier() = default;
    // False when this build cannot verify signatures at all; installs must then be refused.
    virtual bool available() const = 0;
    virtual std::string name() const = 0;
    virtual bool verifyEd25519(const std::vector<uint8_t>& publicKey, std::string_view message,
                               const std::vector<uint8_t>& signature, std::string& error) const = 0;
};

// OpenSSL-backed verifier when compiled in, otherwise one that reports unavailable.
std::unique_ptr<ISignatureVerifier> makeDefaultVerifier();

std::string signingPayload(std::string_view appId, std::string_view version, std::string_view packageId,
                           std::string_view sha256, uint64_t downloadSize);

struct VerifyOutcome {
    bool trusted = false;
    std::string reason;       // why not, in plain language
    std::string publisherId;  // set when trusted
    std::string keyId;
};

VerifyOutcome verifyPackageSignature(const AppEntry& app, const PackageBuild& build, std::string_view version,
                                     const TrustStore& store, const ISignatureVerifier& verifier);

}  // namespace exile
