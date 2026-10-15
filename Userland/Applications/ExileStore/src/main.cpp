// Exile Store - desktop front-end (terminal edition) and command-line interface.
//
// This is the first functional front-end. It drives the same UiController and
// renderers that a graphical toolkit would use; only the I/O loop is terminal
// specific. See docs/ARCHITECTURE.md.

#include <atomic>
#include <chrono>
#include <csignal>
#include <cstdlib>
#include <iostream>
#include <memory>
#include <sstream>
#include <string>
#include <vector>

#include "core/appstate.h"
#include "core/backend.h"
#include "core/catalogclient.h"
#include "core/device.h"
#include "core/installer.h"
#include "core/library.h"
#include "core/search.h"
#include "core/trust.h"
#include "core/util.h"
#include "core/version.h"
#include "ui/controller.h"

#if defined(__unix__) || defined(__APPLE__)
#include <unistd.h>
#define EXILE_HAVE_ISATTY 1
#endif

namespace fs = std::filesystem;
using namespace exile;

namespace {

std::atomic<CancelToken*> g_cancel{nullptr};
extern "C" void onSigint(int) {
    if (CancelToken* t = g_cancel.load()) t->cancel();
}

bool stdoutIsTty() {
#ifdef EXILE_HAVE_ISATTY
    return ::isatty(1) != 0;
#else
    return false;
#endif
}

struct Options {
    fs::path catalog;
    fs::path dataDir;
    fs::path trust;
    DeviceProfile device = detectHostDevice();
    size_t width = 80;
    bool color = false;
    bool allowFileUrls = false;
    std::vector<std::string> command;
};

void usage(std::ostream& os) {
    os << "Exile Store " << "0.1.0 (development build)\n\n"
          "Usage: exile-store [options] [command]\n\n"
          "Commands (no command starts the interactive store):\n"
          "  home                     show the home screen\n"
          "  search <words>           search apps\n"
          "  show <appId>             show an app's details page\n"
          "  install <appId>          install an app\n"
          "  update <appId> | --all   update one app or every app with an update\n"
          "  uninstall <appId> [--force]\n"
          "  library                  show installed apps and updates\n"
          "  status                   show environment and security posture\n\n"
          "Options:\n"
          "  --catalog <file>         catalog JSON (default: $EXILE_STORE_CATALOG or data/catalog/catalog.json)\n"
          "  --trust <file>           trusted publisher keys (default: ../trust/trusted-keys.json next to the catalog)\n"
          "  --data-dir <dir>         where the store keeps its state (default: ~/.local/share/exile-store)\n"
          "  --device <class>         desktop | mobile | tablet | tv | other   (default: desktop)\n"
          "  --arch <arch>            x86_64 | aarch64 | riscv64              (default: this machine)\n"
          "  --os <version>           Exile OS version of the device          (default: 0.1.0)\n"
          "  --beta                   also show the beta release channel\n"
          "  --width <columns>        layout width, 40-400 (default: $COLUMNS or 80)\n"
          "  --color                  use colors\n"
          "  --allow-file-urls        allow absolute file:// package sources (development only)\n"
          "  -h, --help               this text\n";
}

bool parseArgs(int argc, char** argv, Options& o, std::string& err) {
    if (const char* c = std::getenv("COLUMNS")) {
        long w = std::strtol(c, nullptr, 10);
        if (w >= 40 && w <= 400) o.width = static_cast<size_t>(w);
    }
    if (const char* c = std::getenv("EXILE_STORE_CATALOG")) o.catalog = c;
    for (int i = 1; i < argc; ++i) {
        std::string a = argv[i];
        auto next = [&](std::string& out) {
            if (i + 1 >= argc) { err = "missing value for " + a; return false; }
            out = argv[++i];
            return true;
        };
        std::string v;
        if (a == "-h" || a == "--help") { usage(std::cout); std::exit(0); }
        else if (a == "--catalog") { if (!next(v)) return false; o.catalog = v; }
        else if (a == "--trust") { if (!next(v)) return false; o.trust = v; }
        else if (a == "--data-dir") { if (!next(v)) return false; o.dataDir = v; }
        else if (a == "--device") {
            if (!next(v)) return false;
            auto d = parseDeviceClass(v);
            if (!d) { err = "unknown device class '" + v + "'"; return false; }
            o.device.deviceClass = *d;
        } else if (a == "--arch") {
            if (!next(v)) return false;
            auto d = parseArch(v);
            if (!d || *d == Arch::Any) { err = "unknown architecture '" + v + "'"; return false; }
            o.device.arch = *d;
        } else if (a == "--os") {
            if (!next(v)) return false;
            if (!parseVersion(v)) { err = "invalid version '" + v + "'"; return false; }
            o.device.osVersion = v;
        } else if (a == "--beta") o.device.enabledChannels.push_back(ReleaseChannel::Beta);
        else if (a == "--width") {
            if (!next(v)) return false;
            long w = std::strtol(v.c_str(), nullptr, 10);
            if (w < 40 || w > 400) { err = "--width must be between 40 and 400"; return false; }
            o.width = static_cast<size_t>(w);
        } else if (a == "--color") o.color = true;
        else if (a == "--allow-file-urls") o.allowFileUrls = true;
        else if (!a.empty() && a[0] == '-' && o.command.empty()) { err = "unknown option " + a; return false; }
        else o.command.push_back(a);
    }
    return true;
}

fs::path defaultDataDir() {
    if (const char* x = std::getenv("XDG_DATA_HOME")) return fs::path(x) / "exile-store";
    if (const char* h = std::getenv("HOME")) return fs::path(h) / ".local/share/exile-store";
    return fs::path(".exile-store");
}

fs::path findDefaultCatalog() {
    for (fs::path p : {fs::path("data/catalog/catalog.json"), fs::path("../data/catalog/catalog.json"),
                       fs::path("/usr/local/share/exile-store/catalog/catalog.json"),
                       fs::path("/usr/share/exile-store/catalog/catalog.json")}) {
        std::error_code ec;
        if (fs::exists(p, ec)) return p;
    }
    return "data/catalog/catalog.json";
}

// Everything the front-end needs, with stable addresses.
struct App {
    Options opts;
    CatalogState catalogState;
    TrustStore trust;
    std::string trustNotice;
    std::unique_ptr<ISignatureVerifier> verifier = makeDefaultVerifier();
    std::unique_ptr<Library> library;
    std::unique_ptr<LocalPrefixBackend> backend;
    std::unique_ptr<LocalTransport> local;
    UnavailableHttpsTransport https;
    std::unique_ptr<RoutingTransport> transport;
    std::string libraryNotice;

