/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include "http.h"

#include <algorithm>
#include <cerrno>
#include <chrono>
#include <cstring>

#include <arpa/inet.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <sys/socket.h>
#include <unistd.h>

namespace exile::server {

char const* reasonPhrase(int s)
{
    switch (s) {
    case 200:
        return "OK";
    case 204:
        return "No Content";
    case 400:
        return "Bad Request";
    case 401:
        return "Unauthorized";
    case 403:
        return "Forbidden";
    case 404:
        return "Not Found";
    case 405:
        return "Method Not Allowed";
    case 409:
        return "Conflict";
    case 413:
        return "Payload Too Large";
    case 415:
        return "Unsupported Media Type";
    case 431:
        return "Request Header Fields Too Large";
    case 500:
        return "Internal Server Error";
    case 503:
        return "Service Unavailable";
    default:
        return "Unknown";
    }
}

std::string percentDecode(std::string const& s, bool& ok)
{
    ok = true;
    std::string out;
    for (size_t i = 0; i < s.size(); ++i) {
        if (s[i] != '%') {
            out += s[i];
            continue;
        }
        if (i + 2 >= s.size()) {
            ok = false;
            return {};
        }
        auto hex = [](char c) -> int {
            if (c >= '0' && c <= '9')
                return c - '0';
            if (c >= 'a' && c <= 'f')
                return c - 'a' + 10;
            if (c >= 'A' && c <= 'F')
                return c - 'A' + 10;
            return -1;
        };
        int hi = hex(s[i + 1]), lo = hex(s[i + 2]);
        if (hi < 0 || lo < 0) {
            ok = false;
            return {};
        }
        char c = static_cast<char>(hi * 16 + lo);
        if (c == '\0') {
            ok = false;
            return {};
        }
        out += c;
        i += 2;
    }
    return out;
}

std::string parseRequest(std::string const& head, Request& out)
{
    size_t eol = head.find("\r\n");
    if (eol == std::string::npos)
        return "malformed request line";
    std::string line = head.substr(0, eol);
    size_t s1 = line.find(' ');
    size_t s2 = s1 == std::string::npos ? s1 : line.find(' ', s1 + 1);
    if (s1 == std::string::npos || s2 == std::string::npos)
        return "malformed request line";
    out.method = line.substr(0, s1);
    out.target = line.substr(s1 + 1, s2 - s1 - 1);
    std::string version = line.substr(s2 + 1);
    if (version != "HTTP/1.1" && version != "HTTP/1.0")
        return "unsupported HTTP version";
    if (out.target.empty() || out.target[0] != '/' || out.target.compare(0, 2, "//") == 0)
        return "unsupported request target";
    for (unsigned char c : out.target)
        if (c < 0x21 || c == 0x7f)
            return "invalid character in request target";

    std::string rawPath = out.target;
    size_t q = rawPath.find('?');
    if (q != std::string::npos) {
        out.query = rawPath.substr(q + 1);
        rawPath.resize(q);
    }
    size_t hash = rawPath.find('#');
    if (hash != std::string::npos)
        rawPath.resize(hash);
    bool ok = true;
    out.path = percentDecode(rawPath, ok);
    if (!ok)
        return "invalid percent-encoding";
    for (unsigned char c : out.path)
        if (c < 0x20 || c == 0x7f || c == '\\')
            return "invalid character in path";

    size_t pos = eol + 2;
    while (pos < head.size()) {
        size_t e = head.find("\r\n", pos);
        if (e == std::string::npos)
            e = head.size();
        if (e == pos)
            break;
        std::string h = head.substr(pos, e - pos);
        pos = e + 2;
        size_t colon = h.find(':');
        if (colon == std::string::npos || colon == 0)
            return "malformed header";
        std::string name = h.substr(0, colon);
        for (char& c : name) {
            if (c == ' ' || c == '\t')
                return "malformed header name";
            c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
        }
        std::string value = h.substr(colon + 1);
        size_t b = value.find_first_not_of(" \t");
        size_t en = value.find_last_not_of(" \t");
        value = b == std::string::npos ? std::string() : value.substr(b, en - b + 1);
        for (unsigned char c : value)
            if ((c < 0x20 && c != '\t') || c == 0x7f)
                return "invalid header value";
        // Duplicate Host / Content-Length are a request-smuggling smell: refuse.
        if (out.headers.count(name) && (name == "host" || name == "content-length" || name == "transfer-encoding"))
            return "duplicate " + name + " header";
        out.headers[name] = value;
    }
    if (out.headers.count("transfer-encoding"))
        return "transfer-encoding is not supported";
    return {};
}

namespace {

bool sendAll(int fd, std::string const& data)
{
    size_t off = 0;
    while (off < data.size()) {
#ifdef MSG_NOSIGNAL
        ssize_t n = ::send(fd, data.data() + off, data.size() - off, MSG_NOSIGNAL);
#else
        ssize_t n = ::send(fd, data.data() + off, data.size() - off, 0);
#endif
        if (n < 0) {
            if (errno == EINTR)
                continue;
            return false;
        }
        off += static_cast<size_t>(n);
    }
    return true;
}

void writeResponse(int fd, Response const& r, bool headOnly)
{
    std::string out = "HTTP/1.1 " + std::to_string(r.status) + " " + reasonPhrase(r.status) + "\r\n";
    out += "Content-Type: " + r.contentType + "\r\n";
    out += "Content-Length: " + std::to_string(r.body.size()) + "\r\n";
    out += "Connection: close\r\n";
    for (auto const& [k, v] : r.headers)
        out += k + ": " + v + "\r\n";
    out += "\r\n";
    if (!headOnly)
        out += r.body;
    sendAll(fd, out);
}

Response plain(int status, std::string const& msg)
{
    Response r;
    r.status = status;
    r.body = msg + "\n";
    r.headers["Cache-Control"] = "no-store";
    r.headers["X-Content-Type-Options"] = "nosniff";
    return r;
}

} // namespace

Server::Server(Handler handler, Limits limits)
    : handler_(std::move(handler))
    , limits_(limits)
{
}
Server::~Server() { stop(); }

bool Server::listen(unsigned short port, std::string& error)
{
    int fd = ::socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) {
        error = std::string("socket: ") + std::strerror(errno);
        return false;
    }
    int one = 1;
    ::setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one);
    sockaddr_in addr {};
    addr.sin_family = AF_INET;
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK); // never anything but loopback
    addr.sin_port = htons(port);
    if (::bind(fd, reinterpret_cast<sockaddr*>(&addr), sizeof addr) != 0) {
        error = "could not listen on 127.0.0.1:" + std::to_string(port) + " (" + std::strerror(errno) + ")";
        ::close(fd);
        return false;
    }
    if (::listen(fd, 16) != 0) {
        error = std::string("listen: ") + std::strerror(errno);
        ::close(fd);
        return false;
    }
    socklen_t len = sizeof addr;
    ::getsockname(fd, reinterpret_cast<sockaddr*>(&addr), &len);
    port_ = ntohs(addr.sin_port);
    listenFd_ = fd;
    return true;
}

