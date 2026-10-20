/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include "../src/core/json.h"
#include "../src/core/util.h"
#include "testing.h"

using namespace exile;

TEST(safe_relative_path_accepts_normal_paths)
{
    CHECK(util::isSafeRelativePath("a"));
    CHECK(util::isSafeRelativePath("bin/run"));
    CHECK(util::isSafeRelativePath("a/b/c.txt"));
    CHECK(util::isSafeRelativePath("name with space.txt"));
    CHECK(util::isSafeRelativePath("..hidden")); // not a ".." segment
}

TEST(safe_relative_path_rejects_dangerous_paths)
{
    for (char const* bad : { "", "/etc/passwd", "../x", "a/../b", "a/..", "./a", "a/./b", "a//b", "a/", "a\\b",
             "C:evil", "dir/\x01x", "x\x7f" })
        CHECK(!util::isSafeRelativePath(bad));
    CHECK(!util::isSafeRelativePath(std::string("a\0b", 3)));
    CHECK(!util::isSafeRelativePath(std::string(2000, 'a')));
    CHECK(!util::isSafeRelativePath(std::string(300, 'a'))); // single segment over 255
}

TEST(hex_roundtrip_and_validation)
{
    std::vector<uint8_t> bytes = { 0x00, 0xff, 0x10, 0xab };
    std::string h = util::hexEncode(bytes.data(), bytes.size());
    CHECK_EQ(h, std::string("00ff10ab"));
    CHECK(util::hexDecode(h).value() == bytes);
    CHECK(util::hexDecode("ABcd").has_value());
    CHECK(!util::hexDecode("abc").has_value());
    CHECK(!util::hexDecode("zz").has_value());
    CHECK(util::isLowerHex("00ff", 4));
    CHECK(!util::isLowerHex("00FF", 4));
    CHECK(!util::isLowerHex("00ff", 5));
}

TEST(human_size_formats)
{
    CHECK_EQ(util::humanSize(0), std::string("0 B"));
    CHECK_EQ(util::humanSize(999), std::string("999 B"));
    CHECK_EQ(util::humanSize(1500), std::string("1.5 KB"));
    CHECK_EQ(util::humanSize(250000000), std::string("250 MB"));
}

TEST(json_parses_values)
{
    auto v = json::parse(R"({"a":1,"b":[true,false,null],"c":"x\ny\u00e9\ud83d\ude00","d":{"e":-2.5e1}})");
    CHECK(v.isObject());
    CHECK_EQ(*v.find("a")->unsignedInteger(), uint64_t(1));
    CHECK_EQ(v.find("b")->items().size(), size_t(3));
    CHECK(*v.find("b")->items()[0].boolean());
    CHECK(v.find("b")->items()[2].isNull());
    CHECK_EQ(*v.find("c")->string(), std::string("x\ny\xC3\xA9\xF0\x9F\x98\x80"));
    CHECK_EQ(*v.find("d")->find("e")->number(), -25.0);
    CHECK(v.find("missing") == nullptr);
}

TEST(json_rejects_malformed_input)
{
    for (char const* bad : { "", "{", "[1,]", "{\"a\":1,}", "{\"a\" 1}", "{a:1}", "[1 2]", "01", "1.", "-", "1e", "tru",
             "\"unterminated", "\"bad\\q\"", "\"\\u12\"", "{\"a\":1} extra", "[1]]", "nul", "\"tab\there\"" }) {
        bool threw = false;
        try {
            json::parse(bad);
        } catch (json::ParseError const&) {
            threw = true;
        }
        if (!threw)
            throw testing::Failure { std::string("accepted malformed JSON: ") + bad };
    }
}

TEST(json_rejects_duplicate_keys_surrogates_and_deep_nesting)
{
    auto throws = [](std::string const& s) {
        try {
            json::parse(s);
        } catch (json::ParseError const&) {
            return true;
        }
        return false;
    };
    CHECK(throws(R"({"a":1,"a":2})"));
    CHECK(throws(R"("\ud83d")"));
    CHECK(throws(R"("\ude00")"));
    CHECK(throws(R"("\ud83d\u0041")"));
    CHECK(throws(std::string(200, '[') + std::string(200, ']')));
    CHECK(!throws(std::string(40, '[') + std::string(40, ']')));
}

TEST(json_unsigned_integer_is_strict)
{
    CHECK(json::parse("5").unsignedInteger().has_value());
    CHECK(!json::parse("-1").unsignedInteger().has_value());
    CHECK(!json::parse("1.5").unsignedInteger().has_value());
    CHECK(!json::parse("1e300").unsignedInteger().has_value());
    CHECK(!json::parse("\"5\"").unsignedInteger().has_value());
}

TEST(json_dump_roundtrips)
{
    auto v = json::parse(R"({"s":"q\"\\\n","n":[1,2.5,{"x":null}],"b":true})");
    auto again = json::parse(json::dump(v));
    CHECK_EQ(json::dump(v, 0), json::dump(again, 0));
    CHECK_EQ(*again.find("s")->string(), std::string("q\"\\\n"));
}
