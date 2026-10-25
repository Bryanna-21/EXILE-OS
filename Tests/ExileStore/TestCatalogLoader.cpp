/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include <AK/StringBuilder.h>
#include <LibTest/TestCase.h>

#include <CatalogLoader.h>

using namespace ExileStore;

static constexpr auto valid_catalog = R"({
  "schemaVersion": 1,
  "apps": [
    {
      "appId": "org.exile.sample.notes",
      "name": "Sample Notes",
      "summary": "A tiny notes app.",
      "description": "Line one.\nLine two.",
      "publisher": { "id": "org.exile.devsamples", "name": "Exile Development Samples" },
      "categories": ["productivity"],
      "keywords": ["notes", "write"],
      "currentVersion": "1.0.0",
      "license": "Sample data",
      "permissions": [{ "id": "documents", "reason": "Saves notes." }],
      "packages": [{ "id": "main", "kind": "native", "channel": "stable", "architectures": ["x86_64", "aarch64"], "downloadSize": 180 }]
    },
    {
      "appId": "org.exile.sample.player",
      "name": "Sample Player",
      "summary": "Plays things.",
      "currentVersion": "2.1.0"
    }
  ]
})"sv;

TEST_CASE(valid_catalog_loads)
{
    auto result = parse_catalog(valid_catalog);
    EXPECT_EQ(result.status, LoadStatus::Loaded);
    EXPECT_EQ(result.entries.size(), 2u);
    EXPECT_EQ(result.skipped, 0u);
    EXPECT_EQ(result.entries[0].id, "org.exile.sample.notes"sv);
    EXPECT_EQ(result.entries[0].publisher, "Exile Development Samples"sv);
    EXPECT_EQ(result.entries[0].category, "productivity"sv);
    EXPECT_EQ(result.entries[0].permissions.size(), 1u);
    EXPECT_EQ(result.entries[0].permissions[0], "documents: Saves notes."sv);
    EXPECT_EQ(result.entries[0].packages.size(), 1u);
    EXPECT(result.entries[0].packages[0].contains("x86_64,aarch64"sv));
    // Missing optional fields get honest placeholders instead of invented values.
    EXPECT_EQ(result.entries[1].publisher, "Unknown publisher"sv);
    EXPECT_EQ(result.entries[1].license, "Not stated"sv);
}

// Regression: refresh_list() used to overwrite a load_catalog() failure with "0 of 0 catalog entries shown".
TEST_CASE(failed_load_status_is_never_zero_of_zero)
{
    auto failed = failed_load("The Exile Store catalog could not be opened."_string.to_byte_string());
    EXPECT_EQ(failed.status, LoadStatus::Failed);
    for (auto query : { ""sv, "x"sv, "notes"sv }) {
        auto status = status_text(failed, filter_entries(failed.entries, query).size());
        EXPECT_EQ(status, "The Exile Store catalog could not be opened."sv);
        EXPECT(!status.contains("0 of 0"sv));
    }
}

TEST_CASE(malformed_catalogs_fail_with_specific_messages)
{
    auto invalid_json = parse_catalog("{ not json"sv);
    EXPECT_EQ(invalid_json.status, LoadStatus::Failed);
    EXPECT(invalid_json.message.contains("invalid JSON"sv));

    auto empty_file = parse_catalog("   \n"sv);
    EXPECT_EQ(empty_file.status, LoadStatus::Failed);
    EXPECT(empty_file.message.contains("empty"sv));

    auto not_object = parse_catalog("[1,2,3]"sv);
    EXPECT_EQ(not_object.status, LoadStatus::Failed);
    EXPECT(not_object.message.contains("invalid structure"sv));

    auto no_apps = parse_catalog(R"({"schemaVersion":1})"sv);
    EXPECT_EQ(no_apps.status, LoadStatus::Failed);
    EXPECT(no_apps.message.contains("no \"apps\" list"sv));

    auto apps_wrong_type = parse_catalog(R"({"apps": "nope"})"sv);
    EXPECT_EQ(apps_wrong_type.status, LoadStatus::Failed);

    // A failed load must be distinguishable from a successful load with zero packages.
    EXPECT(invalid_json.message != parse_catalog(R"({"apps":[]})"sv).message);
}

TEST_CASE(zero_packages_is_empty_not_failed)
{
    auto result = parse_catalog(R"({"apps":[]})"sv);
    EXPECT_EQ(result.status, LoadStatus::Empty);
    EXPECT(result.entries.is_empty());
    EXPECT(result.message.contains("lists no applications"sv));
    EXPECT_EQ(status_text(result, 0), result.message);
}

TEST_CASE(invalid_entries_are_skipped_and_counted)
{
    auto result = parse_catalog(R"({"apps":[
        {"appId":"org.exile.good","name":"Good"},
        {"appId":"../../etc/passwd","name":"Traversal"},
        {"appId":"Org.Exile.Upper","name":"Uppercase"},
        {"appId":"nodots","name":"One segment"},
        {"name":"No id"},
        42,
        {"appId":"org.exile.good2","name":"Good 2"}
    ]})"sv);
    EXPECT_EQ(result.status, LoadStatus::Loaded);
    EXPECT_EQ(result.entries.size(), 2u);
    EXPECT_EQ(result.skipped, 5u);
    EXPECT(status_text(result, 2).contains("5 invalid entries were skipped"sv));

    auto all_bad = parse_catalog(R"({"apps":[{"appId":"x"},{"appId":"y"}]})"sv);
    EXPECT_EQ(all_bad.status, LoadStatus::Empty);
    EXPECT(all_bad.message.contains("all 2 entries were invalid"sv));
}

