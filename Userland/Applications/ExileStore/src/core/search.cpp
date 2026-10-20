/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include "search.h"

#include <algorithm>

#include "util.h"

namespace exile {

std::vector<std::string> tokenize(std::string_view text)
{
    std::vector<std::string> out;
    std::string cur;
    for (char c : text) {
        bool word = (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || static_cast<unsigned char>(c) >= 0x80;
        if (word) {
            cur.push_back(c);
        } else if (!cur.empty()) {
            out.push_back(util::toLower(cur));
            cur.clear();
        }
    }
    if (!cur.empty())
        out.push_back(util::toLower(cur));
    return out;
}

namespace {

// Score of one token against one app; 0 means "no match".
int scoreToken(AppEntry const& app, Catalog const& cat, std::string const& tok)
{
    int best = 0;
    std::string name = util::toLower(app.name);
    if (name == tok)
        best = std::max(best, 100);
    else if (name.rfind(tok, 0) == 0)
        best = std::max(best, 80);
    else if (name.find(tok) != std::string::npos)
        best = std::max(best, 60);
    for (auto const& w : tokenize(app.name))
        if (w.rfind(tok, 0) == 0)
            best = std::max(best, 70);

    if (util::containsIgnoreCase(app.publisher.name, tok))
        best = std::max(best, 40);
    for (auto const& k : app.keywords)
        if (util::containsIgnoreCase(k, tok))
            best = std::max(best, 45);
    for (auto const& cid : app.categories) {
        Category const* c = cat.findCategory(cid);
        if (c && util::containsIgnoreCase(c->name, tok))
            best = std::max(best, 35);
    }
    if (util::containsIgnoreCase(app.summary, tok))
        best = std::max(best, 30);
    if (util::containsIgnoreCase(app.description, tok))
        best = std::max(best, 10);
    if (util::containsIgnoreCase(app.appId, tok))
        best = std::max(best, 20);
    return best;
}

} // namespace

std::vector<SearchHit> searchCatalog(Catalog const& cat, SearchQuery const& q, DeviceProfile const& dev)
{
    std::vector<std::string> tokens = tokenize(q.text);
    std::vector<SearchHit> hits;

    for (AppEntry const& app : cat.apps) {
        if (!isListedFor(app, dev))
            continue;
        if (!q.categoryId.empty() && std::find(app.categories.begin(), app.categories.end(), q.categoryId) == app.categories.end())
            continue;

        int total = 0;
        bool matchedAll = true;
        for (auto const& t : tokens) {
            int s = scoreToken(app, cat, t);
            if (s == 0) {
                matchedAll = false;
                break;
            }
            total += s;
        }
        if (!matchedAll)
            continue;

        SearchHit hit;
        hit.app = &app;
        hit.score = total;
        hit.compat = resolveCompatibility(app, dev);
        if (q.onlyCompatible && !hit.compat.ok())
            continue;
        hits.push_back(std::move(hit));
    }

    auto byName = [](SearchHit const& a, SearchHit const& b) {
        return util::toLower(a.app->name) < util::toLower(b.app->name);
    };
    switch (q.sort) {
    case SortOrder::Relevance:
        std::stable_sort(hits.begin(), hits.end(), [&](SearchHit const& a, SearchHit const& b) {
            if (a.score != b.score)
                return a.score > b.score;
            return byName(a, b);
        });
        break;
    case SortOrder::RecentlyUpdated:
        std::stable_sort(hits.begin(), hits.end(), [&](SearchHit const& a, SearchHit const& b) {
            if (a.app->updatedAt != b.app->updatedAt)
                return a.app->updatedAt > b.app->updatedAt;
            return byName(a, b);
        });
        break;
    case SortOrder::Name:
        std::stable_sort(hits.begin(), hits.end(), byName);
        break;
    }
    return hits;
}

} // namespace exile