    ui::ViewContext view() const {
        ui::ViewContext v;
        v.catalog = catalogState.catalog ? &*catalogState.catalog : nullptr;
        v.library = library.get();
        v.device = &opts.device;
        v.trust = &trust;
        v.verifier = verifier.get();
        v.backendCanLaunch = backend->canLaunch();
        v.catalogNotice = catalogState.notice;
        return v;
    }

    InstallerEnv env() {
        InstallerEnv e;
        e.catalog = catalogState.catalog ? &*catalogState.catalog : nullptr;
        e.device = &opts.device;
        e.library = library.get();
        e.backend = backend.get();
        e.transport = transport.get();
        e.trust = &trust;
        e.verifier = verifier.get();
        e.cacheDir = opts.dataDir / "downloads";
        return e;
    }
};

void setup(App& app) {
    Options& o = app.opts;
    if (o.dataDir.empty()) o.dataDir = defaultDataDir();
    if (o.catalog.empty()) o.catalog = findDefaultCatalog();
    if (o.trust.empty()) {
        fs::path base = o.catalog.has_parent_path() ? o.catalog.parent_path() : fs::path(".");
        o.trust = base.parent_path() / "trust" / "trusted-keys.json";
    }
    std::error_code ec;
    fs::create_directories(o.dataDir, ec);

    FileCatalogSource source(o.catalog);
    app.catalogState = loadCatalog(source, o.dataDir / "catalog-cache.json");

    std::string terr;
    if (auto ts = TrustStore::loadFile(o.trust.string(), terr)) app.trust = std::move(*ts);
    else app.trustNotice = "No trusted publisher keys could be loaded (" + terr + "). Installs are disabled.";

    app.library = std::make_unique<Library>(o.dataDir / "library.json");
    std::string lerr;
    if (!app.library->load(lerr)) app.libraryNotice = lerr;

    app.backend = std::make_unique<LocalPrefixBackend>(o.dataDir / "prefix");
    app.backend->recoverInterrupted();
    fs::path base = app.catalogState.catalog ? fs::path(app.catalogState.catalog->baseDir) : o.catalog.parent_path();
    app.local = std::make_unique<LocalTransport>(base, o.allowFileUrls);
    app.transport = std::make_unique<RoutingTransport>(*app.local, app.https);
}

ui::RenderOptions renderOptions(const App& app) {
    ui::RenderOptions r;
    r.width = app.opts.width;
    r.theme = ui::Theme(app.opts.color);
    return r;
}

// Runs one install/update, drawing progress. Returns the result.
JobResult runJob(App& app, const std::string& appId, JobKind kind, std::ostream& out) {
    InstallJob job(app.env(), appId, kind);
    g_cancel.store(&job.cancelToken());
    std::signal(SIGINT, onSigint);
    bool tty = stdoutIsTty();
    auto fut = job.runAsync();
    JobState last = JobState::Queued;
    size_t lastLen = 0;
    while (fut.wait_for(std::chrono::milliseconds(80)) != std::future_status::ready) {
        JobProgress p = job.progress();
        std::string line = p.message;
        if (p.state == JobState::Downloading && p.total > 0)
            line = p.message + " " + ui::progressBar(static_cast<double>(p.done) / static_cast<double>(p.total), 24) + " " +
                   util::humanSize(p.done) + " / " + util::humanSize(p.total);
        if (tty) {
            out << '\r' << ui::padRight(line, lastLen) << std::flush;
            lastLen = std::max(lastLen, line.size());
        } else if (p.state != last) {
            out << "  " << line << '\n';
        }
        last = p.state;
    }
    JobResult r = fut.get();
    g_cancel.store(nullptr);
    std::signal(SIGINT, SIG_DFL);
    if (tty) out << '\r' << std::string(lastLen + 2, ' ') << '\r';
    return r;
}

std::string describeResult(const App& app, const std::string& appId, JobKind kind, const JobResult& r) {
    const AppEntry* a = app.catalogState.catalog ? app.catalogState.catalog->find(appId) : nullptr;
    std::string name = a ? a->name : appId;
    if (r.ok()) {
        const InstalledApp* i = app.library->find(appId);
        std::string m = name + (kind == JobKind::Update ? " was updated" : " was installed") +
                        (i ? " (version " + i->version + ")." : ".");
        if (!r.detail.empty()) m += " " + r.detail;
        return m;
    }
    return r.error;
}

int printCatalogProblems(const App& app) {
    if (!app.catalogState.catalog) {
        std::cerr << app.catalogState.notice << "\n";
        return 1;
    }
    return 0;
}

int cmdUpdateAll(App& app, std::ostream& out) {
    if (!app.catalogState.catalog) { std::cerr << app.catalogState.notice << "\n"; return 1; }
    auto updates = findUpdates(*app.catalogState.catalog, *app.library, app.opts.device);
    if (updates.empty()) { out << "Everything is up to date.\n"; return 0; }
    int failures = 0;
    for (const auto& u : updates) {
        out << "Updating " << u.app->name << " " << u.installedVersion << " -> " << u.newVersion << "\n";
        JobResult r = runJob(app, u.app->appId, JobKind::Update, out);
        out << "  " << describeResult(app, u.app->appId, JobKind::Update, r) << "\n";
        if (!r.ok()) ++failures;
    }
    return failures ? 1 : 0;
}

int interactive(App& app) {
    ui::UiController ui(app.view(), app.opts.width, ui::Theme(app.opts.color));
    std::string startup;
    if (!app.libraryNotice.empty()) startup = app.libraryNotice;
    if (!app.trustNotice.empty()) startup += (startup.empty() ? "" : "\n") + app.trustNotice;
    ui.setMessage(startup);
    while (true) {
        std::cout << "\n" << ui.render() << "\n> " << std::flush;
        std::string line;
        if (!std::getline(std::cin, line)) { std::cout << "\n"; return 0; }
        ui::Command cmd = ui.handle(line);
        using K = ui::Command::Kind;
        switch (cmd.kind) {
            case K::Quit: return 0;
            case K::Install:
            case K::Update: {
                JobKind kind = cmd.kind == K::Update ? JobKind::Update : JobKind::Install;
                JobResult r = runJob(app, cmd.appId, kind, std::cout);
                ui.setMessage(describeResult(app, cmd.appId, kind, r));
                break;
            }
            case K::UpdateAll: {
                std::ostringstream captured;
                cmdUpdateAll(app, captured);
                ui.setMessage(captured.str());
                break;
            }
            case K::Uninstall: {
                const InstalledApp* i = app.library->find(cmd.appId);
                std::cout << "Uninstall " << (i && !i->name.empty() ? i->name : cmd.appId) << "? (y/N) " << std::flush;
                std::string answer;
                std::getline(std::cin, answer);
                if (answer != "y" && answer != "Y" && answer != "yes") { ui.setMessage("Nothing was changed."); break; }
                InstallerEnv env = app.env();
                UninstallResult ur = uninstallApp(env, cmd.appId);
                ui.setMessage(ur.ok ? "Uninstalled." : ur.error);
                break;
            }
            case K::Open: {
                BackendResult br = app.backend->launch(cmd.appId);
                ui.setMessage(br.ok ? "Opening..." : br.error);
                break;
            }
            case K::None: break;
        }
    }
}

int cmdStatus(const App& app, std::ostream& out) {
    out << "Exile Store 0.1.0 (development build)\n";
    out << "Device:     " << app.opts.device.describe() << "\n";
    out << "Catalog:    " << app.opts.catalog.string() << "\n";
    if (app.catalogState.catalog)
        out << "            " << app.catalogState.catalog->apps.size() << " apps" << (app.catalogState.fromCache ? " (saved copy, catalog unreachable)" : "")
            << ", " << app.catalogState.issues.size() << " entries skipped as invalid\n";
    else out << "            not loaded\n";
    for (const auto& i : app.catalogState.issues) out << "            skipped " << i.where << ": " << i.message << "\n";
    out << "Trust keys: " << app.trust.size() << " (" << app.opts.trust.string() << ")\n";
    out << "Verifier:   " << app.verifier->name() << (app.verifier->available() ? "" : "  -> INSTALLS DISABLED") << "\n";
    out << "Installer:  " << app.backend->name() << " at " << app.backend->storagePath().string() << "\n";
    out << "Downloads:  local files only; https needs a TLS transport that is not linked in this build\n\n";
    out << "SECURITY: this is NOT production-secure. The installer is an unprivileged development\n"
           "backend with no sandboxing or permission enforcement, and the trust store is a local\n"
           "file. See docs/SECURITY.md before treating any install as safe.\n";
    return 0;
}

}  // namespace

