/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include "../src/core/trust.h"
#include "testing.h"

using namespace exile;
using testing::Fixture;
using testing::PkgSpec;

#ifdef EXILE_HAS_OPENSSL

namespace {

struct Prepared {
    Catalog catalog;
    AppEntry const* app;
    PackageBuild const* build;
};

Prepared prepare(Fixture& f, PkgSpec const& s)
{
    Prepared p { f.catalog({ f.build(s).json }), nullptr, nullptr };
    p.app = p.catalog.find(s.appId);
    p.build = &p.app->packages[0];
    return p;
}

} // namespace

TEST(trust_accepts_valid_signature_from_trusted_key)
{
    Fixture f;
    PkgSpec s;
    auto p = prepare(f, s);
    VerifyOutcome o = verifyPackageSignature(*p.app, *p.build, "1.0.0", f.trust, *f.verifier);
    CHECK(o.trusted);
    CHECK_EQ(o.keyId, std::string("test-key"));
    CHECK_EQ(o.publisherId, std::string("org.test"));
}

TEST(trust_rejects_unsigned_package)
{
    Fixture f;
    PkgSpec s;
    s.omitSignature = true;
    auto p = prepare(f, s);
    VerifyOutcome o = verifyPackageSignature(*p.app, *p.build, "1.0.0", f.trust, *f.verifier);
    CHECK(!o.trusted);
    CHECK_CONTAINS(o.reason, "not signed");
}

TEST(trust_rejects_signature_from_unknown_key)
{
    Fixture f;
    PkgSpec s;
    s.signWithUntrustedKey = true; // key id "other-key" is not in the store
    auto p = prepare(f, s);
    VerifyOutcome o = verifyPackageSignature(*p.app, *p.build, "1.0.0", f.trust, *f.verifier);
    CHECK(!o.trusted);
    CHECK_CONTAINS(o.reason, "not trusted");
}

TEST(trust_rejects_valid_signature_claiming_a_trusted_key_id)
{
    // Signed with the wrong private key but labelled with the trusted key id: the math must fail.
    Fixture f;
    PkgSpec s;
    s.signWithUntrustedKey = true;
    s.keyIdOverride = "test-key";
    auto p = prepare(f, s);
    VerifyOutcome o = verifyPackageSignature(*p.app, *p.build, "1.0.0", f.trust, *f.verifier);
    CHECK(!o.trusted);
    CHECK_CONTAINS(o.reason, "invalid");
}

TEST(trust_signature_binds_version_app_package_digest_and_size)
{
    Fixture f;
    PkgSpec s;
    auto p = prepare(f, s);
    // Replaying a genuine 1.0.0 signature as a different version (downgrade/rollback attack) must fail.
    CHECK(!verifyPackageSignature(*p.app, *p.build, "0.9.0", f.trust, *f.verifier).trusted);
    CHECK(!verifyPackageSignature(*p.app, *p.build, "2.0.0", f.trust, *f.verifier).trusted);

    PackageBuild changed = *p.build;
    changed.sha256 = std::string(64, 'a');
    CHECK(!verifyPackageSignature(*p.app, changed, "1.0.0", f.trust, *f.verifier).trusted);
    changed = *p.build;
    changed.downloadSize += 1;
    CHECK(!verifyPackageSignature(*p.app, changed, "1.0.0", f.trust, *f.verifier).trusted);
    changed = *p.build;
    changed.id = "other";
    CHECK(!verifyPackageSignature(*p.app, changed, "1.0.0", f.trust, *f.verifier).trusted);
    AppEntry other = *p.app;
    other.appId = "org.test.different";
    CHECK(!verifyPackageSignature(other, *p.build, "1.0.0", f.trust, *f.verifier).trusted);
}

TEST(trust_rejects_flipped_signature_bit_and_malformed_signature)
{
    Fixture f;
    PkgSpec s;
    auto p = prepare(f, s);
    PackageBuild b = *p.build;
    b.signature.signatureHex[5] = b.signature.signatureHex[5] == '0' ? '1' : '0';
    CHECK(!verifyPackageSignature(*p.app, b, "1.0.0", f.trust, *f.verifier).trusted);
    b = *p.build;
    b.signature.signatureHex = "abcd";
    VerifyOutcome o = verifyPackageSignature(*p.app, b, "1.0.0", f.trust, *f.verifier);
    CHECK(!o.trusted);
    CHECK_CONTAINS(o.reason, "malformed");
}

TEST(trust_rejects_revoked_key)
{
    Fixture f;
    PkgSpec s;
    auto p = prepare(f, s);
    TrustStore revoked;
    TrustedKey k = *f.trust.find("test-key");
    k.revoked = true;
    revoked.add(k);
    VerifyOutcome o = verifyPackageSignature(*p.app, *p.build, "1.0.0", revoked, *f.verifier);
    CHECK(!o.trusted);
    CHECK_CONTAINS(o.reason, "revoked");
}

