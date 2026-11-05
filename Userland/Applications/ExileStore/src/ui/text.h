/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#pragma once

// Text-layout helpers for the terminal front-end and the Exile brand palette
// (black / white / green / blue; no purple).

#include <string>
#include <string_view>
#include <vector>

namespace exile::ui {

class Theme {
public:
    explicit Theme(bool color = false)
        : color_(color)
    {
    }
    bool color() const { return color_; }

    std::string bold(std::string_view s) const { return wrap("1", s); }
    std::string green(std::string_view s) const { return wrap("1;32", s); } // primary actions, success
    std::string blue(std::string_view s) const { return wrap("34", s); }    // headings, links
    std::string dim(std::string_view s) const { return wrap("2", s); }      // secondary text

private:
    std::string wrap(char const* code, std::string_view s) const;
    bool color_;
};

// Columns occupied by `s`: code points, ignoring ANSI escape sequences.
size_t displayWidth(std::string_view s);
// Plain-text only (call before coloring). Cuts on a code-point boundary and appends "..." when shortened.
std::string truncate(std::string_view s, size_t width);
std::string padRight(std::string_view s, size_t width);
std::string repeat(char c, size_t n);
std::vector<std::string> wrapText(std::string_view text, size_t width);
std::string progressBar(double fraction, size_t width);

} // namespace exile::ui
