// Exile OS Living Battlefield - host preview / benchmark / frame-stream tool.
//
// This is a HOST application (Linux, libpng only). It is not integrated with the Exile OS
// WindowServer; see docs/NATIVE_INTEGRATION.md for what that would take and what is missing.
#include "atlas.h"
#include "config.h"
#include "png_io.h"
#include "renderer.h"
#include "scene.h"
#include <chrono>
#include <csignal>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <ctime>
#include <fcntl.h>
#include <poll.h>
#include <sys/file.h>
#include <sys/resource.h>
#include <thread>
#include <unistd.h>

using namespace lb;
using Clock = std::chrono::steady_clock;

static volatile std::sig_atomic_t g_quit = 0;
static void on_signal(int) { g_quit = 1; }

static void usage()
{
    std::fprintf(stderr,
        "Exile OS Living Battlefield (host preview)\n"
        "  --assets DIR            asset directory (default: ./assets or <exe>/../assets)\n"
        "  --config FILE           settings INI (see config/living-battlefield.ini)\n"
        "  --width N --height N    output size (default 1920x1080)   --scale F  render scale   --threads N  (default 2)\n"
        "  --hour H | --phase P    time to render (phases: dawn morning midday afternoon sunset evening night predawn)\n"
        "  --anim T                animation clock in seconds for stills (default 2.5)\n"
        "  --out FILE.png          render one frame\n"
        "  --render-phases DIR     render the 8 named phases as PNGs\n"
        "  --sequence DIR --from H --to H --step H   render a time sequence (transition checks)\n"
        "  --benchmark N           time N frames (CPU); prints frame time / fps / memory\n"
        "  --selftest              continuity / determinism checks (no rendering)\n"
        "  --stream                raw RGBA frames to stdout at fps; commands on stdin:\n"
        "                          pause|resume|reset|phase NAME|hour H|speed X|cycle SECONDS|realtime on|off|lowpower on|off|quit\n"
        "  --save-config FILE      write effective settings and exit\n");
}

static std::string file_dir(char const* argv0)
{
    std::string p = argv0;
    auto s = p.find_last_of('/');
    return s == std::string::npos ? std::string(".") : p.substr(0, s);
}

static bool exists(std::string const& p) { return access(p.c_str(), R_OK) == 0; }

static long rss_kb()
{
    FILE* f = std::fopen("/proc/self/status", "r");
    if (!f) return -1;
    char line[256]; long kb = -1;
    while (std::fgets(line, sizeof line, f))
        if (!std::strncmp(line, "VmRSS:", 6)) { kb = std::atol(line + 6); break; }
    std::fclose(f);
    return kb;
}
static double cpu_seconds()
{
    rusage ru; getrusage(RUSAGE_SELF, &ru);
    return ru.ru_utime.tv_sec + ru.ru_utime.tv_usec * 1e-6 + ru.ru_stime.tv_sec + ru.ru_stime.tv_usec * 1e-6;
}

static float wall_hour()
{
    time_t t = time(nullptr); tm lt; localtime_r(&t, &lt);
    return lt.tm_hour + lt.tm_min / 60.0f + lt.tm_sec / 3600.0f;
}

