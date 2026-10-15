#pragma once
// Pure rendering: (state in, text lines out). No I/O, so it is unit-testable and
// can be swapped for a graphical toolkit later while keeping the same model.

#include <string>
#include <vector>

#include "core/appstate.h"
#include "core/catalog.h"
#include "core/device.h"
#include "core/library.h"
#include "core/search.h"
#include "core/trust.h"
#include "ui/text.h"

namespace exile::ui {

struct ViewContext {
    const Catalog* catalog = nullptr;  // null when no catalog could be loaded
    const Library* library = nullptr;
    const DeviceProfile* device = nullptr;
    const TrustStore* trust = nullptr;
    const ISignatureVerifier* verifier = nullptr;
    bool backendCanLaunch = false;
    std::string catalogNotice;  // offline / error banner
};

enum class ItemKind { None, App, Category };
enum class NavTab { Home, Explore, Search, Library, None };

struct Page {
    std::vector<std::string> lines;
    std::vector<std::string> items;  // ids reachable by typing their number
    ItemKind kind = ItemKind::None;
    NavTab tab = NavTab::None;
};

struct RenderOptions {
    size_t width = 80;  // width available to the page content
    Theme theme;
};

// Wide terminals get a sidebar; narrow ones get a bottom navigation bar.
constexpr size_t kWideLayoutMinWidth = 72;
constexpr size_t kSidebarWidth = 18;
bool isWideLayout(size_t totalWidth);
size_t contentWidth(size_t totalWidth);

std::string describePermission(const std::string& id);

Page renderHome(const ViewContext& ctx, const RenderOptions& opts);
Page renderExplore(const ViewContext& ctx, const RenderOptions& opts);
Page renderSearch(const ViewContext& ctx, const RenderOptions& opts, const SearchQuery& query);
Page renderLibrary(const ViewContext& ctx, const RenderOptions& opts);
Page renderDetails(const ViewContext& ctx, const RenderOptions& opts, const std::string& appId);

// Surrounds a page with navigation (sidebar or bottom bar), an optional message and a key hint.
std::string renderFrame(const Page& page, size_t totalWidth, const Theme& theme, const std::string& message);

}  // namespace exile::ui
