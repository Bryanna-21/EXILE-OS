/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include <AK/ByteString.h>
#include <LibCore/Environment.h>
#include <LibCore/File.h>
#include <LibCore/Process.h>
#include <LibCore/System.h>
#include <LibCore/SystemServerTakeover.h>
#include <LibTest/TestCase.h>

#if !defined(AK_OS_WINDOWS)
#    include <stdlib.h>
#    include <sys/socket.h>
#    include <sys/wait.h>
#    include <unistd.h>

namespace {

// A scratch directory that is removed again when it goes out of scope.
class ScratchDirectory {
public:
    ScratchDirectory()
    {
        char pattern[] = "/tmp/exile-ipcprocess-XXXXXX";
        m_path = MUST(Core::System::mkdtemp(Span<char> { pattern, sizeof(pattern) }));
    }

    ~ScratchDirectory()
    {
        (void)Core::System::unlink(pid_file_path());
        (void)Core::System::rmdir(m_path);
    }

    ByteString pid_file_path() const { return ByteString::formatted("{}/test.pid", m_path); }

    void write_pid_file(StringView contents) const
    {
        auto file = MUST(Core::File::open(pid_file_path(), Core::File::OpenMode::Write));
        MUST(file->write_until_depleted(contents.bytes()));
    }

private:
    ByteString m_path;
};

}

// --- Core::Environment::unset, as used when taking over sockets from SystemServer ---

TEST_CASE(taking_over_a_socket_consumes_the_socket_takeover_variable)
{
    int fds[2];
    VERIFY(::socketpair(AF_UNIX, SOCK_STREAM, 0, fds) == 0);

    auto takeover = ByteString::formatted("testsocket:{}", fds[0]);
    MUST(Core::Environment::set("SOCKET_TAKEOVER"sv, takeover, Core::Environment::Overwrite::Yes));

    auto socket = Core::take_over_socket_from_system_server("testsocket"sv);
    EXPECT(!socket.is_error());

    // The variable must be gone, so children do not think they are being passed a socket.
    EXPECT(::getenv("SOCKET_TAKEOVER") == nullptr);

    ::close(fds[1]);
}

// --- Process::get_name(pid_t) ---

#    if defined(AK_OS_LINUX)
TEST_CASE(get_name_of_another_pid_matches_get_name_for_the_current_process)
{
    auto own_name = MUST(Core::Process::get_name());
    auto name_by_pid = MUST(Core::Process::get_name(::getpid()));
    EXPECT_EQ(name_by_pid, own_name);
}

TEST_CASE(get_name_of_a_pid_that_does_not_exist_is_an_error)
{
    // A process that has been reaped: its pid cannot be reused in the next few instructions on a quiet test machine,
    // and the lookup must fail either way unless another process took the pid, which is vanishingly unlikely here.
    auto pid = ::fork();
    VERIFY(pid >= 0);
    if (pid == 0)
        ::_exit(0);
    int status = 0;
    VERIFY(::waitpid(pid, &status, 0) == pid);

    EXPECT(Core::Process::get_name(pid).is_error());
}
#    endif

// --- IPCProcess::get_process_pid ---

TEST_CASE(get_process_pid_without_a_pid_file_finds_no_process)
{
    ScratchDirectory directory;
    auto result = Core::IPCProcess::get_process_pid("anything"sv, directory.pid_file_path());
    EXPECT(!result.is_error());
    EXPECT(!result.value().has_value());
}

TEST_CASE(get_process_pid_with_a_garbage_pid_file_is_an_error)
{
    ScratchDirectory directory;
    directory.write_pid_file("not-a-pid"sv);
    EXPECT(Core::IPCProcess::get_process_pid("anything"sv, directory.pid_file_path()).is_error());
}

TEST_CASE(get_process_pid_with_a_dead_pid_finds_no_process)
{
    auto pid = ::fork();
    VERIFY(pid >= 0);
    if (pid == 0)
        ::_exit(0);
    int status = 0;
    VERIFY(::waitpid(pid, &status, 0) == pid);

    ScratchDirectory directory;
    directory.write_pid_file(ByteString::number(pid));

    auto result = Core::IPCProcess::get_process_pid("anything"sv, directory.pid_file_path());
    EXPECT(!result.is_error());
    EXPECT(!result.value().has_value());
}

#    if defined(AK_OS_LINUX)
TEST_CASE(get_process_pid_returns_the_pid_of_a_live_process_with_the_expected_name)
{
    ScratchDirectory directory;
    directory.write_pid_file(ByteString::number(::getpid()));

    auto own_name = MUST(Core::Process::get_name());
    auto result = Core::IPCProcess::get_process_pid(own_name, directory.pid_file_path());
    EXPECT(!result.is_error());
    EXPECT(result.value().has_value());
    EXPECT_EQ(result.value().value(), ::getpid());
}

TEST_CASE(get_process_pid_ignores_a_live_process_with_a_different_name)
{
    // Guards against pid reuse: the pid in the file is alive but is not the process we are looking for.
    ScratchDirectory directory;
    directory.write_pid_file(ByteString::number(::getpid()));

    auto result = Core::IPCProcess::get_process_pid("definitely-not-this-process"sv, directory.pid_file_path());
    EXPECT(!result.is_error());
    EXPECT(!result.value().has_value());
}
#    endif

#endif
