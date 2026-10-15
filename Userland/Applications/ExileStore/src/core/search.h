#pragma once
// Catalog search and filtering. Popularity sorting is deliberately absent: the
// catalog carries no trustworthy download statistics.

#include <string>
#include <string_view>
#include <vector>

#include "core/device.h"
#include "core/model.h"

namespace exile {

enum class SortOrder { Relevance, RecentlyUpdated, Name };

struct SearchQuery {
    std::string text;            // free text; every word must match somewhere
    std::string categoryId;      // empty => all categories
    bool onlyCompatible = false; // hide apps that cannot be installed on this device
    SortOrder sort = SortOrder::Relevance;
};

struct SearchHit {
    const AppEntry* app = nullptr;
    int score = 0;
    Compatibility compat;
};

std::vector<std::string> tokenize(std::string_view text);
std::vector<SearchHit> searchCatalog(const Catalog& catalog, const SearchQuery& query, const DeviceProfile& device);

}  // namespace exile
