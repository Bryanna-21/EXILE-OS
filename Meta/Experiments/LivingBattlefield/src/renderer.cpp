#include "renderer.h"
#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <chrono>
#include <cstring>
#include <thread>

namespace lb {

// Row-band clip for the current thread (the frame is rendered as independent horizontal bands).
static thread_local int t_y0 = 0, t_y1 = 1 << 30;

namespace {
inline float srgb_noise(uint32_t x) { return hash01(x); }
struct OutP { float x, y, s; };
inline float sq(float x) { return x * x; }
} // namespace

// ------------------------------------------------------------------ setup ----
void Renderer::resample(ImageRGBA8 const& s, int dw, int dh, Tex8& d)
{
    d.w = dw; d.h = dh;
    d.px.assign(size_t(dw) * dh * 4, 0);
    if (dw == s.w && dh == s.h) { d.px = s.px; return; }
    if (dw <= s.w && dh <= s.h) { // area average, premultiplied
        for (int y = 0; y < dh; ++y) {
            int y0 = y * s.h / dh, y1 = std::max(y0 + 1, (y + 1) * s.h / dh);
            for (int x = 0; x < dw; ++x) {
                int x0 = x * s.w / dw, x1 = std::max(x0 + 1, (x + 1) * s.w / dw);
                uint64_t r = 0, g = 0, b = 0, a = 0, n = 0;
                for (int yy = y0; yy < y1; ++yy)
                    for (int xx = x0; xx < x1; ++xx) {
                        uint8_t const* p = &s.px[(size_t(yy) * s.w + xx) * 4];
                        r += p[0] * p[3]; g += p[1] * p[3]; b += p[2] * p[3]; a += p[3]; ++n;
                    }
                uint8_t* o = &d.px[(size_t(y) * dw + x) * 4];
                if (a > 0) { o[0] = uint8_t(r / a); o[1] = uint8_t(g / a); o[2] = uint8_t(b / a); }
                o[3] = uint8_t(a / n);
            }
        }
        return;
    }
    for (int y = 0; y < dh; ++y) { // bilinear
        float fy = (y + 0.5f) * s.h / dh - 0.5f;
        int y0 = clampf(std::floor(fy), 0, float(s.h - 1)), y1 = std::min(s.h - 1, y0 + 1);
        float ty = clampf(fy - y0, 0, 1);
        for (int x = 0; x < dw; ++x) {
            float fx = (x + 0.5f) * s.w / dw - 0.5f;
            int x0 = clampf(std::floor(fx), 0, float(s.w - 1)), x1 = std::min(s.w - 1, x0 + 1);
            float tx = clampf(fx - x0, 0, 1);
            for (int c = 0; c < 4; ++c) {
                float v = (s.px[(size_t(y0) * s.w + x0) * 4 + c] * (1 - tx) + s.px[(size_t(y0) * s.w + x1) * 4 + c] * tx) * (1 - ty)
                    + (s.px[(size_t(y1) * s.w + x0) * 4 + c] * (1 - tx) + s.px[(size_t(y1) * s.w + x1) * 4 + c] * tx) * ty;
                d.px[(size_t(y) * dw + x) * 4 + c] = uint8_t(v + 0.5f);
            }
        }
    }
}

bool Renderer::init(std::string const& dir, std::string& error)
{
    m_w = std::max(160, int(m_settings.width * m_settings.render_scale + 0.5f)) & ~1;
    m_h = std::max(90, int(m_settings.height * m_settings.render_scale + 0.5f)) & ~1;
    m_k = std::max(m_w / kRefW, m_h / kRefH);
    m_ox = (m_w - kRefW * m_k) * 0.5f;
    m_oy = (m_h - kRefH * m_k) * 0.5f;
    m_gy0 = m_oy + kHorizon * m_k;
    ImageRGBA8 img;
    auto load = [&](char const* name, Tex8& t, float ref_h) -> bool {
        if (!load_png(dir + "/" + name, img, error)) return false;
        resample(img, int(std::lround(img.w * m_k)), int(std::lround(ref_h * m_k)), t);
        return true;
    };
    if (!load("sky_clouds.png", m_clouds, 430) || !load("hills_far.png", m_hills_far, 300) || !load("hills_near.png", m_hills_near, 260)
        || !load("ground.png", m_ground, 650))
        return false;
    m_fb.assign(size_t(m_w) * m_h * 3, 0.0f);
    m_grain.resize(256 * 256);
    for (size_t i = 0; i < m_grain.size(); ++i) m_grain[i] = uint8_t(hash32(uint32_t(i) * 2654435761u + m_settings.seed) >> 24);
    { // perspective-correct fine grass/soil grain, baked once (it never changes per frame)
        int gx0 = int(std::lround(m_ox)), gy = int(std::lround(m_gy0));
        for (int ty = 0; ty < m_ground.h; ++ty) {
            float rr = std::max(1.0f, (float(gy + ty) + 0.5f - m_gy0) / m_k);
            float Z = kCamF * kCamH / rr;
            float detail = 0.20f * saturate((70.0f - Z) / 48.0f);
            if (detail <= 0.0f) continue;
            for (int tx = 0; tx < m_ground.w; ++tx) {
                float Xm = (float(gx0 + tx) - m_ox - kRefW * 0.5f * m_k) / m_k * Z / kCamF;
                int iu = int(std::floor(Xm * 22.0f)) & 255, iv = int(std::floor(Z * 22.0f)) & 255;
                float n = m_grain[size_t(iv) * 256 + size_t(iu)] * (1.0f / 255.0f);
                float n2 = m_grain[size_t((iv * 7 + 31) & 255) * 256 + size_t((iu * 3 + 11) & 255)] * (1.0f / 255.0f);
                float k = 1.0f + detail * (n - 0.5f) * 2.0f + detail * 0.8f * (n2 - 0.5f) * (n > 0.78f ? 3.0f : 1.0f);
                uint8_t* p = &m_ground.px[(size_t(ty) * m_ground.w + tx) * 4];
                for (int c = 0; c < 3; ++c) p[c] = uint8_t(clampf(p[c] * k, 0.0f, 255.0f) + 0.5f);
            }
        }
    }
    m_lw = (m_w + 3) / 4; m_lh = (m_h + 3) / 4;
    m_light.assign(size_t(m_lw) * m_lh * 3, 0.0f);
    m_lm_ix0.resize(size_t(m_w)); m_lm_ix1.resize(size_t(m_w)); m_lm_tx.resize(size_t(m_w));
    for (int x = 0; x < m_w; ++x) {
        float fx = (x + 0.5f) * 0.25f - 0.5f; int x0 = int(std::floor(fx));
        m_lm_tx[size_t(x)] = fx - float(x0);
        m_lm_ix0[size_t(x)] = std::clamp(x0, 0, m_lw - 1); m_lm_ix1[size_t(x)] = std::clamp(x0 + 1, 0, m_lw - 1);
    }
    m_lrow_min.assign(size_t(m_lh), 0); m_lrow_max.assign(size_t(m_lh), -1);
    m_vig_x.resize(size_t(m_w)); m_vig_y.resize(size_t(m_h));
    for (int x = 0; x < m_w; ++x) { float u = (x + 0.5f) / m_w * 2 - 1; m_vig_x[size_t(x)] = 1.0f - 0.10f * u * u; }
    for (int y = 0; y < m_h; ++y) { float v = (y + 0.5f) / m_h * 2 - 1; m_vig_y[size_t(y)] = 1.0f - 0.12f * v * v; }
    m_mist.resize(128 * 32);
    { // tileable value noise (2 octaves)
        float base[16 * 8 + 8 * 4];
        for (float& b : base) b = hash01(uint32_t(&b - base) * 7919u + m_settings.seed);
        for (int y = 0; y < 32; ++y)
            for (int x = 0; x < 128; ++x) {
                float v = 0;
                for (int o = 0; o < 2; ++o) {
                    int gw = o == 0 ? 16 : 8, gh = o == 0 ? 8 : 4, off = o == 0 ? 0 : 128;
                    float fx = x * float(gw) / 128.0f, fy = y * float(gh) / 32.0f;
                    int x0 = int(fx) % gw, x1 = (x0 + 1) % gw, y0 = int(fy) % gh, y1 = (y0 + 1) % gh;
                    float tx = smooth01(fx - std::floor(fx)), ty = smooth01(fy - std::floor(fy));
                    float a = base[off + y0 * gw + x0], b = base[off + y0 * gw + x1], c = base[off + y1 * gw + x0], dd = base[off + y1 * gw + x1];
                    v += ((a * (1 - tx) + b * tx) * (1 - ty) + (c * (1 - tx) + dd * tx) * ty) * (o == 0 ? 0.65f : 0.35f);
                }
                m_mist[size_t(y) * 128 + x] = v;
            }
    }
    m_lut8.resize(4096);
    for (int i = 0; i < 4096; ++i) {
        float x = float(i) / 1024.0f;
        float y = 1.0f - std::exp(-1.55f * x);
        float s = y * y * (3.0f - 2.0f * y);
        y = lerpf(y, s, 0.30f);
        m_lut8[size_t(i)] = uint8_t(clampf(std::pow(y, 1.0f / 1.08f), 0.0f, 1.0f) * 255.0f + 0.5f);
    }
    m_tone.resize(2049);
    for (int i = 0; i <= 2048; ++i) {
        float x = float(i) / 512.0f; // 0..4
        float y = 1.0f - std::exp(-1.55f * x);
        float s = y * y * (3.0f - 2.0f * y);
        y = lerpf(y, s, 0.30f);
        m_tone[size_t(i)] = std::pow(y, 1.0f / 1.08f);
    }
    for (int i = 0; i < 260; ++i) {
        float u = hash01(m_settings.seed * 3u + uint32_t(i) * 101u), v = hash01(m_settings.seed * 5u + uint32_t(i) * 131u);
        m_stars.push_back({ u * kRefW, std::pow(v, 0.8f) * 400.0f, 0.25f + 0.75f * hash01(uint32_t(i) * 17u + 3u) });
    }
    m_frame.drawables.reserve(2400);
    return true;
}

size_t Renderer::texture_bytes() const
{
    return m_clouds.px.size() + m_hills_far.px.size() + m_hills_near.px.size() + m_ground.px.size() + m_fb.size() * sizeof(float)
        + m_light.size() * sizeof(float) + m_atlas.bytes();
}

// ---------------------------------------------------------------- lighting ----
float Renderer::fog_at(float Z, float rr) const
{
    float f = 1.0f - std::exp(-Z / (320.0f - 120.0f * m_mist_amt));
    f += m_mist_amt * 0.34f * saturate(1.1f - rr / 380.0f);
    return clampf(f, 0.0f, 0.92f);
}

void Renderer::build_lightmap()
{
    std::fill(m_light.begin(), m_light.end(), 0.0f);
    std::fill(m_lrow_min.begin(), m_lrow_min.end(), m_lw);
    std::fill(m_lrow_max.begin(), m_lrow_max.end(), -1);
    for (LightInst const& L : m_frame.lights) {
        Proj G = project(L.X, L.Z, 0.0f);
        float cx = (m_ox + G.sx * m_k) * 0.25f, cy = (m_oy + G.sy * m_k) * 0.25f;
        float rx = L.radius * G.scale * m_k * 0.25f, ry = rx * 0.42f;
        if (rx < 0.5f) continue;
        int x0 = std::max(0, int(cx - rx)), x1 = std::min(m_lw - 1, int(cx + rx) + 1);
        int y0 = std::max(0, int(cy - ry)), y1 = std::min(m_lh - 1, int(cy + ry) + 1);
        for (int y = y0; y <= y1; ++y) {
            float dy = (y - cy) / ry;
            for (int x = x0; x <= x1; ++x) {
                float dx = (x - cx) / rx;
                float d2 = dx * dx + dy * dy;
                if (d2 >= 1.0f) continue;
                float w = sq(1.0f - d2) * L.intensity;
                float* p = &m_light[(size_t(y) * m_lw + x) * 3];
                p[0] += L.col.r * w; p[1] += L.col.g * w; p[2] += L.col.b * w;
            }
            m_lrow_min[size_t(y)] = std::min(m_lrow_min[size_t(y)], x0);
            m_lrow_max[size_t(y)] = std::max(m_lrow_max[size_t(y)], x1);
        }
    }
}

V3 Renderer::lm_sample(float x, float y) const
{
    float fx = x * 0.25f - 0.5f, fy = y * 0.25f - 0.5f;
    int x0 = int(std::floor(fx)), y0 = int(std::floor(fy));
    float tx = fx - x0, ty = fy - y0;
    auto at = [&](int xx, int yy, int c) {
        xx = std::clamp(xx, 0, m_lw - 1); yy = std::clamp(yy, 0, m_lh - 1);
        return m_light[(size_t(yy) * m_lw + xx) * 3 + c];
    };
    V3 o;
    float* po[3] = { &o.r, &o.g, &o.b };
    for (int c = 0; c < 3; ++c)
        *po[c] = (at(x0, y0, c) * (1 - tx) + at(x0 + 1, y0, c) * tx) * (1 - ty) + (at(x0, y0 + 1, c) * (1 - tx) + at(x0 + 1, y0 + 1, c) * tx) * ty;
    return o;
}

// -------------------------------------------------------------------- sky ----
void Renderer::prepare_sky(Daylight const& d)
{
    int w = m_clouds.w;
    bool glow_on = d.glow > 0.01f;
    float gx = m_ox + d.glow_x * m_k;
    float scroll = m_time_anim * 7.0f * m_k;
    m_sky_cx0.resize(size_t(m_w)); m_sky_cx1.resize(size_t(m_w)); m_sky_ctx.resize(size_t(m_w)); m_sky_gx.resize(size_t(m_w));
    for (int x = 0; x < m_w; ++x) {
        float fx = float(x) - m_ox + scroll;
        float fxm = std::fmod(fx, float(w)); if (fxm < 0) fxm += w;
        int x0 = int(fxm) % w;
        m_sky_cx0[size_t(x)] = x0; m_sky_cx1[size_t(x)] = (x0 + 1) % w; m_sky_ctx[size_t(x)] = fxm - std::floor(fxm);
        m_sky_gx[size_t(x)] = glow_on ? std::exp(-sq((x - gx) / (820.0f * m_k))) : 0.0f;
    }
}

void Renderer::draw_sky(Daylight const& d)
{
    int rows = std::min(m_h, int(m_gy0) + 3);
    float skyh = m_gy0;
    bool glow_on = d.glow > 0.01f;
    float cloud_amt = 0.92f - 0.45f * d.night;
    V3 cloud_lit = d.ambient * 1.25f + d.sun_color * d.sun_strength * 0.55f;
    V3 cloud_night = V3 { 0.07f, 0.09f, 0.14f } * (0.5f + d.moon_strength);
    for (int y = std::max(0, t_y0); y < std::min(rows, t_y1); ++y) {
        float t = saturate(1.0f - (y + 0.5f - m_oy) / (kHorizon * m_k));
        float tt = std::pow(t, 0.62f);
        V3 base = mix(d.horizon, d.zenith, tt);
        float hy = saturate(1.0f - std::fabs(skyh - y) / (230.0f * m_k)); // proximity to horizon
        int cy = y - int(m_oy);
        float* row = &m_fb[size_t(y) * m_w * 3];
        for (int x = 0; x < m_w; ++x) {
            V3 c = base;
            if (glow_on) {
                float g = d.glow * m_sky_gx[size_t(x)] * hy;
                c = c + d.glow_color * g;
            }
            if (cy >= 0 && cy < m_clouds.h) {
                int w = m_clouds.w;
                int x0 = m_sky_cx0[size_t(x)], x1 = m_sky_cx1[size_t(x)]; float tx = m_sky_ctx[size_t(x)];
                float a = (m_clouds.px[(size_t(cy) * w + x0) * 4 + 3] * (1 - tx) + m_clouds.px[(size_t(cy) * w + x1) * 4 + 3] * tx) * (1.0f / 255.0f);
                a *= cloud_amt * (1.0f - 0.55f * hy * hy * 0.0f);
                if (a > 0.004f) {
                    float sh = (m_clouds.px[(size_t(cy) * w + x0) * 4] * (1 - tx) + m_clouds.px[(size_t(cy) * w + x1) * 4] * tx) * (1.0f / 255.0f);
                    V3 cc = mix(cloud_night, cloud_lit, 1.0f - d.night) * sh;
                    if (glow_on) cc = cc + d.glow_color * (d.glow * 0.45f * hy);
                    c = mix(c, cc, a);
                }
            }
            row[x * 3] = c.r; row[x * 3 + 1] = c.g; row[x * 3 + 2] = c.b;
        }
    }
    // stars
    if (d.night > 0.3f) {
        for (size_t i = 0; i < m_stars.size(); ++i) {
            Star const& s = m_stars[i];
            float fade = saturate((kHorizon - 40.0f - s.y) / 220.0f) * (d.night - 0.3f) / 0.7f;
            float tw = 0.75f + 0.25f * std::sin(m_time_anim * (1.5f + s.mag * 2.0f) + float(i));
            float v = s.mag * fade * tw;
            if (v < 0.03f) continue;
            int x = int(m_ox + s.x * m_k), y = int(m_oy + s.y * m_k);
            if (x < 0 || x >= m_w || y < t_y0 || y >= t_y1) continue;
            float* p = &m_fb[(size_t(y) * m_w + x) * 3];
            p[0] += 0.75f * v; p[1] += 0.85f * v; p[2] += 1.0f * v;
            if (s.mag > 0.85f && x + 1 < m_w) { float* q = p + 3; q[0] += 0.3f * v; q[1] += 0.34f * v; q[2] += 0.4f * v; }
        }
    }
    if (d.sun_strength > 0.02f && d.sun_elev > 0.0f) {
        float sx = m_ox + d.sun_x * m_k, sy = m_oy + d.sun_y * m_k;
        glow_add(sx, sy, 300.0f * m_k, 300.0f * m_k, d.sun_color, 0.30f * (0.4f + 0.6f * (1.0f - d.sun_elev * 0.5f)));
        glow_add(sx, sy, 60.0f * m_k, 60.0f * m_k, V3 { 1.0f, 0.95f, 0.85f }, 0.9f);
        glow_add(sx, sy, 24.0f * m_k, 24.0f * m_k, V3 { 1.0f, 0.99f, 0.95f }, 2.0f);
    }
    if (d.moon_strength > 0.02f) {
        float mx = m_ox + d.moon_x * m_k, my = m_oy + d.moon_y * m_k;
        glow_add(mx, my, 170.0f * m_k, 170.0f * m_k, V3 { 0.55f, 0.68f, 0.95f }, 0.22f * d.moon_strength);
        glow_add(mx, my, 22.0f * m_k, 22.0f * m_k, V3 { 0.92f, 0.95f, 1.0f }, 1.6f * d.moon_strength);
    }
}

void Renderer::glow_add(float cx, float cy, float rx, float ry, V3 col, float intensity)
{
    int x0 = std::max(0, int(cx - rx)), x1 = std::min(m_w - 1, int(cx + rx) + 1);
    int y0 = std::max(t_y0, int(cy - ry)), y1 = std::min(t_y1 - 1, int(cy + ry) + 1);
    float irx = 1.0f / rx, iry = 1.0f / ry;
    for (int y = y0; y <= y1; ++y) {
        float dy = (y + 0.5f - cy) * iry;
        float* row = &m_fb[size_t(y) * m_w * 3];
        for (int x = x0; x <= x1; ++x) {
            float dx = (x + 0.5f - cx) * irx;
            float d2 = dx * dx + dy * dy;
            if (d2 >= 1.0f) continue;
            float w = sq(1.0f - d2) * intensity;
            row[x * 3] += col.r * w; row[x * 3 + 1] += col.g * w; row[x * 3 + 2] += col.b * w;
        }
    }
}

void Renderer::disc_alpha(float cx, float cy, float r, V3 col, float alpha)
{
    int x0 = std::max(0, int(cx - r)), x1 = std::min(m_w - 1, int(cx + r) + 1);
    int y0 = std::max(t_y0, int(cy - r)), y1 = std::min(t_y1 - 1, int(cy + r) + 1);
    float ir = 1.0f / r;
    for (int y = y0; y <= y1; ++y) {
        float dy = (y + 0.5f - cy) * ir;
        float* row = &m_fb[size_t(y) * m_w * 3];
        for (int x = x0; x <= x1; ++x) {
            float dx = (x + 0.5f - cx) * ir;
            float d2 = dx * dx + dy * dy;
            if (d2 >= 1.0f) continue;
            float a = alpha * (1.0f - d2) * std::sqrt(1.0f - d2);
            row[x * 3] += (col.r - row[x * 3]) * a; row[x * 3 + 1] += (col.g - row[x * 3 + 1]) * a; row[x * 3 + 2] += (col.b - row[x * 3 + 2]) * a;
        }
    }
}

// ------------------------------------------------------------------ hills ----
void Renderer::draw_hills(Daylight const& d)
{
    struct Layer { Tex8 const* t; float top_ref; float fog; float lit; };
    Layer layers[2] = { { &m_hills_far, 110.0f, 0.62f + 0.22f * m_mist_amt, 0.55f }, { &m_hills_near, 215.0f, 0.34f + 0.26f * m_mist_amt, 0.62f } };
    V3 Lh_base = d.ambient;
    for (Layer const& L : layers) {
        V3 Lh = Lh_base + d.sun_color * (d.sun_strength * L.lit) + V3 { 0.45f, 0.58f, 0.85f } * (0.12f * d.moon_strength);
        int ty0 = int(std::lround(m_oy + L.top_ref * m_k));
        int tx0 = int(std::lround(m_ox));
        for (int y = 0; y < L.t->h; ++y) {
            int yy = ty0 + y;
            if (yy < t_y0 || yy >= t_y1) continue;
            float* row = &m_fb[size_t(yy) * m_w * 3];
            uint8_t const* src = &L.t->px[size_t(y) * L.t->w * 4];
            for (int x = 0; x < L.t->w; ++x) {
                int xx = tx0 + x;
                if (xx < 0 || xx >= m_w) continue;
                float a = src[x * 4 + 3] * (1.0f / 255.0f);
                if (a <= 0.003f) continue;
                V3 c = V3 { src[x * 4] * (1.0f / 255.0f), src[x * 4 + 1] * (1.0f / 255.0f), src[x * 4 + 2] * (1.0f / 255.0f) } * Lh;
                c = mix(c, m_fogc, L.fog);
                float* p = row + xx * 3;
                p[0] += (c.r - p[0]) * a; p[1] += (c.g - p[1]) * a; p[2] += (c.b - p[2]) * a;
            }
        }
    }
}

// ----------------------------------------------------------------- ground ----
void Renderer::draw_ground(Daylight const& d)
{
    int gy = int(std::lround(m_gy0));
    int gx0 = int(std::lround(m_ox));
    float scroll = m_time_anim * 0.9f;
    for (int y = std::max({ 0, gy, t_y0 }); y < std::min(m_h, t_y1); ++y) {
        int ty = std::min(m_ground.h - 1, y - gy);
        float rr = (y + 0.5f - m_gy0) / m_k;
        if (rr < 1.0f) rr = 1.0f;
        float Z = kCamF * kCamH / rr;
        float fog = fog_at(Z, rr);
        float mist_row = m_mist_amt * 0.30f * saturate(1.15f - rr / 360.0f);
        float* row = &m_fb[size_t(y) * m_w * 3];
        uint8_t const* src = &m_ground.px[size_t(ty) * m_ground.w * 4];
        int ly = std::min(m_lh - 1, y >> 2);
        int lmin = m_lrow_min[size_t(ly)] * 4 - 6, lmax = m_lrow_max[size_t(ly)] * 4 + 10;
        bool has_l = m_lrow_max[size_t(ly)] >= m_lrow_min[size_t(ly)];
        float lfy = (y + 0.5f) * 0.25f - 0.5f; int ly0 = int(std::floor(lfy));
        float lty = lfy - float(ly0), ity = 1.0f - lty;
        float const* L0 = &m_light[size_t(std::clamp(ly0, 0, m_lh - 1)) * m_lw * 3];
        float const* L1 = &m_light[size_t(std::clamp(ly0 + 1, 0, m_lh - 1)) * m_lw * 3];
        int ny = int(rr / 14.0f) & 31;
        for (int x = 0; x < m_w; ++x) {
            int tx = std::clamp(x - gx0, 0, m_ground.w - 1);
            V3 a { src[tx * 4] * (1.0f / 255.0f), src[tx * 4 + 1] * (1.0f / 255.0f), src[tx * 4 + 2] * (1.0f / 255.0f) };
            V3 c = a * m_L;
            if (has_l && x >= lmin && x <= lmax) {
                int i0 = m_lm_ix0[size_t(x)] * 3, i1 = m_lm_ix1[size_t(x)] * 3;
                float tx = m_lm_tx[size_t(x)], itx = 1.0f - tx;
                float lr = (L0[i0] * itx + L0[i1] * tx) * ity + (L1[i0] * itx + L1[i1] * tx) * lty;
                float lg = (L0[i0 + 1] * itx + L0[i1 + 1] * tx) * ity + (L1[i0 + 1] * itx + L1[i1 + 1] * tx) * lty;
                float lb_ = (L0[i0 + 2] * itx + L0[i1 + 2] * tx) * ity + (L1[i0 + 2] * itx + L1[i1 + 2] * tx) * lty;
                c = c + V3 { a.r * lr, a.g * lg, a.b * lb_ } * 0.95f;
            }
            float f = fog;
            if (mist_row > 0.002f) {
                float nx = std::fmod((float(x) / (46.0f * m_k)) + scroll * 0.05f, 128.0f);
                f += mist_row * (0.45f + 1.1f * m_mist[size_t(ny) * 128 + (int(nx) & 127)]);
            }
            f = std::min(f, 0.93f);
            row[x * 3] = c.r + (m_fogc.r - c.r) * f; row[x * 3 + 1] = c.g + (m_fogc.g - c.g) * f; row[x * 3 + 2] = c.b + (m_fogc.b - c.b) * f;
        }
    }
}

// ---------------------------------------------------------------- sprites ----
void Renderer::blit_sprite(SpriteMeta const& m, float X0, float Y0, float scale, bool flip, float sy, float alpha, V3 L, float fog,
    ClothParams const* cloth)
{
    float syS = scale * sy;
    float s = scale;
    int level = 0;
    while (level < Atlas::kMips - 1 && s < 0.62f) { s *= 2.0f; ++level; }
    Atlas::Mip const& mp = m_atlas.mip(level);
    float inv = 1.0f / float(1 << level);
    int rx0 = int(std::floor(m.x * inv)), ry0 = int(std::floor(m.y * inv));
    int rx1 = std::min(mp.w, int(std::ceil((m.x + m.w) * inv))), ry1 = std::min(mp.h, int(std::ceil((m.y + m.h) * inv)));
    float bl, br;
    if (!flip) { bl = X0 + (m.bx0 - m.ax) * scale; br = X0 + (m.bx1 + 1 - m.ax) * scale; }
    else { bl = X0 - (m.bx1 + 1 - m.ax) * scale; br = X0 - (m.bx0 - m.ax) * scale; }
    float bt = Y0 + (m.by0 - m.ay) * syS, bb = Y0 + (m.by1 + 1 - m.ay) * syS;
    float margin = cloth ? cloth->amp * syS * 1.4f + 2.0f : 1.0f;
    int xa = std::max(0, int(std::floor(bl)) - 1), xb = std::min(m_w, int(std::ceil(br)) + 1);
    int ya = std::max(t_y0, int(std::floor(bt - margin))), yb = std::min(std::min(m_h, t_y1), int(std::ceil(bb + margin)));
    if (xa >= xb || ya >= yb) return;
    float iS = 1.0f / scale, iSy = 1.0f / syS;
    V3 Lf = L * (1.0f - fog);
    V3 Ff = m_fogc * fog;
    float du = flip ? -iS : iS;
    float dfx = du * inv;
    float const k255 = 1.0f / (255.0f * 65536.0f);
    for (int y = ya; y < yb; ++y) {
        float v0 = (y + 0.5f - Y0) * iSy + m.ay;
        float u0 = (xa + 0.5f - X0) * iS;
        u0 = flip ? m.ax - u0 : m.ax + u0;
        float* row = &m_fb[size_t(y) * m_w * 3];
        // fast path: rows are linear in x unless the cloth wave perturbs v
        float fy0 = (m.y + v0) * inv - 0.5f;
        float fx = (m.x + u0) * inv - 0.5f;
        if (!cloth && (v0 < 0 || v0 >= m.h)) continue;
        int iy = int(std::floor(fy0));
        int tyi = int((fy0 - float(iy)) * 256.0f);
        int iy0 = std::clamp(iy, ry0, ry1 - 1), iy1 = std::clamp(iy + 1, ry0, ry1 - 1);
        uint8_t const* r0 = &mp.px[size_t(iy0) * mp.w * 4];
        uint8_t const* r1 = &mp.px[size_t(iy1) * mp.w * 4];
        for (int x = xa; x < xb; ++x, fx += dfx) {
            float shade = 1.0f;
            uint8_t const* q0 = r0;
            uint8_t const* q1 = r1;
            int tyx = tyi;
            if (cloth) {
                float u = m.ax + (flip ? -(x + 0.5f - X0) * iS : (x + 0.5f - X0) * iS);
                float cf = saturate(u / float(m.w));
                float ph = cloth->phase + cf * 5.5f;
                float v = v0 - cloth->amp * cf * std::sin(ph);
                shade = 1.0f + 0.24f * cf * std::cos(ph);
                if (u < 0 || u >= m.w || v < 0 || v >= m.h) continue;
                float fyc = (m.y + v) * inv - 0.5f;
                int iyc = int(std::floor(fyc));
                tyx = int((fyc - float(iyc)) * 256.0f);
                q0 = &mp.px[size_t(std::clamp(iyc, ry0, ry1 - 1)) * mp.w * 4];
                q1 = &mp.px[size_t(std::clamp(iyc + 1, ry0, ry1 - 1)) * mp.w * 4];
            } else if (fx < float(rx0) - 1.0f || fx >= float(rx1)) continue;
            int ix = int(std::floor(fx));
            int txi = int((fx - float(ix)) * 256.0f);
            int ix0 = std::clamp(ix, rx0, rx1 - 1), ix1 = std::clamp(ix + 1, rx0, rx1 - 1);
            uint8_t const* p00 = q0 + ix0 * 4; uint8_t const* p10 = q0 + ix1 * 4;
            uint8_t const* p01 = q1 + ix0 * 4; uint8_t const* p11 = q1 + ix1 * 4;
            if ((p00[3] | p10[3] | p01[3] | p11[3]) == 0) continue;
            int w00 = (256 - txi) * (256 - tyx), w10 = txi * (256 - tyx), w01 = (256 - txi) * tyx, w11 = txi * tyx;
            int Ai = p00[3] * w00 + p10[3] * w10 + p01[3] * w01 + p11[3] * w11;
            if (Ai < 3000 * 16) continue;
            float a = float(Ai) * k255 * alpha;
            float ks = k255 * alpha * shade;
            float R = float(p00[0] * w00 + p10[0] * w10 + p01[0] * w01 + p11[0] * w11) * ks;
            float G = float(p00[1] * w00 + p10[1] * w10 + p01[1] * w01 + p11[1] * w11) * ks;
            float B = float(p00[2] * w00 + p10[2] * w10 + p01[2] * w01 + p11[2] * w11) * ks;
            float* o = row + x * 3;
            o[0] = o[0] * (1 - a) + R * Lf.r + a * Ff.r;
            o[1] = o[1] * (1 - a) + G * Lf.g + a * Ff.g;
            o[2] = o[2] * (1 - a) + B * Lf.b + a * Ff.b;
        }
    }
}

void Renderer::draw_shadow(SpriteMeta const& m, float X0, float Y0, float scale, bool flip, float sy, float shear, float strength)
{
    if (strength < 0.02f) return;
    float s = scale;
    int level = 0;
    while (level < Atlas::kMips - 1 && s < 0.62f) { s *= 2.0f; ++level; }
    Atlas::Mip const& mp = m_atlas.mip(level);
    float inv = 1.0f / float(1 << level);
    float syS = scale * sy;
    float Hmax = std::max(1.0f, (m.ay - m.by0)) * syS;
    const float dyk = 0.10f;
    float wide = std::max(std::fabs(m.bx0 - m.ax), std::fabs(m.bx1 + 1 - m.ax)) * scale;
    wide = std::max(wide, 0.15f * m.w * scale);
    int ya = std::max(t_y0, int(std::floor(Y0 - 2))), yb = std::min(std::min(m_h, t_y1), int(std::ceil(Y0 + Hmax * dyk + 2)));
    int xa = std::max(0, int(X0 - wide + std::min(0.0f, shear * Hmax) - 2)), xb = std::min(m_w, int(X0 + wide + std::max(0.0f, shear * Hmax) + 2));
    for (int y = ya; y < yb; ++y) {
        float hgt = std::max(0.0f, (y + 0.5f - Y0) / dyk);
        float v = m.ay - hgt / syS;
        float* row = &m_fb[size_t(y) * m_w * 3];
        // contact ellipse (keeps grounded objects anchored at noon)
        float crx = std::max(2.0f, 0.15f * m.w * scale), cry = crx * 0.30f;
        for (int x = xa; x < xb; ++x) {
            float cover = 0.0f;
            float dxe = (x + 0.5f - X0) / crx, dye = (y + 0.5f - Y0 - cry * 0.4f) / cry;
            float de = dxe * dxe + dye * dye;
            if (de < 1.0f) cover = 0.85f * (1.0f - de);
            if (v >= 0 && v < m.h) {
                float uo = (x + 0.5f - X0 - hgt * shear) / scale;
                float u = flip ? m.ax - uo : m.ax + uo;
                if (u >= 0 && u < m.w) {
                    int tx = std::clamp(int((m.x + u) * inv), 0, mp.w - 1), ty = std::clamp(int((m.y + v) * inv), 0, mp.h - 1);
                    cover = std::max(cover, mp.px[(size_t(ty) * mp.w + tx) * 4 + 3] * (1.0f / 255.0f));
                }
            }
            if (cover <= 0.01f) continue;
            float a = cover * strength;
            float* o = row + x * 3;
            o[0] *= 1.0f - a * 0.80f; o[1] *= 1.0f - a * 0.86f; o[2] *= 1.0f - a * 0.97f;
        }
    }
}

void Renderer::lantern_glow(float cx, float cy, float scale, float intensity, bool star)
{
    // Lanterns: small, crisp, white-gold core with a tight halo (clearly unlike a large orange campfire).
    float r = std::max(2.0f, scale * 0.10f);
    glow_add(cx, cy, scale * 1.15f + 6.0f, scale * 1.15f + 6.0f, V3 { 1.0f, 0.82f, 0.46f }, 0.26f * intensity);
    glow_add(cx, cy, r * 3.2f, r * 3.2f, V3 { 1.0f, 0.93f, 0.70f }, 0.95f * intensity);
    glow_add(cx, cy, r * 1.15f, r * 1.15f, V3 { 1.0f, 1.0f, 0.92f }, 1.9f * intensity);
    if (star) {
        float len = r * 4.5f;
        for (int i = -int(len); i <= int(len); ++i) {
            float w = (1.0f - std::fabs(float(i)) / len) * 0.5f * intensity;
            int x = int(cx) + i, y = int(cy);
            if (x >= 0 && x < m_w && y >= t_y0 && y < t_y1) { float* p = &m_fb[(size_t(y) * m_w + x) * 3]; p[0] += w; p[1] += w * 0.95f; p[2] += w * 0.7f; }
            x = int(cx); y = int(cy) + i;
            if (x >= 0 && x < m_w && y >= t_y0 && y < t_y1) { float* p = &m_fb[(size_t(y) * m_w + x) * 3]; p[0] += w; p[1] += w * 0.95f; p[2] += w * 0.7f; }
        }
    }
}

// ------------------------------------------------------------------- fire ----
void Renderer::draw_fire(FireInst const& f, Daylight const& d)
{
    Proj G = project(f.X, f.Z, 0.0f);
    float px = m_ox + G.sx * m_k, py = m_oy + G.sy * m_k, s = G.scale * m_k; // s = px per metre
    if (px < -300 || px > m_w + 300 || py < -300 || py > m_h + 300) return;
    float t = m_time_anim;
    uint32_t id = f.id;
    float lvl = f.level;
    float wind = wind_at(t);
    // smoke (rises and disperses, drawn first so flames sit in front)
    V3 smoke_light = m_L * 0.78f + V3 { 0.05f, 0.05f, 0.05f };
    constexpr int kSmoke = 13;
    constexpr float kLife = 7.0f;
    for (int p = 0; p < kSmoke; ++p) {
        float ph = hash01(id * 131u + uint32_t(p) * 17u);
        float age = std::fmod(t * 0.85f + (float(p) + ph * 0.6f) * kLife / float(kSmoke), kLife);
        float u = age / kLife;
        float x = f.X + wind * age * 0.55f + 0.34f * std::sin(age * 1.25f + float(p) * 2.3f) * u;
        float h = 0.8f + u * 6.5f;
        float r = (0.30f + u * 1.15f) * s;
        float a = lvl * 0.40f * std::pow(1.0f - u, 1.5f) * ramp(u, 0.0f, 0.07f);
        if (a < 0.01f || r < 1.0f) continue;
        Proj P = project(x, f.Z, h);
        V3 col = V3 { 0.70f, 0.70f, 0.72f } * smoke_light;
        col = col + V3 { 1.0f, 0.55f, 0.22f } * (0.55f * lvl * (1.0f - ramp(u, 0.0f, 0.28f)));
        disc_alpha(m_ox + P.sx * m_k, m_oy + P.sy * m_k, r, col, a);
    }
    // glow bloom on the air + ground
    float flick = 0.86f + 0.14f * std::sin(t * 9.3f + float(id) * 1.7f) + 0.07f * std::sin(t * 23.1f + float(id) * 2.9f);
    glow_add(px, py - 0.5f * s, 2.4f * s, 1.7f * s, V3 { 1.0f, 0.46f, 0.14f }, 0.20f * lvl * flick);
    // flame tongues (additive)
    constexpr int kFl = 8;
    for (int i = 0; i < kFl; ++i) {
        float hf = (float(i) + 0.5f) / kFl;
        float ph = float(id) * 3.1f + float(i) * 1.9f;
        float sway = std::sin(t * (6.0f + float(i) * 0.7f) + ph) * 0.14f * (0.3f + hf) + wind * 0.10f * hf;
        float hh = (0.08f + hf * 0.95f * (0.7f + 0.3f * std::sin(t * 7.7f + ph))) * lvl;
        float rm = 0.30f * (1.0f - 0.6f * hf) * (0.55f + 0.45f * lvl);
        V3 col = mix(V3 { 1.0f, 0.82f, 0.38f }, V3 { 1.0f, 0.30f, 0.05f }, hf);
        Proj P = project(f.X + sway, f.Z, 0.22f + hh);
        float cx = m_ox + P.sx * m_k, cy = m_oy + P.sy * m_k;
        glow_add(cx, cy, rm * s * 1.0f, rm * s * 1.7f, col, 0.95f * lvl * (1.0f - 0.35f * hf));
    }
    glow_add(px, py - 0.25f * s, 0.34f * s, 0.2f * s, V3 { 1.0f, 0.9f, 0.55f }, 1.2f * lvl); // hot bed
    // sparks
    if (lvl > 0.3f) {
        for (int i = 0; i < 9; ++i) {
            uint32_t hs = id * 977u + uint32_t(i) * 53u;
            float period = 1.3f + 1.0f * hash01(hs);
            float age = std::fmod(t + hash01(hs + 1) * period, period) / period;
            if (hash01(hs + 2 + uint32_t(std::floor((t + hash01(hs + 1) * period) / period))) < 0.45f) continue;
            float x = f.X + (hash01(hs + 3) - 0.5f) * 0.5f + wind * age * 1.4f + std::sin(age * 9.0f + float(i)) * 0.14f * age;
            float h = 0.5f + age * 3.6f;
            float b = (1.0f - age) * (1.0f - age) * lvl;
            Proj P = project(x, f.Z, h);
            float cx = m_ox + P.sx * m_k, cy = m_oy + P.sy * m_k;
            glow_add(cx, cy, std::max(1.8f, 0.035f * s), std::max(1.8f, 0.035f * s), V3 { 1.0f, 0.62f, 0.2f }, 1.5f * b);
        }
    }
}

// ---------------------------------------------------------------- drawing ----
void Renderer::draw_drawable(Drawable const& dr, Daylight const& d)
{
    if (dr.Z < 3.0f) return;
    Proj P = project(dr.X, dr.Z, 0.0f);
    float s_px = P.scale * m_k; // output px per metre at this depth
    float footx = m_ox + P.sx * m_k, footy = m_oy + P.sy * m_k;
    switch (dr.kind) {
    case DKind::Fire: {
        draw_fire(m_frame.fires[size_t(dr.fire)], d);
        return;
    }
    case DKind::Glow: {
        Proj Q = project(dr.X, dr.Z, dr.lift);
        float cx = m_ox + Q.sx * m_k, cy = m_oy + Q.sy * m_k;
        float rr = dr.radius * s_px;
        glow_add(cx, cy, rr * 1.8f + 3.0f, rr * 1.8f + 3.0f, dr.col, 0.30f * dr.intensity);
        glow_add(cx, cy, rr * 0.55f + 1.5f, rr * 0.55f + 1.5f, dr.col * 1.1f + V3 { 0.1f, 0.1f, 0.1f }, 1.1f * dr.intensity);
        if (dr.star) lantern_glow(cx, cy, std::max(4.0f, rr * 1.1f), 0.9f * dr.intensity, true);
        return;
    }
    default: break;
    }
    if (dr.sprite < 0) return;
    SpriteMeta const& m = m_atlas.meta(dr.sprite);
    float scale = s_px / dr.mpp;
    if (scale * m.h * dr.sy < 1.0f) return;
    float lift_px = dr.lift * s_px;
    float Y0 = footy - lift_px;
    float X0 = footx;
    // cull
    float wpx = m.w * scale, hpx = m.h * scale;
    if (X0 + wpx < 0 || X0 - wpx > m_w || Y0 + 4 < 0 || Y0 - hpx > m_h) return;
    float rr_ref = (footy - m_gy0) / m_k;
    float fog = fog_at(dr.Z, rr_ref);
    V3 L = m_L;
    V3 lm = lm_sample(footx, footy - 0.45f * hpx * dr.sy * 0.6f - lift_px);
    L = L + lm * 0.95f;
    if (dr.kind == DKind::Cloth) {
        ClothParams cp { dr.wave * 6.5f, m_time_anim * 3.4f + dr.X * 0.9f };
        blit_sprite(m, X0, Y0, scale, dr.flip, 1.0f, dr.alpha, L, fog, &cp);
        return;
    }
    if (dr.shadow && dr.lift < 0.5f) draw_shadow(m, footx, footy, scale, dr.flip, dr.sy, m_frame_shear, m_frame_shadow * (1.0f - 0.7f * fog) * dr.alpha);
    blit_sprite(m, X0, Y0, scale, dr.flip, dr.sy, dr.alpha, L, fog, nullptr);
    if (dr.lantern_lift >= 0.0f && dr.intensity > 0.02f) {
        Proj Q = project(dr.X + dr.lantern_dx, dr.Z - 0.05f, dr.lantern_lift);
        lantern_glow(m_ox + Q.sx * m_k, m_oy + Q.sy * m_k, s_px * 0.85f, dr.intensity, true);
    }
}

// ------------------------------------------------------------------- post ----
void Renderer::post(uint8_t* rgba, Daylight const& d)
{
    float expo = 1.0f + 0.60f * d.night;
    uint32_t fr = uint32_t(m_time_anim * 30.0f);
    int gox = int(hash32(fr * 7u + 1u) & 255), goy = int(hash32(fr * 13u + 5u) & 255);
    float gs = m_settings.grain ? (0.020f + 0.024f * d.night) / 127.5f : 0.0f;
    bool grade = d.night > 0.04f;
    float dk = d.night * 0.25f;
    uint8_t const* lut = m_lut8.data();
    for (int y = std::max(0, t_y0); y < std::min(m_h, t_y1); ++y) {
        float vy = m_vig_y[size_t(y)] * expo;
        float const* row = &m_fb[size_t(y) * m_w * 3];
        uint8_t* o = rgba + size_t(y) * m_w * 4;
        uint8_t const* g = &m_grain[size_t((y + goy) & 255) * 256];
        for (int x = 0; x < m_w; ++x) {
            float v = vy * m_vig_x[size_t(x)];
            float r = row[x * 3] * v, gg = row[x * 3 + 1] * v, b = row[x * 3 + 2] * v;
            if (grade) { // cool, slightly desaturated shadows at night; fire/lantern warmth stays intact
                float lum = r * 0.3f + gg * 0.59f + b * 0.11f;
                float dark = saturate(1.0f - lum * 2.2f) * dk;
                r = r * (1 - dark) + lum * dark * 0.85f;
                gg = gg * (1 - dark) + lum * dark;
                b = b * (1 - dark) + lum * dark * 1.18f;
            }
            float gn = 1.0f + (float(g[(x + gox) & 255]) - 127.5f) * gs;
            int ir = std::clamp(int(r * gn * 1024.0f), 0, 4095);
            int ig = std::clamp(int(gg * gn * 1024.0f), 0, 4095);
            int ib = std::clamp(int(b * gn * 1024.0f), 0, 4095);
            o[x * 4] = lut[ir]; o[x * 4 + 1] = lut[ig]; o[x * 4 + 2] = lut[ib]; o[x * 4 + 3] = 255;
        }
    }
}

// ------------------------------------------------------------------ render ----
void Renderer::render_band(Scene const&, Daylight const& d, int y0, int y1, uint8_t* rgba, bool prof)
{
    t_y0 = y0; t_y1 = y1;
    auto tp = std::chrono::steady_clock::now();
    auto lap = [&](char const* n) {
        if (!prof) return;
        auto now = std::chrono::steady_clock::now();
        std::fprintf(stderr, "  %-10s %7.2f ms\n", n, std::chrono::duration<double, std::milli>(now - tp).count());
        tp = now;
    };
    draw_sky(d); lap("sky");
    draw_hills(d); lap("hills");
    draw_ground(d); lap("ground");
    for (int i : m_order) draw_drawable(m_frame.drawables[size_t(i)], d);
    lap("objects");
    post(rgba, d);
    lap("post");
    t_y0 = 0; t_y1 = 1 << 30;
}

void Renderer::render(Scene const& scene, float hour, float anim, uint8_t* rgba)
{
    m_time_anim = anim;
    Daylight d = compute_daylight(hour);
    scene.evaluate(hour, anim, d, m_frame);
    m_mist_amt = d.mist;
    m_L = d.ambient + d.sun_color * (d.sun_strength * 0.78f) + V3 { 0.42f, 0.56f, 0.86f } * (0.16f * d.moon_strength);
    m_fogc = d.horizon * 0.92f + d.ambient * 0.12f;
    m_frame_shear = d.shadow_shear;
    m_frame_shadow = d.shadow_alpha;
    bool prof = std::getenv("LB_PROFILE") != nullptr;
    if (std::getenv("LB_DEBUG")) {
        int nl = 0, ns = 0;
        for (auto const& dr : m_frame.drawables) { if (dr.lantern_lift >= 0) ++nl; if (dr.kind == DKind::Sprite) ++ns; }
        std::fprintf(stderr, "debug: hour %.2f drawables %zu sprites %d lanterns %d lights %zu lantern_level %.2f\n", hour, m_frame.drawables.size(), ns, nl, m_frame.lights.size(), d.lantern);
    }
    build_lightmap();
    m_order.resize(m_frame.drawables.size());
    for (size_t i = 0; i < m_order.size(); ++i) m_order[i] = int(i);
    std::sort(m_order.begin(), m_order.end(), [&](int a, int b) {
        float za = m_frame.drawables[size_t(a)].Z, zb = m_frame.drawables[size_t(b)].Z;
        return za != zb ? za > zb : a < b;
    });
    prepare_sky(d);
    int n = std::max(1, std::min(m_settings.threads, 8));
    if (n == 1) { render_band(scene, d, 0, m_h, rgba, prof); return; }
    // Cost-balanced row bands (the camps and sprites make the lower half heavier).
    std::vector<double> cum(size_t(m_h) + 1, 0.0);
    for (int y = 0; y < m_h; ++y) cum[size_t(y) + 1] = cum[size_t(y)] + 1.0 + ((y > m_h * 0.45 && y < m_h * 0.95) ? 2.2 : 0.0);
    std::vector<int> cut(size_t(n) + 1, 0);
    cut[size_t(n)] = m_h;
    for (int i = 1; i < n; ++i) {
        double target = cum.back() * i / n;
        int y = int(std::lower_bound(cum.begin(), cum.end(), target) - cum.begin());
        cut[size_t(i)] = std::clamp(y, cut[size_t(i) - 1], m_h);
    }
    std::vector<std::thread> th;
    for (int i = 1; i < n; ++i) th.emplace_back([&, i] { render_band(scene, d, cut[size_t(i)], cut[size_t(i) + 1], rgba, false); });
    render_band(scene, d, cut[0], cut[1], rgba, false);
    for (auto& t : th) t.join();
}

} // namespace lb
