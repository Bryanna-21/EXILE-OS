#pragma once
// Catalog parsing and validation. Catalog data is untrusted input.
//
// Failure model:
//  * A malformed document (bad JSON, wrong schema version, missing top-level
//    arrays) yields no catalog at all.
//  * A malformed individual app is skipped and reported as an issue, so one bad
//    entry never hides the rest of the catalog.

#include <filesystem>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

#include "core/model.h"

namespace exile {

struct CatalogIssue {
    std::string where;    // e.g. "apps[3] (org.example.app)"
    std::string message;
};

struct CatalogLoadResult {
    std::optional<Catalog> catalog;
    std::vector<CatalogIssue> issues;
    bool ok() const { return catalog.has_value(); }
};

constexpr int kCatalogSchemaVersion = 1;
constexpr uint64_t kMaxCatalogBytes = 16ull * 1024 * 1024;

CatalogLoadResult parseCatalog(std::string_view jsonText, const std::filesystem::path& baseDir);
CatalogLoadResult loadCatalogFile(const std::filesystem::path& file);

// Shared validators (also used by the transport layer and tests).
bool isValidAppId(std::string_view id);
// "https://host/..." | "file:///abs/path" | safe relative path. Rejects http:// and any other scheme.
bool isAcceptableSourceUrl(std::string_view url, std::string* reason = nullptr);

}  // namespace exile
