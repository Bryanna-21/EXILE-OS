// CPU layered 2D renderer (depth-separated image layers + sorted sprites + analytic particles).
//
// This is NOT a 3D renderer. The only "3D" is a pinhole ground-plane projection
// (common.h: kCamF/kCamH/kHorizon) used to place and scale sprites and to bake the
// ground texture; there is no real geometry, depth buffer or per-vertex lighting.
#pragma once
#include "atlas.h"
#include "config.h"
#include "scene.h"
#include <vector>

namespace lb {

class Renderer {
public:
    Renderer(Atlas const& atlas, Settings const& settings) : m_atlas(atlas), m_settings(settings) {}
    bool init(std::string const& assets_dir, std::string& error);
    int width() const { return m_w; }
    int height() const { return m_h; }
    // Renders one frame into RGBA8 (size width()*height()*4).
    void render(Scene const& scene, float hour, float anim, uint8_t* rgba);
    size_t texture_bytes() const;
    size_t drawable_count() const { return m_frame.drawables.size(); }

private:
    struct Tex8 { int w = 0, h = 0; std::vector<uint8_t> px; }; // RGBA straight alpha
    struct ClothParams { float amp; float phase; };

    Atlas const& m_atlas;
    Settings const& m_settings;
    int m_w = 0, m_h = 0;
    float m_k = 1, m_ox = 0, m_oy = 0; // reference->output transform
    float m_gy0 = 0;                   // output row of the horizon
    Tex8 m_clouds, m_hills_far, m_hills_near, m_ground;
    std::vector<float> m_fb;           // RGB float framebuffer
    std::vector<float> m_light;        // 1/4-res light map (RGB)
    int m_lw = 0, m_lh = 0;
    std::vector<int> m_lrow_min, m_lrow_max;
    std::vector<float> m_vig_x, m_vig_y;
    std::vector<uint8_t> m_grain;      // 256x256 noise
    std::vector<float> m_mist;         // 128x32 tileable noise
    std::vector<float> m_tone;         // (unused legacy)
    std::vector<uint8_t> m_lut8;       // tonemap + gamma LUT (4096 entries over 0..4)
    std::vector<int> m_lm_ix0, m_lm_ix1, m_sky_cx0, m_sky_cx1;
    std::vector<float> m_lm_tx, m_sky_ctx, m_sky_gx;
    struct Star { float x, y, mag; };
    std::vector<Star> m_stars;
    Frame m_frame;
    std::vector<int> m_order;

    // per-frame lighting state
    V3 m_L;          // ambient + direct (+moon) light on lit surfaces
    V3 m_fogc;
    float m_mist_amt = 0, m_time_anim = 0;
    float m_frame_shear = 0, m_frame_shadow = 0;

    void prepare_sky(Daylight const& d);
    void render_band(Scene const& scene, Daylight const& d, int y0, int y1, uint8_t* rgba, bool prof);
    void draw_sky(Daylight const& d);
    void draw_hills(Daylight const& d);
    void draw_ground(Daylight const& d);
    void build_lightmap();
    V3 lm_sample(float x, float y) const;
    float fog_at(float Z, float foot_rr) const;
    void draw_drawable(Drawable const& dr, Daylight const& d);
    void blit_sprite(SpriteMeta const& m, float X0, float Y0, float scale, bool flip, float sy, float alpha, V3 L, float fog,
        ClothParams const* cloth);
    void draw_shadow(SpriteMeta const& m, float X0, float Y0, float scale, bool flip, float sy, float shear, float strength);
    void draw_fire(FireInst const& f, Daylight const& d);
    void glow_add(float cx, float cy, float rx, float ry, V3 col, float intensity);
    void disc_alpha(float cx, float cy, float r, V3 col, float alpha);
    void lantern_glow(float cx, float cy, float scale, float intensity, bool star);
    void post(uint8_t* rgba, Daylight const& d);
    static void resample(ImageRGBA8 const& src, int dw, int dh, Tex8& dst);
};

} // namespace lb
