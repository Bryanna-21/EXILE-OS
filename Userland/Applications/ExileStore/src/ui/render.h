/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#pragma once

// Pure rendering: (state in, text lines out). No I/O, so it is unit-testable and
// can be swapped for a graphical toolkit later while keeping the same model.

#include <string>
#include <vector>

#include "../core/appstate.h"
#include "../core/catalog.h"
#include "../core/device.h"
#include "../core/library.h"
#include "../core/search.h"
#include "../core/trust.h"
#include "text.h"

namespace exile::ui {

struct ViewContext {
    Catalog const* catalog = nullptr; // null when no catalog could be loaded
    Library const* library = nullptr;
    DeviceProfile const* device = nullptr;
    TrustStore const* trust = nullptr;
    ISignatureVerifier const* verifier = nullptr;
    bool backendCanLaunch = false;
    std::string catalogNotice; // offline / error banner
};

enum class ItemKind { None,
    App,
    Category };
enum class NavTab { Home,
    Explore,
    Search,
    Library,
    None };

struct Page {
    std::vector<std::string> lines;
    std::vector<std::string> items; // ids reachable by typing their number
    ItemKind kind = ItemKind::None;
    NavTab tab = NavTab::None;
};

struct RenderOptions {
    size_t width = 80; // width available to the page content
    Theme theme;
};

// Wide terminals get a sidebar; narrow ones get a bottom navigation bar.
constexpr size_t kWideLayoutMinWidth = 72;
constexpr size_t kSidebarWidth = 18;
bool isWideLayout(size_t totalWidth);
size_t contentWidth(size_t totalWidth);

std::string describePermission(std::string const& id);

Page renderHome(ViewContext const& ctx, RenderOptions const& opts);
Page renderExplore(ViewContext const& ctx, RenderOptions const& opts);
Page renderSearch(ViewContext const& ctx, RenderOptions const& opts, SearchQuery const& query);
Page renderLibrary(ViewContext const& ctx, RenderOptions const& opts);
Page renderDetails(ViewContext const& ctx, RenderOptions const& opts, std::string const& appId);

// Surrounds a page with navigation (sidebar or bottom bar), an optional message and a key hint.
std::string renderFrame(Page const& page, size_t totalWidth, Theme const& theme, std::string const& message);

} // namespace exile::ui
