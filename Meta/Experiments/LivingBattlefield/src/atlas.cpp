#include "atlas.h"
#include "json_mini.h"
#include <fstream>
#include <sstream>

namespace lb {

bool Atlas::load(std::string const& dir, std::string& error)
{
    std::ifstream in(dir + "/sprites.json");
    if (!in) { error = "cannot open " + dir + "/sprites.json"; return false; }
    std::stringstream ss;
    ss << in.rdbuf();
    std::string text = ss.str();
    JV root;
    JsonParser p(text);
    if (!p.parse(root)) { error = "sprites.json: parse error"; return false; }
    JV const* sprites = root.get("sprites");
    if (!sprites) { error = "sprites.json: missing 'sprites'"; return false; }
    for (auto const& [name, v] : sprites->obj) {
        SpriteMeta m;
        m.x = int(v.number("x")); m.y = int(v.number("y")); m.w = int(v.number("w")); m.h = int(v.number("h"));
        m.ax = float(v.number("ax")); m.ay = float(v.number("ay"));
        if (v.get("hx")) { m.has_hand = true; m.hx = float(v.number("hx")); m.hy = float(v.number("hy")); }
        m_index[name] = int(m_meta.size());
        m_meta.push_back(m);
    }
    ImageRGBA8 img;
    if (!load_png(dir + "/sprites.png", img, error)) return false;
    for (SpriteMeta& m : m_meta) { // opaque bounds so blits skip transparent cell margins
        int x0 = m.w, y0 = m.h, x1 = -1, y1 = -1;
        for (int y = 0; y < m.h; ++y)
            for (int x = 0; x < m.w; ++x)
                if (img.px[(size_t(m.y + y) * img.w + (m.x + x)) * 4 + 3] > 4) {
                    x0 = std::min(x0, x); x1 = std::max(x1, x); y0 = std::min(y0, y); y1 = std::max(y1, y);
                }
        if (x1 < 0) { x0 = y0 = 0; x1 = y1 = 0; }
        m.bx0 = std::max(0, x0 - 1); m.by0 = std::max(0, y0 - 1); m.bx1 = std::min(m.w - 1, x1 + 1); m.by1 = std::min(m.h - 1, y1 + 1);
    }
    Mip m0;
    m0.w = img.w; m0.h = img.h; m0.px = img.px;
    for (size_t i = 0; i < m0.px.size(); i += 4) { // premultiply
        uint32_t a = m0.px[i + 3];
        m0.px[i + 0] = uint8_t((m0.px[i + 0] * a + 127) / 255);
        m0.px[i + 1] = uint8_t((m0.px[i + 1] * a + 127) / 255);
        m0.px[i + 2] = uint8_t((m0.px[i + 2] * a + 127) / 255);
    }
    m_mips.push_back(std::move(m0));
    for (int l = 1; l < kMips; ++l) {
        Mip const& s = m_mips.back();
        Mip d;
        d.w = std::max(1, s.w / 2); d.h = std::max(1, s.h / 2);
        d.px.assign(size_t(d.w) * d.h * 4, 0);
        for (int y = 0; y < d.h; ++y)
            for (int x = 0; x < d.w; ++x)
                for (int c = 0; c < 4; ++c) {
                    int sx = std::min(s.w - 1, x * 2), sy = std::min(s.h - 1, y * 2);
                    int sx1 = std::min(s.w - 1, sx + 1), sy1 = std::min(s.h - 1, sy + 1);
                    int sum = s.px[(size_t(sy) * s.w + sx) * 4 + c] + s.px[(size_t(sy) * s.w + sx1) * 4 + c]
                        + s.px[(size_t(sy1) * s.w + sx) * 4 + c] + s.px[(size_t(sy1) * s.w + sx1) * 4 + c];
                    d.px[(size_t(y) * d.w + x) * 4 + c] = uint8_t((sum + 2) / 4);
                }
        m_mips.push_back(std::move(d));
    }
    return true;
}

} // namespace lb
