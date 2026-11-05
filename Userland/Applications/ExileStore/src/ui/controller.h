/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#pragma once

// Navigation state machine. Turns typed commands into either a screen change or
// a Command for the caller to execute (install, update, ...). It performs no
// I/O and no package operations itself.

#include <string>
#include <vector>

#include "render.h"

namespace exile::ui {

enum class Screen { Home,
    Explore,
    Search,
    Library,
    Details };

struct Command {
    enum class Kind { None,
        Install,
        Update,
        UpdateAll,
        Uninstall,
        Open,
        Quit } kind
        = Kind::None;
    std::string appId;
};

class UiController {
public:
    UiController(ViewContext const& ctx, size_t width, Theme theme);

    // Handles one line of input. Never throws on bad input.
    Command handle(std::string const& line);

    // Text to show next; `message` is a one-shot status line (cleared after rendering).
    std::string render();
    Page currentPage() const;

    void setMessage(std::string m) { message_ = std::move(m); }
    void setWidth(size_t w) { width_ = w; }
    void setTheme(Theme t) { theme_ = t; }

    Screen screen() const { return nav_.screen; }
    std::string const& selectedApp() const { return nav_.appId; }
    SearchQuery const& query() const { return nav_.query; }

private:
    struct Nav {
        Screen screen = Screen::Home;
        SearchQuery query;
        std::string appId;
    };
    void go(Screen s);
    void push();
    Command openItem(size_t number);

    ViewContext ctx_;
    size_t width_;
    Theme theme_;
    Nav nav_;
    std::vector<Nav> back_;
    std::string message_;
};

std::string helpText();

} // namespace exile::ui