TEST_CASE(app_id_validation)
{
    EXPECT(is_valid_app_id("org.exile.store"sv));
    EXPECT(is_valid_app_id("a1.b-2"sv));
    EXPECT(!is_valid_app_id(""sv));
    EXPECT(!is_valid_app_id("single"sv));
    EXPECT(!is_valid_app_id("org..empty"sv));
    EXPECT(!is_valid_app_id(".leading.dot"sv));
    EXPECT(!is_valid_app_id("trailing.dot."sv));
    EXPECT(!is_valid_app_id("org.exile/store"sv));
    EXPECT(!is_valid_app_id("org.exile.store; rm -rf /"sv));
    EXPECT(!is_valid_app_id("org.-bad.start"sv));
    EXPECT(!is_valid_app_id("org.under_score"sv));
    StringBuilder too_long;
    too_long.append("org."sv);
    for (int i = 0; i < 130; ++i)
        too_long.append('a');
    EXPECT(!is_valid_app_id(too_long.string_view()));
}

TEST_CASE(text_is_sanitized_not_executed)
{
    auto result = parse_catalog(R"({"apps":[{
        "appId":"org.exile.evil",
        "name":"Evil\u0007\u001b[31m Name",
        "summary":"$(touch /tmp/pwned) `id` ; rm -rf /",
        "description":"First\nSecond\u0000Third"
    }]})"sv);
    EXPECT_EQ(result.status, LoadStatus::Loaded);
    auto const& entry = result.entries[0];
    for (auto const& field : { entry.name, entry.summary, entry.description }) {
        for (size_t i = 0; i < field.length(); ++i) {
            u8 c = static_cast<u8>(field[i]);
            EXPECT(c == '\n' || (c >= 0x20 && c != 0x7f));
        }
    }
    EXPECT(!entry.name.contains('\x1b'));
    EXPECT(entry.description.contains('\n'));
    // Shell metacharacters are plain text; they are shown, never interpreted.
    EXPECT(entry.summary.contains("$(touch /tmp/pwned)"sv));
}

TEST_CASE(overlong_fields_are_truncated_on_utf8_boundaries)
{
    StringBuilder name;
    for (int i = 0; i < 400; ++i)
        name.append("\xC3\xA9"sv); // U+00E9, two bytes each
    auto text = ByteString::formatted(R"({{"apps":[{{"appId":"org.exile.long","name":"{}"}}]}})", name.string_view());
    auto result = parse_catalog(text);
    EXPECT_EQ(result.status, LoadStatus::Loaded);
    EXPECT(result.entries[0].name.length() <= max_short_field);
    EXPECT(Utf8View(result.entries[0].name).validate());
}

TEST_CASE(catalog_size_limit)
{
    StringBuilder huge;
    huge.append(R"({"apps":[],"pad":")"sv);
    for (size_t i = 0; i < max_catalog_bytes; ++i)
        huge.append('a');
    huge.append("\"}"sv);
    auto result = parse_catalog(huge.string_view());
    EXPECT_EQ(result.status, LoadStatus::Failed);
    EXPECT(result.message.contains("larger than the supported limit"sv));
}

TEST_CASE(search_and_filter)
{
    auto result = parse_catalog(valid_catalog);
    EXPECT_EQ(filter_entries(result.entries, ""sv).size(), 2u);
    EXPECT_EQ(filter_entries(result.entries, "   "sv).size(), 2u);
    EXPECT_EQ(filter_entries(result.entries, "NOTES"sv).size(), 1u);                   // name, case-insensitive
    EXPECT_EQ(filter_entries(result.entries, "productivity"sv).size(), 1u);            // category
    EXPECT_EQ(filter_entries(result.entries, "write"sv).size(), 1u);                   // keyword
    EXPECT_EQ(filter_entries(result.entries, "development samples"sv).size(), 1u);     // publisher
    EXPECT_EQ(filter_entries(result.entries, "org.exile.sample.player"sv).size(), 1u); // id
    EXPECT_EQ(filter_entries(result.entries, "zzz-no-match"sv).size(), 0u);

    // A successful load with a filter that matches nothing is a normal "0 of N" state, not an error.
    auto status = status_text(result, 0);
    EXPECT_EQ(status, "0 of 2 catalog entries shown."sv);
    EXPECT_EQ(status_text(result, 2), "2 of 2 catalog entries shown."sv);
}

// Regression for the use-after-free: the names shown by the list model are copied out of the loaded entries on each
// refresh, and entries are owned by the LoadResult, so a result/filter pair stays valid for as long as the result lives.
TEST_CASE(filtered_indices_remain_valid_for_result_lifetime)
{
    auto result = parse_catalog(valid_catalog);
    auto visible = filter_entries(result.entries, "sample"sv);
    EXPECT_EQ(visible.size(), 2u);
    Vector<ByteString> names;
    for (auto index : visible)
        names.append(result.entries[index].name);
    // Reload (as the Refresh button does) and make sure old copies are unaffected.
    result = parse_catalog(R"({"apps":[]})"sv);
    EXPECT_EQ(names.size(), 2u);
    EXPECT_EQ(names[0], "Sample Notes"sv);
    EXPECT_EQ(result.status, LoadStatus::Empty);
}
