#pragma once
#include <cstdint>
#include <string>
#include <vector>

namespace lb {

struct ImageRGBA8 {
    int w = 0, h = 0;
    std::vector<uint8_t> px; // straight (non-premultiplied) RGBA
};

bool load_png(std::string const& path, ImageRGBA8& out, std::string& error);
bool save_png_rgb(std::string const& path, int w, int h, uint8_t const* rgb, std::string& error);
bool save_png_rgba(std::string const& path, int w, int h, uint8_t const* rgba, std::string& error);

} // namespace lb
