/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

// Catalog parsing for the native Exile Store. Depends on AK only so it can be unit-tested on the host (Lagom).
//
// The catalog is untrusted data: it is only ever displayed, never executed, and identifiers are validated before
// they are shown. This code does NOT verify catalog or package signatures; see docs/INTEGRATION.md.

#pragma once

#include <AK/ByteString.h>
#include <AK/StringView.h>
#include <AK/Vector.h>

namespace ExileStore {

// Upper bounds applied while parsing, so a hostile or corrupt catalog cannot exhaust memory.
constexpr size_t max_catalog_bytes = 8 * 1024 * 1024;
constexpr size_t max_catalog_entries = 5000;
constexpr size_t max_short_field = 160;
constexpr size_t max_long_field = 4096;

struct Entry {
    ByteString id;
    ByteString name;
    ByteString summary;
    ByteString description;
    ByteString publisher;
    ByteString version;
    ByteString category;
    ByteString license;
    ByteString keywords;            // space separated, used for search only
    Vector<ByteString> permissions; // "id: reason"
    Vector<ByteString> packages;    // human readable one-liners, never executed
};

enum class LoadStatus {
    Loaded, // catalog parsed, at least one valid entry
    Empty,  // catalog parsed successfully but contains no valid entries
    Failed, // catalog could not be read or parsed; `message` says why
};

struct LoadResult {
    Vector<Entry> entries;
    LoadStatus status { LoadStatus::Failed };
    ByteString message;   // specific reason for Failed / Empty; empty on success
    size_t skipped { 0 }; // entries rejected by validation
};

// Parses the catalog document. Never crashes on malformed input; returns Failed with a specific message instead.
LoadResult parse_catalog(StringView json_text);

// Builds a Failed result (used when the file itself cannot be opened or read).
LoadResult failed_load(ByteString message);

// Reverse-DNS style identifier: 2..n dot separated segments of [a-z0-9-], at most 128 bytes in total.
bool is_valid_app_id(StringView);

// Removes control characters (newline and tab are kept when `allow_newlines` is set) and truncates to `max_bytes`
// on a UTF-8 boundary.
ByteString sanitize_text(StringView, size_t max_bytes, bool allow_newlines = false);

// Case-insensitive match on name, summary, category, publisher, keywords and id.
bool entry_matches(Entry const&, StringView query);

// Indices of the entries matching `query`, in catalog order.
Vector<size_t> filter_entries(Vector<Entry> const&, StringView query);

// The status line for the current load result and filter. A failed load always reports its own error and is never
// shown as "0 of 0".
ByteString status_text(LoadResult const&, size_t shown);

} // namespace ExileStore
