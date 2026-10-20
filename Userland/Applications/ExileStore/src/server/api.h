/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#pragma once

// Request handler for the local web front-end: static assets plus a small JSON API
// over app::Engine. All request-level security decisions live here:
//
//   * Host header must be the loopback address we listen on (blocks DNS rebinding).
//   * /api/* needs a per-run random bearer token (compared in constant time).
//   * If the browser sends Origin it must be our own origin; Sec-Fetch-Site must be
//     same-origin or none. POST bodies must be application/json.
//   * Every response carries a strict Content-Security-Policy and no-sniff headers.
//
// The server has no way to run arbitrary commands, read arbitrary files, or install
// anything the installer pipeline would refuse: it only forwards intents.

#include <string>

#include "../app/engine.h"
#include "http.h"

namespace exile::server {

class Api {
public:
    Api(app::Engine& engine, std::string token);

    void setPort(unsigned short port) { port_ = port; }
    Response handle(Request const& req);

    // 256 bits from the OS random source; empty (and error set) if unavailable.
    static std::string makeToken(std::string& error);

private:
    Response dispatchApi(Request const& req);
    Response serveAsset(Request const& req);

    app::Engine& engine_;
    std::string token_;
    unsigned short port_ = 0;
};

} // namespace exile::server
