// Settings + battlefield clock.
//
// Storage: a plain INI file (host build). On Exile OS the equivalent would be the
// existing Config:: (LibConfig) domain under ~/.config; that mapping is NOT
// implemented here because the native build is not possible yet (see docs/).
#pragma once
#include <cstdint>
#include <string>

namespace lb {

struct Settings {
    // [Time]
    float cycle_seconds = 1800.0f; // real seconds per simulated 24h day (default: 30 min)
    float speed = 1.0f;            // multiplier on top of cycle_seconds
    float start_hour = 6.0f;       // dawn
    bool realtime = false;         // follow the computer's local clock instead
    bool paused = false;
    // [Render]
    int width = 1920;
    int height = 1080;
    int fps = 30;
    bool low_power = false;        // forces 15 fps
    float render_scale = 0.6f;     // internal resolution scale (measured default; see README)
    int threads = 2;               // row-band worker threads (1 = single thread)
    // [Effects]
    bool grain = true;
    bool bloom = true;
    uint32_t seed = 20261022;

    int effective_fps() const { return low_power ? 15 : fps; }
};

bool load_settings(std::string const& path, Settings& s, std::string& error);
bool save_settings(std::string const& path, Settings const& s, std::string& error);

class BattleClock {
public:
    explicit BattleClock(Settings const& s) : m_s(&s) { reset(); }
    void reset() { m_hour = m_s->start_hour; m_anim = 0; }
    void set_hour(float h) { m_hour = h < 0 ? 0 : h; while (m_hour >= 24.0f) m_hour -= 24.0f; }
    void advance(float real_dt, bool wall_clock_hour_valid, float wall_clock_hour);
    float hour() const { return m_hour; }
    float anim_time() const { return m_anim; } // monotonic seconds, drives idle/wind/fire
    void pause(bool p) { m_paused = p; }
    bool paused() const { return m_paused; }

private:
    Settings const* m_s;
    float m_hour = 6.0f;
    float m_anim = 0.0f;
    bool m_paused = false;
};

// Named phase -> hour (dawn 6, morning 9, midday 12, afternoon 15, sunset 18, evening 21, night 0, predawn 3).
bool phase_hour(std::string const& name, float& hour);
char const* phase_name_for(float hour);

} // namespace lb
