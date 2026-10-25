// Sprite atlas with premultiplied-alpha mip chain.
#pragma once
#include "png_io.h"
#include <string>
#include <unordered_map>
#include <vector>

namespace lb {

struct SpriteMeta {
    int x = 0, y = 0, w = 0, h = 0;
    float ax = 0, ay = 0;     // ground anchor inside the sprite
    float hx = 0, hy = 0;     // lantern/hand point (if has_hand)
    bool has_hand = false;
    int bx0 = 0, by0 = 0, bx1 = 0, by1 = 0; // tight opaque bounds inside the sprite (sprite px, inclusive)
};

class Atlas {
public:
    static constexpr int kMips = 4;
    struct Mip {
        int w = 0, h = 0;
        std::vector<uint8_t> px; // premultiplied RGBA8
    };
    bool load(std::string const& assets_dir, std::string& error);
    int find(std::string const& name) const
    {
        auto it = m_index.find(name);
        return it == m_index.end() ? -1 : it->second;
    }
    SpriteMeta const& meta(int id) const { return m_meta[size_t(id)]; }
    Mip const& mip(int level) const { return m_mips[size_t(level)]; }
    size_t sprite_count() const { return m_meta.size(); }
    size_t bytes() const
    {
        size_t n = 0;
        for (auto& m : m_mips) n += m.px.size();
        return n;
    }

private:
    std::vector<SpriteMeta> m_meta;
    std::unordered_map<std::string, int> m_index;
    std::vector<Mip> m_mips;
};

} // namespace lb