void Server::start()
{
    if (listenFd_ < 0 || running_.exchange(true))
        return;
    thread_ = std::thread([this] { acceptLoop(); });
}

void Server::stop()
{
    if (running_.exchange(false) && thread_.joinable())
        thread_.join();
    if (listenFd_ >= 0) {
        ::close(listenFd_);
        listenFd_ = -1;
    }
    for (int i = 0; i < 300 && active_.load() > 0; ++i)
        std::this_thread::sleep_for(std::chrono::milliseconds(20));
}

void Server::acceptLoop()
{
    while (running_.load()) {
        pollfd p { listenFd_, POLLIN, 0 };
        int r = ::poll(&p, 1, 200);
        if (r <= 0)
            continue;
        int fd = ::accept(listenFd_, nullptr, nullptr);
        if (fd < 0)
            continue;
        timeval tv { limits_.ioTimeoutSeconds, 0 };
        ::setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv);
        ::setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof tv);
        if (active_.load() >= limits_.maxConnections) {
            writeResponse(fd, plain(503, "Too many connections"), false);
            ::close(fd);
            continue;
        }
        ++active_;
        std::thread([this, fd] {
            serve(fd);
            ::close(fd);
            --active_;
        }).detach();
    }
}

void Server::serve(int fd)
{
    std::string buf;
    size_t headEnd = std::string::npos;
    char chunk[4096];
    while (headEnd == std::string::npos) {
        if (buf.size() > limits_.maxHeaderBytes) {
            writeResponse(fd, plain(431, "Headers too large"), false);
            return;
        }
        ssize_t n = ::recv(fd, chunk, sizeof chunk, 0);
        if (n <= 0)
            return;
        buf.append(chunk, static_cast<size_t>(n));
        headEnd = buf.find("\r\n\r\n");
    }
    if (headEnd > limits_.maxHeaderBytes) {
        writeResponse(fd, plain(431, "Headers too large"), false);
        return;
    }

    Request req;
    std::string err = parseRequest(buf.substr(0, headEnd + 2), req);
    if (!err.empty()) {
        writeResponse(fd, plain(400, err), false);
        return;
    }

    size_t contentLength = 0;
    std::string cl = req.header("content-length");
    if (!cl.empty()) {
        if (cl.size() > 9 || cl.find_first_not_of("0123456789") != std::string::npos) {
            writeResponse(fd, plain(400, "bad content-length"), false);
            return;
        }
        contentLength = static_cast<size_t>(std::stoul(cl));
    }
    if (contentLength > limits_.maxBodyBytes) {
        writeResponse(fd, plain(413, "Body too large"), false);
        return;
    }
    if (req.method != "GET" && req.method != "HEAD" && req.method != "POST") {
        Response r = plain(405, "Method not allowed");
        r.headers["Allow"] = "GET, HEAD, POST";
        writeResponse(fd, r, false);
        return;
    }
    std::string body = buf.substr(headEnd + 4);
    while (body.size() < contentLength) {
        ssize_t n = ::recv(fd, chunk, sizeof chunk, 0);
        if (n <= 0)
            return;
        body.append(chunk, static_cast<size_t>(n));
        if (body.size() > limits_.maxBodyBytes) {
            writeResponse(fd, plain(413, "Body too large"), false);
            return;
        }
    }
    body.resize(contentLength);
    req.body = std::move(body);

    Response resp;
    try {
        resp = handler_(req);
    } catch (std::exception const&) {
        resp = plain(500, "Internal error");
    } catch (...) {
        resp = plain(500, "Internal error");
    }
    writeResponse(fd, resp, req.method == "HEAD");
}

} // namespace exile::server
