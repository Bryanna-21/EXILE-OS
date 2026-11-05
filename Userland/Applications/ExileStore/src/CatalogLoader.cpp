/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include "CatalogLoader.h"
#include <AK/JsonArray.h>
#include <AK/JsonObject.h>
#include <AK/JsonValue.h>
#include <AK/StringBuilder.h>
#include <AK/Utf8View.h>

namespace ExileStore {

LoadResult failed_load(ByteString message)
{
    LoadResult result;
    result.status = LoadStatus::Failed;
    result.message = move(message);
    return result;
}

static bool is_segment(StringView segment)
{
    if (segment.is_empty() || segment.length() > 63)
        return false;
    auto is_alnum = [](char c) { return (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9'); };
    if (!is_alnum(segment[0]))
        return false;
    for (size_t i = 0; i < segment.length(); ++i) {
        char c = segment[i];
        if (!is_alnum(c) && c != '-')
            return false;
    }
    return true;
}

bool is_valid_app_id(StringView id)
{
    if (id.is_empty() || id.length() > 128)
        return false;
    size_t segments = 0;
    size_t start = 0;
    while (true) {
        auto dot = id.find('.', start);
        auto end = dot.has_value() ? dot.value() : id.length();
        if (!is_segment(id.substring_view(start, end - start)))
            return false;
        ++segments;
        if (!dot.has_value())
            break;
        start = end + 1;
    }
    return segments >= 2;
}

ByteString sanitize_text(StringView text, size_t max_bytes, bool allow_newlines)
{
    StringBuilder builder;
    for (size_t i = 0; i < text.length() && builder.length() < max_bytes; ++i) {
        u8 c = static_cast<u8>(text[i]);
        if (c == '\n' && allow_newlines) {
            builder.append('\n');
            continue;
        }
        if (c == '\t' || c == '\r' || c == '\n') {
            builder.append(' ');
            continue;
        }
        if (c < 0x20 || c == 0x7f)
            continue;
        builder.append(static_cast<char>(c));
    }
    auto result = builder.to_byte_string();
    // Do not leave half a UTF-8 sequence at the cut: drop trailing continuation bytes and the lead byte before them.
    if (result.length() >= max_bytes && max_bytes > 0) {
        size_t length = result.length();
        size_t i = length;
        while (i > 0 && (static_cast<u8>(result[i - 1]) & 0xC0) == 0x80)
            --i;
        if (i > 0 && static_cast<u8>(result[i - 1]) >= 0xC0)
            --i;
        if (i < length && !Utf8View(result).validate())
            result = result.substring(0, i);
    }
    return result;
}

static ByteString text_field(JsonObject const& object, StringView key, StringView fallback, size_t max = max_short_field, bool newlines = false)
{
    auto value = object.get_byte_string(key);
    if (!value.has_value())
        return ByteString(fallback);
    auto clean = sanitize_text(value.value(), max, newlines);
    return clean.is_empty() ? ByteString(fallback) : clean;
}

static void append_permissions(JsonObject const& app, Vector<ByteString>& out)
{
    auto permissions = app.get_array("permissions"sv);
    if (!permissions.has_value())
        return;
    for (size_t i = 0; i < permissions->size() && i < 32; ++i) {
        auto const& item = permissions->at(i);
        if (!item.is_object())
            continue;
        auto const& object = item.as_object();
        auto id = text_field(object, "id"sv, ""sv);
        if (id.is_empty())
            continue;
        auto reason = text_field(object, "reason"sv, "no reason given"sv, max_long_field);
        out.append(ByteString::formatted("{}: {}", id, reason));
    }
}

static void append_packages(JsonObject const& app, Vector<ByteString>& out)
{
    auto packages = app.get_array("packages"sv);
    if (!packages.has_value())
        return;
    for (size_t i = 0; i < packages->size() && i < 32; ++i) {
        auto const& item = packages->at(i);
        if (!item.is_object())
            continue;
        auto const& object = item.as_object();
        auto kind = text_field(object, "kind"sv, "unknown"sv);
        auto channel = text_field(object, "channel"sv, "unknown"sv);
        StringBuilder architectures;
        if (auto list = object.get_array("architectures"sv); list.has_value()) {
            for (size_t j = 0; j < list->size() && j < 8; ++j) {
                if (!list->at(j).is_string())
                    continue;
                if (!architectures.is_empty())
                    architectures.append(',');
                architectures.append(sanitize_text(list->at(j).as_string(), 24));
            }
        }
        auto size = object.get_u64("downloadSize"sv);
        auto line = ByteString::formatted("{} build, channel {}, architectures {}", kind, channel,
            architectures.is_empty() ? "unspecified"sv : architectures.string_view());
        if (size.has_value())
            line = ByteString::formatted("{}, download {} bytes", line, size.value());
        out.append(move(line));
    }
}

LoadResult parse_catalog(StringView json_text)
{
    if (json_text.length() > max_catalog_bytes)
        return failed_load("The Exile Store catalog is larger than the supported limit and was not loaded.");
    if (json_text.trim_whitespace().is_empty())
        return failed_load("The Exile Store catalog file is empty.");

    auto parsed = JsonValue::from_string(json_text);
    if (parsed.is_error())
        return failed_load("The Exile Store catalog contains invalid JSON and was not loaded.");

    auto const json = parsed.release_value();
    if (!json.is_object())
        return failed_load("The Exile Store catalog has an invalid structure (expected a JSON object).");

    auto apps = json.as_object().get_array("apps"sv);
    if (!apps.has_value())
        return failed_load("The Exile Store catalog has no \"apps\" list.");

    LoadResult result;
    result.status = LoadStatus::Loaded;
    for (size_t i = 0; i < apps->size(); ++i) {
        if (result.entries.size() >= max_catalog_entries) {
            result.skipped += apps->size() - i;
            break;
        }
        auto const& item = apps->at(i);
        if (!item.is_object()) {
            ++result.skipped;
            continue;
        }
        auto const& object = item.as_object();

        auto id = object.get_byte_string("appId"sv);
        if (!id.has_value() || !is_valid_app_id(id.value())) {
            ++result.skipped;
            continue;
        }

        Entry entry;
        entry.id = id.value();
        entry.name = text_field(object, "name"sv, "Unnamed application"sv);
        entry.summary = text_field(object, "summary"sv, ""sv, max_long_field);
        entry.description = text_field(object, "description"sv, ""sv, max_long_field, true);
        entry.version = text_field(object, "currentVersion"sv, "Unknown"sv, 64);
        entry.license = text_field(object, "license"sv, "Not stated"sv);
        entry.publisher = "Unknown publisher"sv;
        if (auto publisher = object.get_object("publisher"sv); publisher.has_value())
            entry.publisher = text_field(publisher.value(), "name"sv, "Unknown publisher"sv);

        if (auto categories = object.get_array("categories"sv); categories.has_value() && !categories->is_empty()
            && categories->at(0).is_string())
            entry.category = sanitize_text(categories->at(0).as_string(), 64);

        StringBuilder keywords;
        if (auto list = object.get_array("keywords"sv); list.has_value()) {
            for (size_t j = 0; j < list->size() && j < 32; ++j) {
                if (!list->at(j).is_string())
                    continue;
                keywords.append(sanitize_text(list->at(j).as_string(), 48));
                keywords.append(' ');
            }
        }
        entry.keywords = keywords.to_byte_string();

        append_permissions(object, entry.permissions);
        append_packages(object, entry.packages);
        result.entries.append(move(entry));
    }

    if (result.entries.is_empty()) {
        result.status = LoadStatus::Empty;
        result.message = result.skipped > 0
            ? ByteString::formatted("The catalog was read but all {} entries were invalid and skipped.", result.skipped)
            : ByteString("The catalog was read successfully but lists no applications.");
    }
    return result;
}

bool entry_matches(Entry const& entry, StringView query)
{
    auto trimmed = query.trim_whitespace();
    if (trimmed.is_empty())
        return true;
    auto contains = [&](ByteString const& haystack) { return haystack.contains(trimmed, CaseSensitivity::CaseInsensitive); };
    return contains(entry.name) || contains(entry.summary) || contains(entry.category) || contains(entry.publisher)
        || contains(entry.keywords) || contains(entry.id);
}

Vector<size_t> filter_entries(Vector<Entry> const& entries, StringView query)
{
    Vector<size_t> visible;
    for (size_t i = 0; i < entries.size(); ++i) {
        if (entry_matches(entries[i], query))
            visible.append(i);
    }
    return visible;
}

ByteString status_text(LoadResult const& result, size_t shown)
{
    switch (result.status) {
    case LoadStatus::Failed:
        return result.message;
    case LoadStatus::Empty:
        return result.message;
    case LoadStatus::Loaded:
        break;
    }
    auto text = ByteString::formatted("{} of {} catalog entries shown.", shown, result.entries.size());
    if (result.skipped > 0)
        text = ByteString::formatted("{} {} invalid entries were skipped.", text, result.skipped);
    return text;
}

}