TEST(trust_key_is_only_valid_for_its_own_publisher)
{
    Fixture f;
    PkgSpec s;
    s.publisher = "org.attacker"; // a publisher the key was never authorised for
    auto p = prepare(f, s);
    VerifyOutcome o = verifyPackageSignature(*p.app, *p.build, "1.0.0", f.trust, *f.verifier);
    CHECK(!o.trusted);
    CHECK_CONTAINS(o.reason, "not authorised");
}

TEST(trust_rejects_unsupported_algorithm)
{
    Fixture f;
    PkgSpec s;
    auto p = prepare(f, s);
    PackageBuild b = *p.build;
    b.signature.algorithm = "rsa-md5";
    CHECK_CONTAINS(verifyPackageSignature(*p.app, b, "1.0.0", f.trust, *f.verifier).reason, "unsupported");
}

TEST(trust_store_parsing_valid_and_malformed)
{
    Fixture f;
    std::string err;
    std::string good = "{\"schemaVersion\":1,\"keys\":[{\"keyId\":\"k1\",\"publisherId\":\"org.a\",\"algorithm\":\"ed25519\",\"publicKey\":\"" + f.good.pubHex + "\"}]}";
    auto ts = TrustStore::parse(good, err);
    CHECK(ts.has_value());
    CHECK_EQ(ts->size(), size_t(1));
    CHECK(ts->find("k1") != nullptr);
    CHECK(ts->find("nope") == nullptr);

    auto bad = [&](std::string const& json) { std::string e; return !TrustStore::parse(json, e).has_value() && !e.empty(); };
    CHECK(bad("not json"));
    CHECK(bad("{\"schemaVersion\":2,\"keys\":[]}"));
    CHECK(bad("{\"schemaVersion\":1}"));
    CHECK(bad("{\"schemaVersion\":1,\"keys\":[{\"keyId\":\"k\"}]}"));
    CHECK(bad("{\"schemaVersion\":1,\"keys\":[{\"keyId\":\"k\",\"publisherId\":\"org.a\",\"algorithm\":\"ed25519\",\"publicKey\":\"abcd\"}]}"));
    CHECK(bad("{\"schemaVersion\":1,\"keys\":[{\"keyId\":\"k\",\"publisherId\":\"org.a\",\"algorithm\":\"rsa\",\"publicKey\":\"" + f.good.pubHex + "\"}]}"));
    CHECK(bad("{\"schemaVersion\":1,\"keys\":[{\"keyId\":\"k\",\"publisherId\":\"bad id\",\"algorithm\":\"ed25519\",\"publicKey\":\"" + f.good.pubHex + "\"}]}"));
    std::string dup = "{\"schemaVersion\":1,\"keys\":[{\"keyId\":\"k\",\"publisherId\":\"org.a\",\"algorithm\":\"ed25519\",\"publicKey\":\"" + f.good.pubHex + "\"},{\"keyId\":\"k\",\"publisherId\":\"org.a\",\"algorithm\":\"ed25519\",\"publicKey\":\"" + f.good.pubHex + "\"}]}";
    CHECK(bad(dup));
}

#endif // EXILE_HAS_OPENSSL

namespace {
// A verifier that cannot verify, standing in for a build with no crypto backend.
class NoVerifier final : public ISignatureVerifier {
public:
    bool available() const override { return false; }
    std::string name() const override { return "none"; }
    bool verifyEd25519(std::vector<uint8_t> const&, std::string_view, std::vector<uint8_t> const&, std::string&) const override { return true; }
};
} // namespace

TEST(trust_refuses_everything_when_no_verifier_is_available)
{
    Fixture f;
    PkgSpec s;
    Catalog c = f.catalog({ f.build(s).json });
    AppEntry const* app = c.find(s.appId);
    NoVerifier none; // even though its verify() would say "true", an unavailable verifier must never be consulted
    TrustStore store;
    TrustedKey k;
    k.keyId = "test-key";
    k.publisherId = "org.test";
    k.algorithm = "ed25519";
    k.publicKey.assign(32, 1);
    store.add(k);
    VerifyOutcome o = verifyPackageSignature(*app, app->packages[0], "1.0.0", store, none);
    CHECK(!o.trusted);
    CHECK_CONTAINS(o.reason, "cannot verify signatures");
}

TEST(signing_payload_is_unambiguous)
{
    CHECK(signingPayload("a.b", "1.0.0", "main", "ff", 5) != signingPayload("a.b", "1.0.0", "main", "ff", 6));
    CHECK(signingPayload("a.b", "1.0.0", "main", "ff", 5) != signingPayload("a.c", "1.0.0", "main", "ff", 5));
    // A field value containing a newline cannot be smuggled in: ids are validated, but the payload also
    // differs structurally from one built with different splits.
    CHECK(signingPayload("a.b", "1.0.0\nsha256=ff", "main", "00", 5) != signingPayload("a.b", "1.0.0", "main", "ff", 5));
}
