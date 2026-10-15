#include "core/sha256.h"
#include "core/version.h"
#include "testing.h"

using namespace exile;

TEST(version_parses_and_prints) {
    auto v = parseVersion("1.2.3-beta.4+build.7");
    CHECK(v.has_value());
    CHECK_EQ(v->major, uint64_t(1));
    CHECK_EQ(v->minor, uint64_t(2));
    CHECK_EQ(v->patch, uint64_t(3));
    CHECK_EQ(v->str(), std::string("1.2.3-beta.4"));
}

TEST(version_rejects_invalid) {
    for (const char* bad : {"", "1", "1.2", "1.2.3.4", "01.2.3", "1.02.3", "1.2.x", "a.b.c", "1.2.3-", "1.2.3-01",
                            "1.2.3-be ta", "1.2.3+", "-1.2.3", "1..3", " 1.2.3", "1.2.3 "})
        if (parseVersion(bad)) throw testing::Failure(testing::Failure{std::string("accepted invalid version: '") + bad + "'"});
    CHECK(!parseVersion(std::string(200, '1') + ".0.0"));
}

TEST(version_ordering_follows_semver) {
    // The exact chain from the semver 2.0 specification, lowest to highest.
    const char* chain[] = {"1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-alpha.beta", "1.0.0-beta", "1.0.0-beta.2",
                           "1.0.0-beta.11", "1.0.0-rc.1", "1.0.0", "1.0.1", "1.1.0", "2.0.0"};
    for (size_t i = 0; i + 1 < std::size(chain); ++i) {
        CHECK_EQ(compareVersionStrings(chain[i], chain[i + 1]), -1);
        CHECK_EQ(compareVersionStrings(chain[i + 1], chain[i]), 1);
    }
}

TEST(version_numeric_not_lexicographic) {
    CHECK_EQ(compareVersionStrings("1.10.0", "1.9.0"), 1);
    CHECK_EQ(compareVersionStrings("10.0.0", "9.99.99"), 1);
    CHECK_EQ(compareVersionStrings("1.0.0+a", "1.0.0+b"), 0);  // build metadata ignored
    CHECK_EQ(compareVersionStrings("1.0.0", "1.0.0"), 0);
}

TEST(version_unparsable_sorts_lowest) {
    CHECK_EQ(compareVersionStrings("garbage", "0.0.1"), -1);
    CHECK_EQ(compareVersionStrings("0.0.1", "garbage"), 1);
    CHECK_EQ(compareVersionStrings("garbage", "also-garbage"), 0);
}

TEST(sha256_known_vectors) {
    CHECK_EQ(sha256Hex(""), std::string("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"));
    CHECK_EQ(sha256Hex("abc"), std::string("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
    CHECK_EQ(sha256Hex("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
             std::string("248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"));
}

TEST(sha256_million_a_and_chunking) {
    std::string million(1000000, 'a');
    CHECK_EQ(sha256Hex(million), std::string("cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"));
    Sha256 s;  // feeding in odd-sized chunks must not change the digest
    for (size_t i = 0; i < million.size(); i += 777) s.update(million.data() + i, std::min<size_t>(777, million.size() - i));
    CHECK_EQ(s.finishHex(), std::string("cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"));
}

TEST(sha256_file_matches_memory) {
    testing::TempDir dir;
    testing::writeFile(dir / "f.bin", std::string(200000, 'x') + "tail");
    auto d = sha256FileHex(dir / "f.bin");
    CHECK(d.has_value());
    CHECK_EQ(*d, sha256Hex(std::string(200000, 'x') + "tail"));
    std::string err;
    CHECK(!sha256FileHex(dir / "missing.bin", &err).has_value());
    CHECK(!err.empty());
}
