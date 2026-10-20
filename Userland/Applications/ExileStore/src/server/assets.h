/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#pragma once

// The web front-end (web/*) is compiled into the binary so the store is one
// self-contained executable with nothing extra to deploy or to tamper with on disk.
// The table is generated at build time by cmake/embed_assets.cmake.

#include <cstddef>
#include <string_view>

namespace exile::server {

struct Asset {
    char const* path; // e.g. "/app.js"
    char const* mime;
    unsigned char const* data;
    size_t size;
};

// "/" maps to the index page. Returns nullptr when there is no such asset.
Asset const* findAsset(std::string_view path);

} // namespace exile::server
