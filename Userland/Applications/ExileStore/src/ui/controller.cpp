#include "ui/controller.h"

#include <algorithm>
#include <cctype>

#include "core/util.h"

namespace exile::ui {

namespace {
std::string trim(const std::string& s) {
    size_t a = 0, b = s.size();
    while (a < b && std::isspace(static_cast<unsigned char>(s[a]))) ++a;
    while (b > a && std::isspace(static_cast<unsigned char>(s[b - 1]))) --b;
    return s.substr(a, b - a);
}
bool isNumber(const std::string& s) {
    return !s.empty() && s.size() <= 6 && std::all_of(s.begin(), s.end(), [](char c) { return c >= '0' && c <= '9'; });
}
}  // namespace

std::string helpText() {
    return "h home | e explore | l library | / words to search | number to open an item\n"
           "On an app: i install | u update | o open | x uninstall | b back\n"
           "In search: c toggle 'works on this device only' | sort name / updated / relevance\n"
           "Library: ua update everything | q quit";
}

UiController::UiController(const ViewContext& ctx, size_t width, Theme theme) : ctx_(ctx), width_(width), theme_(theme) {}

void UiController::push() {
    back_.push_back(nav_);
    if (back_.size() > 50) back_.erase(back_.begin());
}

void UiController::go(Screen s) {
    if (nav_.screen == s && s != Screen::Search) return;
    push();
    nav_.screen = s;
    if (s != Screen::Search) nav_.query = SearchQuery{};
}

Page UiController::currentPage() const {
    RenderOptions o;
    o.width = contentWidth(width_);
    o.theme = theme_;
    switch (nav_.screen) {
        case Screen::Home: return renderHome(ctx_, o);
        case Screen::Explore: return renderExplore(ctx_, o);
        case Screen::Search: return renderSearch(ctx_, o, nav_.query);
        case Screen::Library: return renderLibrary(ctx_, o);
        case Screen::Details: return renderDetails(ctx_, o, nav_.appId);
    }
    return {};
}

std::string UiController::render() {
    std::string out = renderFrame(currentPage(), width_, theme_, message_);
    message_.clear();
    return out;
}

Command UiController::openItem(size_t number) {
    Page page = currentPage();
    if (number == 0 || number > page.items.size()) {
        message_ = "There is no item " + std::to_string(number) + " on this screen.";
        return {};
    }
    const std::string& id = page.items[number - 1];
    if (page.kind == ItemKind::Category) {
        push();
        nav_.screen = Screen::Search;
        nav_.query = SearchQuery{};
        nav_.query.categoryId = id;
    } else if (page.kind == ItemKind::App) {
        push();
        nav_.screen = Screen::Details;
        nav_.appId = id;
    }
    return {};
}

Command UiController::handle(const std::string& rawLine) {
    std::string line = trim(rawLine);
    if (line.empty()) return {};

    // "/words" and "/ words" both search.
    if (line[0] == '/') {
        std::string text = trim(line.substr(1));
        push();
        nav_.screen = Screen::Search;
        bool keepFilter = nav_.query.onlyCompatible;
        nav_.query = SearchQuery{};
        nav_.query.text = text;
        nav_.query.onlyCompatible = keepFilter;
        if (text.empty()) message_ = "Type / followed by what you want to find, e.g.  / notes";
        return {};
    }

    size_t sp = line.find(' ');
    std::string cmd = util::toLower(line.substr(0, sp));
    std::string rest = sp == std::string::npos ? "" : trim(line.substr(sp + 1));

    if (isNumber(cmd)) return openItem(static_cast<size_t>(std::stoul(cmd)));

    if (cmd == "q" || cmd == "quit" || cmd == "exit") return {Command::Kind::Quit, {}};
    if (cmd == "?" || cmd == "help") { message_ = helpText(); return {}; }
    if (cmd == "h" || cmd == "home") { go(Screen::Home); return {}; }
    if (cmd == "e" || cmd == "explore") { go(Screen::Explore); return {}; }
    if (cmd == "l" || cmd == "library") { go(Screen::Library); return {}; }
    if (cmd == "s" || cmd == "search") {
        push();
        nav_.screen = Screen::Search;
        bool keepFilter = nav_.query.onlyCompatible;
        nav_.query = SearchQuery{};
        nav_.query.text = rest;
        nav_.query.onlyCompatible = keepFilter;
        if (rest.empty()) message_ = "Type  s notes  (or  / notes ) to search.";
        return {};
    }
    if (cmd == "b" || cmd == "back") {
        if (back_.empty()) { message_ = "Nothing to go back to."; return {}; }
        nav_ = back_.back();
        back_.pop_back();
        return {};
    }
    if (cmd == "c" && nav_.screen == Screen::Search) {
        nav_.query.onlyCompatible = !nav_.query.onlyCompatible;
        return {};
    }
    if (cmd == "sort" && nav_.screen == Screen::Search) {
        std::string k = util::toLower(rest);
        if (k == "name") nav_.query.sort = SortOrder::Name;
        else if (k == "updated") nav_.query.sort = SortOrder::RecentlyUpdated;
        else if (k == "relevance") nav_.query.sort = SortOrder::Relevance;
        else message_ = "Sort by: name, updated or relevance.";
        return {};
    }
    if (cmd == "ua" || cmd == "updateall") {
        if (nav_.screen != Screen::Library) { message_ = "Open your Library (l) to update everything."; return {}; }
        return {Command::Kind::UpdateAll, {}};
    }

    bool onDetails = nav_.screen == Screen::Details;
    if (cmd == "i" || cmd == "install" || cmd == "u" || cmd == "update" || cmd == "x" || cmd == "uninstall" ||
        cmd == "remove" || cmd == "o" || cmd == "open") {
        if (!onDetails) { message_ = "Open an app first (type its number), then use this command."; return {}; }
        const AppEntry* app = ctx_.catalog ? ctx_.catalog->find(nav_.appId) : nullptr;
        const InstalledApp* inst = ctx_.library->find(nav_.appId);

        if (cmd == "x" || cmd == "uninstall" || cmd == "remove") {
            if (!inst) { message_ = "This app isn't installed."; return {}; }
            return {Command::Kind::Uninstall, nav_.appId};
        }
        if (cmd == "o" || cmd == "open") {
            if (!inst) { message_ = "This app isn't installed yet."; return {}; }
            if (!ctx_.backendCanLaunch) { message_ = "Opening apps isn't available with the development installer."; return {}; }
            return {Command::Kind::Open, nav_.appId};
        }
        if (!app) { message_ = "This app is not in the catalog."; return {}; }
        AppStatus st = computeStatus(*app, *ctx_.library, *ctx_.device, ctx_.backendCanLaunch);
        bool wantInstall = cmd == "i" || cmd == "install";
        if (wantInstall && st.action == PrimaryAction::Install) return {Command::Kind::Install, nav_.appId};
        if (!wantInstall && st.action == PrimaryAction::Update) return {Command::Kind::Update, nav_.appId};
        if (st.action == PrimaryAction::Unavailable) message_ = st.note;
        else if (wantInstall && st.action == PrimaryAction::Update) message_ = "Already installed. Use u to update to the new version.";
        else if (wantInstall) message_ = "Already installed.";
        else message_ = "No update is available for this app.";
        return {};
    }

    message_ = "Unknown command '" + cmd + "'. Type ? for help.";
    return {};
}

}  // namespace exile::ui
