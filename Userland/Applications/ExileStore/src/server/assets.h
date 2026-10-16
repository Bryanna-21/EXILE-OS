#pragma once
// The web front-end (web/*) is compiled into the binary so the store is one
// self-contained executable with nothing extra to deploy or to tamper with on disk.
// The table is generated at build time by cmake/embed_assets.cmake.

#include <cstddef>
#include <string_view>

namespace exile::server {

struct Asset {
    const char* path;  // e.g. "/app.js"
    const char* mime;
    const unsigned char* data;
    size_t size;
};

// "/" maps to the index page. Returns nullptr when there is no such asset.
const Asset* findAsset(std::string_view path);

}  // namespace exile::server
