/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#pragma once

// Catalog retrieval with an offline fallback. A refreshed catalog is validated
// before it replaces the cache, so a bad download can never poison the cache.

#include <filesystem>
#include <optional>
#include <string>
#include <vector>

#include "catalog.h"

namespace exile {

class ICatalogSource {
public:
    virtual ~ICatalogSource() = default;
    virtual bool fetch(std::string& text, std::string& error) = 0;
    virtual std::filesystem::path baseDir() const = 0; // resolves relative package paths
    virtual std::string describe() const = 0;
};

class FileCatalogSource final : public ICatalogSource {
public:
    explicit FileCatalogSource(std::filesystem::path file)
        : file_(std::move(file))
    {
    }
    bool fetch(std::string& text, std::string& error) override;
    std::filesystem::path baseDir() const override { return file_.has_parent_path() ? file_.parent_path() : "."; }
    std::string describe() const override { return file_.string(); }

private:
    std::filesystem::path file_;
};

struct CatalogState {
    std::optional<Catalog> catalog;
    bool fromCache = false; // true when the live source failed and saved data is shown
    std::string notice;     // user-facing message, empty when everything is fine
    std::vector<CatalogIssue> issues;
};

CatalogState loadCatalog(ICatalogSource& source, std::filesystem::path const& cacheFile);

} // namespace exile
