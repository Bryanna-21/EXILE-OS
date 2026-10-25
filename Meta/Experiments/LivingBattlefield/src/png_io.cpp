#include "png_io.h"
#include <cstdio>
#include <cstring>
#include <png.h>

namespace lb {

bool load_png(std::string const& path, ImageRGBA8& out, std::string& error)
{
    FILE* f = std::fopen(path.c_str(), "rb");
    if (!f) { error = "cannot open " + path; return false; }
    png_structp png = png_create_read_struct(PNG_LIBPNG_VER_STRING, nullptr, nullptr, nullptr);
    png_infop info = png ? png_create_info_struct(png) : nullptr;
    if (!png || !info) { std::fclose(f); error = "libpng init failed"; return false; }
    if (setjmp(png_jmpbuf(png))) {
        png_destroy_read_struct(&png, &info, nullptr);
        std::fclose(f);
        error = "libpng decode error in " + path;
        return false;
    }
    png_init_io(png, f);
    png_read_info(png, info);
    png_set_expand(png);
    png_set_strip_16(png);
    png_set_gray_to_rgb(png);
    png_set_filler(png, 0xFF, PNG_FILLER_AFTER);
    png_read_update_info(png, info);
    out.w = int(png_get_image_width(png, info));
    out.h = int(png_get_image_height(png, info));
    out.px.assign(size_t(out.w) * out.h * 4, 255);
    std::vector<png_bytep> rows(out.h);
    for (int y = 0; y < out.h; ++y)
        rows[y] = out.px.data() + size_t(y) * out.w * 4;
    png_read_image(png, rows.data());
    png_destroy_read_struct(&png, &info, nullptr);
    std::fclose(f);
    return true;
}

static bool save_png(std::string const& path, int w, int h, uint8_t const* data, int channels, std::string& error)
{
    FILE* f = std::fopen(path.c_str(), "wb");
    if (!f) { error = "cannot write " + path; return false; }
    png_structp png = png_create_write_struct(PNG_LIBPNG_VER_STRING, nullptr, nullptr, nullptr);
    png_infop info = png ? png_create_info_struct(png) : nullptr;
    if (!png || !info) { std::fclose(f); error = "libpng init failed"; return false; }
    if (setjmp(png_jmpbuf(png))) {
        png_destroy_write_struct(&png, &info);
        std::fclose(f);
        error = "libpng encode error";
        return false;
    }
    png_init_io(png, f);
    png_set_IHDR(png, info, w, h, 8, channels == 4 ? PNG_COLOR_TYPE_RGBA : PNG_COLOR_TYPE_RGB,
        PNG_INTERLACE_NONE, PNG_COMPRESSION_TYPE_DEFAULT, PNG_FILTER_TYPE_DEFAULT);
    png_set_compression_level(png, 6);
    png_write_info(png, info);
    for (int y = 0; y < h; ++y)
        png_write_row(png, const_cast<png_bytep>(data + size_t(y) * w * channels));
    png_write_end(png, nullptr);
    png_destroy_write_struct(&png, &info);
    std::fclose(f);
    return true;
}

bool save_png_rgb(std::string const& path, int w, int h, uint8_t const* rgb, std::string& error)
{
    return save_png(path, w, h, rgb, 3, error);
}
bool save_png_rgba(std::string const& path, int w, int h, uint8_t const* rgba, std::string& error)
{
    return save_png(path, w, h, rgba, 4, error);
}

} // namespace lb
