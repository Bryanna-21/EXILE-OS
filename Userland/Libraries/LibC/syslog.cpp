/*
 * Copyright (c) 2018-2020, Andreas Kling <kling@serenityos.org>
 * Copyright (c) 2026, Exile OS contributors
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#define SYSLOG_NAMES 1

#include <AK/ByteString.h>
#include <AK/StringBuilder.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <syslog.h>
#include <unistd.h>

extern "C" {

static struct syslog_data global_log_data = {
    .ident = nullptr,
    .logopt = 0,
    .facility = LOG_USER,
    .maskpri = LOG_UPTO(LOG_DEBUG)
};

static char program_name_buffer[256];
static bool program_name_set = false;

static char const* get_syslog_ident(struct syslog_data* data)
{
    if (!program_name_set && data->ident == nullptr)
        program_name_set = get_process_name(program_name_buffer, sizeof(program_name_buffer)) >= 0;

    if (data->ident != nullptr)
        return data->ident;
    if (program_name_set)
        return program_name_buffer;
    return "unknown";
}

void openlog_r(char const* ident, int logopt, int facility, struct syslog_data* data)
{
    data->ident = ident;
    data->logopt = logopt;
    data->facility = facility;
    data->maskpri = LOG_UPTO(LOG_DEBUG);
}

void openlog(char const* ident, int logopt, int facility)
{
    openlog_r(ident, logopt, facility, &global_log_data);
}

void closelog_r(struct syslog_data* data)
{
    data->ident = nullptr;
    data->logopt = 0;
    data->facility = LOG_USER;
    data->maskpri = LOG_UPTO(LOG_DEBUG);
}

void closelog(void)
{
    closelog_r(&global_log_data);
}

int setlogmask_r(int maskpri, struct syslog_data* data)
{
    int old_maskpri = data->maskpri;
    data->maskpri = maskpri;
    return old_maskpri;
}

int setlogmask(int maskpri)
{
    return setlogmask_r(maskpri, &global_log_data);
}

void syslog_r(int priority, struct syslog_data* data, char const* message, ...)
{
    va_list ap;
    va_start(ap, message);
    vsyslog_r(priority, data, message, ap);
    va_end(ap);
}

void syslog(int priority, char const* message, ...)
{
    va_list ap;
    va_start(ap, message);
    vsyslog_r(priority, &global_log_data, message, ap);
    va_end(ap);
}

void vsyslog_r(int priority, struct syslog_data* data, char const* message, va_list args)
{
    int real_priority = LOG_PRI(priority);
    if (real_priority < LOG_EMERG || real_priority > LOG_DEBUG || !(data->maskpri & LOG_MASK(real_priority)))
        return;

    char formatted_message[1500];
    va_list copied_args;
    va_copy(copied_args, args);
    int message_length = vsnprintf(formatted_message, sizeof(formatted_message), message, copied_args);
    va_end(copied_args);
    if (message_length < 0)
        return;
    auto bounded_length = min(static_cast<size_t>(message_length), sizeof(formatted_message) - 1);

    StringBuilder record;
    record.appendff("<{}>{}", real_priority, get_syslog_ident(data));
    if (data->logopt & LOG_PID)
        record.appendff("[{}]", getpid());
    record.append(": "sv);
    record.append(StringView { formatted_message, bounded_length });
    auto bytes = record.to_byte_string();

    int fd = socket(AF_LOCAL, SOCK_DGRAM | SOCK_CLOEXEC, 0);
    if (fd >= 0) {
        struct sockaddr_un address {};
        address.sun_family = AF_LOCAL;
        constexpr char path[] = "/tmp/portal/syslog";
        if (sizeof(path) <= sizeof(address.sun_path)) {
            memcpy(address.sun_path, path, sizeof(path));
            auto sent = sendto(fd, bytes.characters(), bytes.length(), MSG_NOSIGNAL,
                reinterpret_cast<struct sockaddr*>(&address), sizeof(address));
            close(fd);
            if (sent == static_cast<ssize_t>(bytes.length()))
                return;
        } else {
            close(fd);
        }
    }

    // Keep syslog useful before SystemLog is available or if its socket is down.
    if (data->logopt & LOG_CONS)
        dbgputstr(bytes.characters(), bytes.length());
    if (data->logopt & LOG_PERROR)
        fputs(bytes.characters(), stderr);
}

void vsyslog(int priority, char const* message, va_list args)
{
    vsyslog_r(priority, &global_log_data, message, args);
}

}
