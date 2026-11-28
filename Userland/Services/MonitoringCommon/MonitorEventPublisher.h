/*
 * Copyright (c) 2026, Exile OS contributors
 * SPDX-License-Identifier: BSD-2-Clause
 */
#pragma once

#include <stddef.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

namespace Exile::MonitoringCommon {

static constexpr char system_log_socket_path[] = "/tmp/portal/syslog";
static constexpr size_t maximum_event_size = 1024;

// Build a bounded syslog datagram. Control characters are replaced so that
// untrusted or malformed text cannot forge additional log records.
inline size_t format_monitor_event(char* output, size_t capacity, char const* service, char const* detail)
{
    if (!output || capacity == 0)
        return 0;

    size_t used = 0;
    auto append = [&](char ch) {
        if (used + 1 >= capacity)
            return false;
        if (ch == '\n' || ch == '\r' || ch == '\0' || static_cast<unsigned char>(ch) < 0x20)
            ch = ' ';
        output[used++] = ch;
        return true;
    };

    auto append_text = [&](char const* text) {
        if (!text)
            return;
        for (size_t i = 0; text[i] != '\0'; ++i) {
            if (!append(text[i]))
                break;
        }
    };

    append('<');
    append('5');
    append('>');
    append_text(service ? service : "Monitor");
    append(':');
    append(' ');
    append_text(detail ? detail : "(no detail)");
    output[used] = '\0';
    return used;
}

// Best-effort delivery over the repository's existing SystemLog AF_LOCAL
// datagram endpoint. A missing logger must never stop a monitor service.
inline bool publish_monitor_event(char const* service, char const* detail)
{
    char record[maximum_event_size] {};
    auto length = format_monitor_event(record, sizeof(record), service, detail);
    if (length == 0)
        return false;

    int socket_fd = ::socket(AF_LOCAL, SOCK_DGRAM | SOCK_CLOEXEC, 0);
    if (socket_fd < 0)
        return false;

    struct sockaddr_un address {};
    address.sun_family = AF_LOCAL;
    constexpr size_t path_length = sizeof(system_log_socket_path);
    if (path_length > sizeof(address.sun_path)) {
        ::close(socket_fd);
        return false;
    }
    memcpy(address.sun_path, system_log_socket_path, path_length);

    auto address_length = static_cast<socklen_t>(offsetof(struct sockaddr_un, sun_path) + path_length);
    auto sent = ::sendto(socket_fd, record, length, 0,
        reinterpret_cast<struct sockaddr const*>(&address), address_length);
    ::close(socket_fd);
    return sent == static_cast<ssize_t>(length);
}

}