int main(int argc, char** argv) {
    App app;
    std::string err;
    if (!parseArgs(argc, argv, app.opts, err)) {
        std::cerr << "exile-store: " << err << "\n\n";
        usage(std::cerr);
        return 2;
    }
    setup(app);
    if (!app.libraryNotice.empty()) std::cerr << app.libraryNotice << "\n";
    if (!app.trustNotice.empty()) std::cerr << app.trustNotice << "\n";

    const auto& c = app.opts.command;
    if (c.empty()) return interactive(app);

    ui::RenderOptions ro = renderOptions(app);
    ui::ViewContext vc = app.view();
    auto printPage = [&](const ui::Page& p) {
        for (const auto& l : p.lines) std::cout << l << "\n";
    };
    const std::string& cmd = c[0];

    if (cmd == "status") return cmdStatus(app, std::cout);
    if (cmd == "home") { if (!vc.catalog) return printCatalogProblems(app); printPage(ui::renderHome(vc, ro)); return 0; }
    if (cmd == "library") { printPage(ui::renderLibrary(vc, ro)); return 0; }
    if (cmd == "search") {
        if (!vc.catalog) return printCatalogProblems(app);
        SearchQuery q;
        for (size_t i = 1; i < c.size(); ++i) q.text += (i > 1 ? " " : "") + c[i];
        printPage(ui::renderSearch(vc, ro, q));
        return 0;
    }
    if (cmd == "show") {
        if (c.size() < 2) { std::cerr << "usage: show <appId>\n"; return 2; }
        printPage(ui::renderDetails(vc, ro, c[1]));
        return 0;
    }
    if (cmd == "install" || cmd == "update") {
        if (c.size() < 2) { std::cerr << "usage: " << cmd << " <appId>\n"; return 2; }
        if (cmd == "update" && c[1] == "--all") return cmdUpdateAll(app, std::cout);
        if (!vc.catalog) return printCatalogProblems(app);
        JobKind kind = cmd == "update" ? JobKind::Update : JobKind::Install;
        JobResult r = runJob(app, c[1], kind, std::cout);
        std::cout << describeResult(app, c[1], kind, r) << "\n";
        return r.ok() ? 0 : 1;
    }
    if (cmd == "uninstall") {
        if (c.size() < 2) { std::cerr << "usage: uninstall <appId> [--force]\n"; return 2; }
        bool force = c.size() > 2 && c[2] == "--force";
        InstallerEnv env = app.env();
        UninstallResult ur = uninstallApp(env, c[1], force);
        std::cout << (ur.ok ? "Uninstalled." : ur.error) << "\n";
        return ur.ok ? 0 : 1;
    }
    std::cerr << "exile-store: unknown command '" << cmd << "'\n\n";
    usage(std::cerr);
    return 2;
}