int main(int argc, char** argv)
{
    Settings st;
    std::string assets, config, out, phases_dir, seq_dir, save_cfg;
    float hour = -1, anim = 2.5f, from = 0, to = 24, step = 0.5f;
    int cli_w = -1, cli_h = -1, cli_threads = -1; float cli_scale = -1; bool selftest = false;
    int bench = 0; bool stream = false; std::string phase;
    for (int i = 1; i < argc; ++i) {
        std::string a = argv[i];
        auto next = [&]() -> char const* { if (i + 1 >= argc) { usage(); std::exit(2); } return argv[++i]; };
        if (a == "--assets") assets = next();
        else if (a == "--config") config = next();
        else if (a == "--width") cli_w = std::atoi(next());
        else if (a == "--height") cli_h = std::atoi(next());
        else if (a == "--scale") cli_scale = std::atof(next());
        else if (a == "--threads") cli_threads = std::max(1, std::atoi(next()));
        else if (a == "--selftest") selftest = true;
        else if (a == "--hour") hour = std::atof(next());
        else if (a == "--phase") phase = next();
        else if (a == "--anim") anim = std::atof(next());
        else if (a == "--out") out = next();
        else if (a == "--render-phases") phases_dir = next();
        else if (a == "--sequence") seq_dir = next();
        else if (a == "--from") from = std::atof(next());
        else if (a == "--to") to = std::atof(next());
        else if (a == "--step") step = std::atof(next());
        else if (a == "--benchmark") bench = std::atoi(next());
        else if (a == "--stream") stream = true;
        else if (a == "--save-config") save_cfg = next();
        else if (a == "--help" || a == "-h") { usage(); return 0; }
        else { std::fprintf(stderr, "unknown option %s\n", a.c_str()); usage(); return 2; }
    }
    std::string err;
    if (!config.empty()) {
        if (!load_settings(config, st, err)) { std::fprintf(stderr, "%s\n", err.c_str()); return 1; }
    }
    if (cli_w > 0) st.width = cli_w;
    if (cli_h > 0) st.height = cli_h;
    if (cli_scale > 0) st.render_scale = cli_scale;
    if (cli_threads > 0) st.threads = cli_threads;
    if (!save_cfg.empty()) {
        if (!save_settings(save_cfg, st, err)) { std::fprintf(stderr, "%s\n", err.c_str()); return 1; }
        return 0;
    }
    if (!phase.empty() && !phase_hour(phase, hour)) { std::fprintf(stderr, "unknown phase '%s'\n", phase.c_str()); return 2; }
    if (assets.empty()) {
        for (std::string c : { std::string("assets"), file_dir(argv[0]) + "/../assets", file_dir(argv[0]) + "/assets" })
            if (exists(c + "/sprites.json")) { assets = c; break; }
        if (assets.empty()) { std::fprintf(stderr, "cannot find assets/ (use --assets)\n"); return 1; }
    }

    Atlas atlas;
    if (!atlas.load(assets, err)) { std::fprintf(stderr, "atlas: %s\n", err.c_str()); return 1; }
    Scene scene(atlas, st.seed);
    Renderer rend(atlas, st);
    if (!rend.init(assets, err)) { std::fprintf(stderr, "renderer: %s\n", err.c_str()); return 1; }
    std::vector<uint8_t> frame(size_t(rend.width()) * rend.height() * 4);
    auto save = [&](std::string const& path) {
        std::string e;
        if (!save_png_rgba(path, rend.width(), rend.height(), frame.data(), e)) { std::fprintf(stderr, "%s\n", e.c_str()); std::exit(1); }
    };

    if (selftest) {
        std::vector<Scene::EntityPose> a, b, c;
        int fails = 0; long checks = 0;
        double worst = 0; size_t worst_i = 0; float worst_h = 0;
        long mount_checks = 0; double mount_worst = 0;
        long invalid = 0;
        const float step_h = 0.01f;
        scene.entity_poses(0.0f, 0.0f, a);
        // 1) continuity: no entity may jump between consecutive 0.01 h steps (including 24:00 -> 00:00)
        for (int i = 1; i <= 2400 + 5; ++i) {
            float h = wrap24(float(i) * step_h);
            scene.entity_poses(h, 0.0f, b);
            for (size_t k = 0; k < a.size(); ++k) {
                if (!a[k].valid || !b[k].valid) { ++invalid; continue; }
                double dd = std::hypot(double(b[k].X - a[k].X), double(b[k].Z - a[k].Z));
                ++checks;
                if (dd > worst) { worst = dd; worst_i = k; worst_h = h; }
                if (dd > 1.0) { if (fails < 12) std::printf("JUMP entity %zu role %d army %d at %.2f h: %.2f m\n", k, a[k].role, a[k].army, h, dd); ++fails; }
                // lift must not pop either (rider mount/dismount is ramped)
                if (std::fabs(b[k].lift - a[k].lift) > 0.35f) { if (fails < 12) std::printf("LIFT POP entity %zu at %.2f h\n", k, h); ++fails; }
            }
            // 2) mounted riders must share the horse's position
            for (size_t k = 0; k < b.size(); ++k)
                if (b[k].role == Scene::Rider && b[k].valid && b[k].lift > 0.4f && b[k].pair >= 0) {
                    auto const& hp = b[size_t(b[k].pair)];
                    double sep = std::hypot(double(b[k].X - hp.X), double(b[k].Z - hp.Z));
                    ++mount_checks; mount_worst = std::max(mount_worst, sep);
                    if (sep > 0.06) { if (fails < 12) std::printf("RIDER OFF HORSE entity %zu at %.2f h: %.2f m\n", k, h, sep); ++fails; }
                }
            a = b;
        }
        // 3) positions must not depend on the animation clock (no random walks / jumps on redraw)
        scene.entity_poses(13.37f, 0.0f, b); scene.entity_poses(13.37f, 987.6f, c);
        long anim_dep = 0;
        for (size_t k = 0; k < b.size(); ++k)
            if (std::fabs(b[k].X - c[k].X) > 1e-4f || std::fabs(b[k].Z - c[k].Z) > 1e-4f) ++anim_dep;
        std::printf("continuity: %ld entity-steps checked, largest step %.3f m (entity %zu at %.2f h), jumps>1m: %d, entities w/o sprite: %ld\n",
            checks, worst, worst_i, worst_h, fails, invalid);
        std::printf("riders: %ld mounted checks, largest rider/horse separation %.4f m\n", mount_checks, mount_worst);
        std::printf("determinism: %ld/%zu entities depend on animation clock\n", anim_dep, b.size());
        bool ok = fails == 0 && anim_dep == 0 && invalid == 0;
        std::printf("SELFTEST %s\n", ok ? "PASS" : "FAIL");
        return ok ? 0 : 1;
    }
    if (!phases_dir.empty()) {
        static const char* names[] = { "dawn", "morning", "midday", "afternoon", "sunset", "evening", "night", "predawn" };
        for (char const* n : names) {
            float h; phase_hour(n, h);
            rend.render(scene, h, anim, frame.data());
            save(phases_dir + "/phase_" + n + ".png");
            std::printf("wrote phase_%s.png (hour %.2f)\n", n, h);
        }
        return 0;
    }
    if (!seq_dir.empty()) {
        int n = 0;
        for (float h = from; h <= to + 1e-4f; h += step, ++n) {
            rend.render(scene, h, anim, frame.data());
            char name[64]; std::snprintf(name, sizeof name, "/seq_%05.2f.png", h);
            save(seq_dir + name);
        }
        std::printf("wrote %d frames\n", n);
        return 0;
    }
    if (bench > 0) {
        float h = hour < 0 ? 12.0f : hour;
        rend.render(scene, h, anim, frame.data()); // warm-up
        double cpu0 = cpu_seconds();
        auto t0 = Clock::now();
        double worst = 0, sum = 0;
        for (int i = 0; i < bench; ++i) {
            auto a = Clock::now();
            rend.render(scene, h, anim + i / 30.0f, frame.data());
            double ms = std::chrono::duration<double, std::milli>(Clock::now() - a).count();
            sum += ms; worst = std::max(worst, ms);
        }
        double wall = std::chrono::duration<double>(Clock::now() - t0).count();
        double cpu = cpu_seconds() - cpu0;
        std::printf("size %dx%d hour %.1f drawables %zu entities %zu\n", rend.width(), rend.height(), h, rend.drawable_count(), scene.entity_count());
        std::printf("frames %d  avg %.2f ms  worst %.2f ms  max-throughput %.1f fps  cpu-utilisation %.0f%%  RSS %ld MB\n",
            bench, sum / bench, worst, bench / wall, 100.0 * cpu / wall, rss_kb() / 1024);
        return 0;
    }
    if (stream) {
        // single-instance guard
        std::string lockpath = std::string(getenv("XDG_RUNTIME_DIR") ? getenv("XDG_RUNTIME_DIR") : "/tmp") + "/exile-living-battlefield.lock";
        int lockfd = open(lockpath.c_str(), O_CREAT | O_RDWR, 0600);
        if (lockfd < 0 || flock(lockfd, LOCK_EX | LOCK_NB) != 0) {
            std::fprintf(stderr, "another Living Battlefield process is already running (%s)\n", lockpath.c_str());
            return 3;
        }
        std::signal(SIGINT, on_signal); std::signal(SIGTERM, on_signal); std::signal(SIGPIPE, SIG_IGN);
        BattleClock clock(st);
        clock.pause(st.paused);
        auto next_t = Clock::now();
        auto last = next_t;
        std::string cmdbuf;
        while (!g_quit) {
            pollfd pfd { 0, POLLIN, 0 };
            while (poll(&pfd, 1, 0) > 0 && (pfd.revents & POLLIN)) {
                char buf[256]; ssize_t n = read(0, buf, sizeof buf);
                if (n <= 0) { pfd.fd = -1; break; }
                cmdbuf.append(buf, size_t(n));
            }
            size_t nl;
            while ((nl = cmdbuf.find('\n')) != std::string::npos) {
                std::string line = cmdbuf.substr(0, nl); cmdbuf.erase(0, nl + 1);
                char c1[64] = {}, c2[64] = {};
                std::sscanf(line.c_str(), "%63s %63s", c1, c2);
                std::string c = c1, v = c2;
                float h;
                if (c == "pause") clock.pause(true);
                else if (c == "resume") clock.pause(false);
                else if (c == "reset") clock.reset();
                else if (c == "phase" && phase_hour(v, h)) clock.set_hour(h);
                else if (c == "hour") clock.set_hour(float(std::atof(v.c_str())));
                else if (c == "speed") st.speed = std::max(0.01f, float(std::atof(v.c_str())));
                else if (c == "cycle") st.cycle_seconds = std::max(10.0f, float(std::atof(v.c_str())));
                else if (c == "realtime") st.realtime = (v == "on");
                else if (c == "lowpower") st.low_power = (v == "on");
                else if (c == "quit") g_quit = 1;
            }
            auto now = Clock::now();
            float dt = std::chrono::duration<float>(now - last).count(); last = now;
            clock.advance(dt, st.realtime, wall_hour());
            rend.render(scene, clock.hour(), clock.anim_time(), frame.data());
            size_t off = 0, total = frame.size();
            while (off < total) {
                ssize_t w = write(1, frame.data() + off, total - off);
                if (w <= 0) { g_quit = 1; break; }
                off += size_t(w);
            }
            next_t += std::chrono::microseconds(1000000 / st.effective_fps());
            if (next_t < Clock::now()) next_t = Clock::now(); // never accumulate debt
            std::this_thread::sleep_until(next_t);
        }
        std::fprintf(stderr, "clean shutdown\n");
        flock(lockfd, LOCK_UN); close(lockfd);
        return 0;
    }
    float h = hour < 0 ? st.start_hour : hour;
    if (out.empty()) { usage(); return 2; }
    rend.render(scene, h, anim, frame.data());
    save(out);
    std::printf("wrote %s (%dx%d, hour %.2f)\n", out.c_str(), rend.width(), rend.height(), h);
    return 0;
}
