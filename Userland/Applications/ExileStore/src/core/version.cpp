/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include "version.h"

#include <algorithm>

namespace exile {

namespace {

bool allDigits(std::string_view s)
{
    return !s.empty() && std::all_of(s.begin(), s.end(), [](char c) { return c >= '0' && c <= '9'; });
}

std::optional<uint64_t> numericPart(std::string_view s)
{
    if (!allDigits(s) || s.size() > 18)
        return std::nullopt;
    if (s.size() > 1 && s[0] == '0')
        return std::nullopt; // no leading zeros
    uint64_t v = 0;
    for (char c : s)
        v = v * 10 + static_cast<uint64_t>(c - '0');
    return v;
}

bool validIdentifier(std::string_view s)
{
    if (s.empty())
        return false;
    for (char c : s)
        if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || c == '-'))
            return false;
    if (allDigits(s) && s.size() > 1 && s[0] == '0')
        return false;
    return true;
}

} // namespace

std::string Version::str() const
{
    std::string out = std::to_string(major) + "." + std::to_string(minor) + "." + std::to_string(patch);
    for (size_t i = 0; i < prerelease.size(); ++i)
        out += (i == 0 ? "-" : ".") + prerelease[i];
    return out;
}

std::optional<Version> parseVersion(std::string_view text)
{
    if (text.empty() || text.size() > 100)
        return std::nullopt;
    if (size_t plus = text.find('+'); plus != std::string_view::npos) {
        std::string_view build = text.substr(plus + 1);
        if (build.empty())
            return std::nullopt;
        for (char c : build)
            if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || c == '-' || c == '.'))
                return std::nullopt;
        text = text.substr(0, plus);
    }
    std::string_view core = text;
    std::string_view pre;
    bool hasPre = false;
    if (size_t dash = text.find('-'); dash != std::string_view::npos) {
        core = text.substr(0, dash);
        pre = text.substr(dash + 1);
        hasPre = true;
    }

    Version v;
    size_t p1 = core.find('.');
    if (p1 == std::string_view::npos)
        return std::nullopt;
    size_t p2 = core.find('.', p1 + 1);
    if (p2 == std::string_view::npos)
        return std::nullopt;
    auto a = numericPart(core.substr(0, p1));
    auto b = numericPart(core.substr(p1 + 1, p2 - p1 - 1));
    auto c = numericPart(core.substr(p2 + 1));
    if (!a || !b || !c)
        return std::nullopt;
    v.major = *a;
    v.minor = *b;
    v.patch = *c;

    if (hasPre) {
        size_t start = 0;
        while (true) {
            size_t dot = pre.find('.', start);
            std::string_view id = pre.substr(start, dot == std::string_view::npos ? std::string_view::npos : dot - start);
            if (!validIdentifier(id))
                return std::nullopt;
            v.prerelease.emplace_back(id);
            if (dot == std::string_view::npos)
                break;
            start = dot + 1;
        }
    }
    return v;
}

int compareVersions(Version const& a, Version const& b)
{
    auto cmp = [](uint64_t x, uint64_t y) { return x < y ? -1 : (x > y ? 1 : 0); };
    if (int c = cmp(a.major, b.major))
        return c;
    if (int c = cmp(a.minor, b.minor))
        return c;
    if (int c = cmp(a.patch, b.patch))
        return c;
    if (a.prerelease.empty() && b.prerelease.empty())
        return 0;
    if (a.prerelease.empty())
        return 1;
    if (b.prerelease.empty())
        return -1;
    size_t n = std::min(a.prerelease.size(), b.prerelease.size());
    for (size_t i = 0; i < n; ++i) {
        std::string const& x = a.prerelease[i];
        std::string const& y = b.prerelease[i];
        bool xn = allDigits(x), yn = allDigits(y);
        if (xn && yn) {
            if (int c = cmp(*numericPart(x), *numericPart(y)))
                return c;
        } else if (xn != yn) {
            return xn ? -1 : 1; // numeric identifiers sort before alphanumeric ones
        } else if (x != y) {
            return x < y ? -1 : 1;
        }
    }
    return cmp(a.prerelease.size(), b.prerelease.size());
}

int compareVersionStrings(std::string_view a, std::string_view b)
{
    auto va = parseVersion(a);
    auto vb = parseVersion(b);
    if (!va && !vb)
        return 0;
    if (!va)
        return -1;
    if (!vb)
        return 1;
    return compareVersions(*va, *vb);
}

} // namespace exile
