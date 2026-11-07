/*
 * Copyright (c) 2026, Exile OS contributors
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include <AK/Debug.h>
#include <AK/StringBuilder.h>
#include <LibCore/EventLoop.h>
#include <LibCore/Notifier.h>
#include <LibCore/System.h>
#include <LibMain/Main.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>

static constexpr char socket_path[] = "/tmp/portal/syslog";
static constexpr char log_path[] = "/var/log/system.log";
static constexpr char first_archive_path[] = "/var/log/system.log.1";
static constexpr char second_archive_path[] = "/var/log/system.log.2";
static constexpr size_t maximum_log_bytes = 1024 * 1024;
static constexpr size_t maximum_record_bytes = 2048;

static void rotate_if_needed(int fd, size_t incoming_size)
{
    struct stat status {};
    if (fstat(fd, &status) < 0 || status.st_size < 0)
        return;
    auto current_size = static_cast<size_t>(status.st_size);
    if (incoming_size > maximum_log_bytes || current_size <= maximum_log_bytes - incoming_size)
        return;

    close(fd);
    (void)unlink(second_archive_path);
    (void)rename(first_archive_path, second_archive_path);
    (void)rename(log_path, first_archive_path);
}

static StringView severity_name(unsigned priority)
{
    switch (priority) {
    case 0:
    case 1:
    case 2:
        return "critical"sv;
    case 3:
        return "error"sv;
    case 4:
        return "warning"sv;
    case 5:
    case 6:
        return "info"sv;
    case 7:
        return "debug"sv;
    default:
        return "unknown"sv;
    }
}

static ErrorOr<void> append_record(StringView payload, unsigned priority)
{
    auto now = time(nullptr);
    struct tm utc_time {};
    if (!gmtime_r(&now, &utc_time))
        return Error::from_errno(EIO);

    char timestamp[32] {};
    if (strftime(timestamp, sizeof(timestamp), "%Y-%m-%dT%H:%M:%SZ", &utc_time) == 0)
        return Error::from_errno(EOVERFLOW);

    StringBuilder line;
    TRY(line.try_appendff("{} {} {}\n", timestamp, severity_name(priority), payload));
    if (line.length() > maximum_record_bytes)
        return Error::from_errno(EMSGSIZE);

    auto fd = open(log_path, O_WRONLY | O_APPEND | O_CREAT | O_CLOEXEC, 0640);
    if (fd < 0)
        return Error::from_errno(errno);
    rotate_if_needed(fd, line.length());

    // Rotation may have closed the original descriptor. Reopen append-only so
    // the record always goes to the active bounded log, not an archived inode.
    if (fcntl(fd, F_GETFD) < 0 && errno == EBADF)
        fd = open(log_path, O_WRONLY | O_APPEND | O_CREAT | O_CLOEXEC, 0640);
    if (fd < 0)
        return Error::from_errno(errno);

    size_t written = 0;
    while (written < line.length()) {
        auto rc = write(fd, line.string_view().substring_view(written).characters_without_null_termination(), line.length() - written);
        if (rc < 0) {
            auto error = errno;
            close(fd);
            return Error::from_errno(error);
        }
        if (rc == 0) {
            close(fd);
            return Error::from_errno(EIO);
        }
        written += static_cast<size_t>(rc);
    }
    if (fsync(fd) < 0) {
        auto error = errno;
        close(fd);
        return Error::from_errno(error);
    }
    close(fd);
    return {};
}

ErrorOr<int> serenity_main(Main::Arguments)
{
    TRY(Core::System::pledge("stdio rpath wpath cpath dpath unix"));
    TRY(Core::System::unveil("/tmp/portal", "rwc"));
    TRY(Core::System::unveil("/var/log", "rwc"));
    TRY(Core::System::unveil(nullptr, nullptr));

    (void)mkdir("/var/log", 0755);
    (void)unlink(socket_path);
    int fd = TRY(Core::System::socket(AF_LOCAL, SOCK_DGRAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0));
    ScopeGuard close_socket = [&] {
        (void)Core::System::close(fd);
        (void)Core::System::unlink(socket_path);
    };
    struct sockaddr_un address {};
    address.sun_family = AF_LOCAL;
    if (sizeof(socket_path) > sizeof(address.sun_path))
        return Error::from_errno(ENAMETOOLONG);
    memcpy(address.sun_path, socket_path, sizeof(socket_path));
    TRY(Core::System::bind(fd, reinterpret_cast<struct sockaddr const*>(&address), sizeof(address)));
    TRY(Core::System::chmod(socket_path, 0660));

    Core::EventLoop event_loop;
    auto notifier = Core::Notifier::construct(fd, Core::Notifier::Type::Read);
    notifier->on_activation = [fd] {
        for (;;) {
            char buffer[maximum_record_bytes + 1] {};
            auto received = recv(fd, buffer, sizeof(buffer), MSG_DONTWAIT);
            if (received < 0) {
                if (errno == EAGAIN || errno == EWOULDBLOCK)
                    return;
                warnln("SystemLog: receive failed: {}", strerror(errno));
                return;
            }
            if (received == 0)
                return;

            StringView record { buffer, static_cast<size_t>(received) };
            if (!record.starts_with("<"sv)) {
                warnln("SystemLog: rejecting malformed record");
                continue;
            }
            auto close_bracket = record.find('>');
            if (!close_bracket.has_value() || close_bracket.value() < 2 || close_bracket.value() > 3) {
                warnln("SystemLog: rejecting malformed priority");
                continue;
            }
            auto priority_value = record.substring_view(1, close_bracket.value() - 1).to_number<unsigned>();
            if (!priority_value.has_value() || priority_value.value() > 7) {
                warnln("SystemLog: rejecting invalid priority");
                continue;
            }

            auto payload = record.substring_view(close_bracket.value() + 1);
            // Avoid control characters and embedded newlines creating forged
            // records in the persistent log.
            StringBuilder sanitized;
            for (auto ch : payload) {
                if (ch == '\n' || ch == '\r' || ch == '\0')
                    ch = ' ';
                if (ch >= 0x20 || ch == '\t')
                    sanitized.append(ch);
            }
            if (sanitized.is_empty())
                sanitized.append("(empty message)"sv);

            if (auto result = append_record(sanitized.string_view(), priority_value.value()); result.is_error())
                warnln("SystemLog: unable to persist record: {}", result.error());
        }
    };
    (void)close_socket.disarm();
    return event_loop.exec();
}
