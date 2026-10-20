/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include "util.h"
#include <cstdlib>

#include <algorithm>
#include <cstdio>
#include <ctime>
#include <fstream>
#include <system_error>

namespace exile::util {

bool hasControlChars(std::string_view s, bool allowNewlineAndTab)
{
    for (unsigned char c : s) {
        if (c == 0)
            return true;
        if (c < 0x20 || c == 0x7f) {
            if (allowNewlineAndTab && (c == '\n' || c == '\t'))
                continue;
            return true;
        }
    }
    return false;
}

bool isSafeRelativePath(std::string_view path)
{
    if (path.empty() || path.size() > 1024)
        return false;
    if (path.front() == '/')
        return false;
    if (path.find('\\') != std::string_view::npos)
        return false;
    if (hasControlChars(path))
        return false;

    size_t start = 0;
    bool first = true;
    while (start <= path.size()) {
        size_t end = path.find('/', start);
        if (end == std::string_view::npos)
            end = path.size();
        std::string_view seg = path.substr(start, end - start);
        if (seg.empty() || seg == "." || seg == ".." || seg.size() > 255)
            return false;
        if (first && seg.find(':') != std::string_view::npos)
            return false;
        first = false;
        if (end == path.size())
            break;
        start = end + 1;
    }
    return true;
}

std::string toLower(std::string_view s)
{
    std::string out(s);
    for (char& c : out)
        if (c >= 'A' && c <= 'Z')
            c = static_cast<char>(c - 'A' + 'a');
    return out;
}

bool isLowerHex(std::string_view s, size_t len)
{
    if (s.size() != len)
        return false;
    return std::all_of(s.begin(), s.end(), [](char c) { return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f'); });
}

std::optional<std::vector<uint8_t>> hexDecode(std::string_view hex)
{
    if (hex.size() % 2 != 0)
        return std::nullopt;
    auto nibble = [](char c) -> int {
        if (c >= '0' && c <= '9')
            return c - '0';
        if (c >= 'a' && c <= 'f')
            return c - 'a' + 10;
        if (c >= 'A' && c <= 'F')
            return c - 'A' + 10;
        return -1;
    };
    std::vector<uint8_t> out;
    out.reserve(hex.size() / 2);
    for (size_t i = 0; i < hex.size(); i += 2) {
        int hi = nibble(hex[i]);
        int lo = nibble(hex[i + 1]);
        if (hi < 0 || lo < 0)
            return std::nullopt;
        out.push_back(static_cast<uint8_t>(hi * 16 + lo));
    }
    return out;
}

std::string hexEncode(uint8_t const* data, size_t size)
{
    static char const* digits = "0123456789abcdef";
    std::string out;
    out.reserve(size * 2);
    for (size_t i = 0; i < size; ++i) {
        out.push_back(digits[data[i] >> 4]);
        out.push_back(digits[data[i] & 0xf]);
    }
    return out;
}

std::string humanSize(uint64_t bytes)
{
    static char const* units[] = { "B", "KB", "MB", "GB", "TB" };
    double v = static_cast<double>(bytes);
    size_t u = 0;
    while (v >= 1000.0 && u < 4) {
        v /= 1000.0;
        ++u;
    }
    char buf[32];
    if (u == 0)
        std::snprintf(buf, sizeof buf, "%llu B", static_cast<unsigned long long>(bytes));
    else
        std::snprintf(buf, sizeof buf, v >= 100 ? "%.0f %s" : "%.1f %s", v, units[u]);
    return buf;
}

std::string nowIso8601()
{
    std::time_t t = std::time(nullptr);
    std::tm tm {};
#ifdef _WIN32
    gmtime_s(&tm, &t);
#else
    gmtime_r(&t, &tm);
#endif
    char buf[32];
    std::strftime(buf, sizeof buf, "%Y-%m-%dT%H:%M:%SZ", &tm);
    return buf;
}

std::optional<std::string> readFile(std::filesystem::path const& path, uint64_t maxBytes, std::string* error)
{
    std::error_code ec;
    auto size = std::filesystem::file_size(path, ec);
    if (ec) {
        if (error)
            *error = "cannot read " + path.string() + ": " + ec.message();
        return std::nullopt;
    }
    if (size > maxBytes) {
        if (error)
            *error = path.string() + " is too large";
        return std::nullopt;
    }
    std::ifstream in(path, std::ios::binary);
    if (!in) {
        if (error)
            *error = "cannot open " + path.string();
        return std::nullopt;
    }
    std::string data(static_cast<size_t>(size), '\0');
    in.read(data.data(), static_cast<std::streamsize>(size));
    if (static_cast<uint64_t>(in.gcount()) != size) {
        if (error)
            *error = "short read on " + path.string();
        return std::nullopt;
    }
    return data;
}

bool writeFileAtomic(std::filesystem::path const& path, std::string_view data, std::string& error)
{
    std::error_code ec;
    if (path.has_parent_path())
        std::filesystem::create_directories(path.parent_path(), ec);
    std::filesystem::path tmp = path;
    tmp += ".tmp";
    {
        std::ofstream out(tmp, std::ios::binary | std::ios::trunc);
        if (!out) {
            error = "cannot write " + tmp.string();
            return false;
        }
        out.write(data.data(), static_cast<std::streamsize>(data.size()));
        out.flush();
        if (!out) {
            error = "write failed for " + tmp.string();
            return false;
        }
    }
    std::filesystem::rename(tmp, path, ec);
    if (ec) {
        error = "cannot replace " + path.string() + ": " + ec.message();
        std::filesystem::remove(tmp, ec);
        return false;
    }
    return true;
}

bool containsIgnoreCase(std::string_view haystack, std::string_view needle)
{
    if (needle.empty())
        return true;
    return toLower(haystack).find(toLower(needle)) != std::string::npos;
}

std::filesystem::path defaultDataDir()
{
    namespace fs = std::filesystem;
    if (char const* x = std::getenv("XDG_DATA_HOME"))
        return fs::path(x) / "exile-store";
    if (char const* h = std::getenv("HOME"))
        return fs::path(h) / ".local/share/exile-store";
    return fs::path(".exile-store");
}

std::filesystem::path findDefaultCatalog()
{
    namespace fs = std::filesystem;
    for (fs::path p : { fs::path("data/catalog/catalog.json"), fs::path("../data/catalog/catalog.json"),
             fs::path("/usr/local/share/exile-store/catalog/catalog.json"),
             fs::path("/usr/share/exile-store/catalog/catalog.json") }) {
        std::error_code ec;
        if (fs::exists(p, ec))
            return p;
    }
    return "data/catalog/catalog.json";
}

} // namespace exile::util
