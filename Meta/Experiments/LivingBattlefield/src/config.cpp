#include "config.h"
#include "common.h"
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <map>
#include <sstream>

namespace lb {

static std::string trim(std::string s)
{
    size_t a = s.find_first_not_of(" \t\r\n"), b = s.find_last_not_of(" \t\r\n");
    return a == std::string::npos ? std::string() : s.substr(a, b - a + 1);
}
static bool to_bool(std::string const& v) { return v == "1" || v == "true" || v == "yes" || v == "on"; }

bool load_settings(std::string const& path, Settings& s, std::string& error)
{
    std::ifstream in(path);
    if (!in) { error = "cannot open config " + path; return false; }
    std::string line, section;
    while (std::getline(in, line)) {
        line = trim(line);
        if (line.empty() || line[0] == '#' || line[0] == ';') continue;
        if (line[0] == '[' && line.back() == ']') { section = line.substr(1, line.size() - 2); continue; }
        auto eq = line.find('=');
        if (eq == std::string::npos) continue;
        std::string k = trim(line.substr(0, eq)), v = trim(line.substr(eq + 1));
        if (section == "Time") {
            if (k == "cycle_seconds") s.cycle_seconds = std::max(10.0f, float(std::atof(v.c_str())));
            else if (k == "speed") s.speed = std::max(0.01f, float(std::atof(v.c_str())));
            else if (k == "start_hour") s.start_hour = wrap24(float(std::atof(v.c_str())));
            else if (k == "realtime") s.realtime = to_bool(v);
            else if (k == "paused") s.paused = to_bool(v);
        } else if (section == "Render") {
            if (k == "width") s.width = std::max(160, std::atoi(v.c_str()));
            else if (k == "height") s.height = std::max(90, std::atoi(v.c_str()));
            else if (k == "fps") s.fps = std::clamp(std::atoi(v.c_str()), 1, 120);
            else if (k == "low_power") s.low_power = to_bool(v);
            else if (k == "render_scale") s.render_scale = clampf(float(std::atof(v.c_str())), 0.25f, 2.0f);
            else if (k == "threads") s.threads = std::clamp(std::atoi(v.c_str()), 1, 8);
        } else if (section == "Effects") {
            if (k == "grain") s.grain = to_bool(v);
            else if (k == "bloom") s.bloom = to_bool(v);
            else if (k == "seed") s.seed = uint32_t(std::strtoul(v.c_str(), nullptr, 10));
        }
    }
    return true;
}

bool save_settings(std::string const& path, Settings const& s, std::string& error)
{
    std::ofstream out(path);
    if (!out) { error = "cannot write config " + path; return false; }
    out << "[Time]\ncycle_seconds=" << s.cycle_seconds << "\nspeed=" << s.speed << "\nstart_hour=" << s.start_hour
        << "\nrealtime=" << (s.realtime ? "true" : "false") << "\npaused=" << (s.paused ? "true" : "false") << "\n\n";
    out << "[Render]\nwidth=" << s.width << "\nheight=" << s.height << "\nfps=" << s.fps << "\nlow_power="
        << (s.low_power ? "true" : "false") << "\nrender_scale=" << s.render_scale << "\nthreads=" << s.threads << "\n\n";
    out << "[Effects]\ngrain=" << (s.grain ? "true" : "false") << "\nbloom=" << (s.bloom ? "true" : "false")
        << "\nseed=" << s.seed << "\n";
    return true;
}

void BattleClock::advance(float real_dt, bool wall_valid, float wall_hour)
{
    m_anim += real_dt; // wind/fire/idle keep running even while the day is paused
    if (m_paused) return;
    if (m_s->realtime && wall_valid) { m_hour = wall_hour; return; }
    m_hour = wrap24(m_hour + real_dt * m_s->speed * 24.0f / m_s->cycle_seconds);
}

bool phase_hour(std::string const& n, float& h)
{
    static const std::map<std::string, float> t = { { "dawn", 6 }, { "morning", 9 }, { "midday", 12 }, { "afternoon", 15 },
        { "sunset", 18 }, { "evening", 21 }, { "night", 0 }, { "midnight", 0 }, { "predawn", 3 } };
    auto it = t.find(n);
    if (it == t.end()) return false;
    h = it->second;
    return true;
}

char const* phase_name_for(float h)
{
    h = wrap24(h);
    if (h < 1.5f) return "night";
    if (h < 4.5f) return "predawn";
    if (h < 7.5f) return "dawn";
    if (h < 10.5f) return "morning";
    if (h < 13.5f) return "midday";
    if (h < 16.5f) return "afternoon";
    if (h < 19.5f) return "sunset";
    if (h < 22.5f) return "evening";
    return "night";
}

} // namespace lb
