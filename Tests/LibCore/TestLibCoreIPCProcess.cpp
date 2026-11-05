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
#    include <signal.h>
#    include <errno.h>
#    include <stdlib.h>
#    include <string.h>
#    include <sys/socket.h>
#    include <sys/wait.h>
#    include <unistd.h>

namespace {

// A pid that no running process can have: it is above the maximum pid of every supported platform (Linux caps at
// 2^22, other Unixes lower), so lookups of it are deterministic and do not depend on pids being reused or not.
constexpr pid_t s_impossible_pid = 0x7fffffff;

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
        // Also removes a directory placed at the pid path by a test, then the pid file, then the scratch directory.
        (void)Core::System::rmdir(pid_file_path());
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
    EXPECT(Core::Process::get_name(s_impossible_pid).is_error());
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
    ScratchDirectory directory;
    directory.write_pid_file(ByteString::number(s_impossible_pid));

    auto result = Core::IPCProcess::get_process_pid("anything"sv, directory.pid_file_path());
    EXPECT(!result.is_error());
    EXPECT(!result.value().has_value());
}

TEST_CASE(get_process_pid_rejects_empty_non_positive_and_out_of_range_pids)
{
    // 0 and negative pids would make kill(pid, 0) address a process group or every process, so they must be refused
    // before any signal is sent. 2147483648 does not fit in pid_t.
    for (auto contents : { ""sv, "0"sv, "-1"sv, "-12345"sv, "2147483648"sv, "12 34"sv, "0x10"sv }) {
        ScratchDirectory directory;
        directory.write_pid_file(contents);
        auto result = Core::IPCProcess::get_process_pid("anything"sv, directory.pid_file_path());
        EXPECT(result.is_error());
        if (result.is_error())
            EXPECT_EQ(result.error().code(), EINVAL);
    }
}

TEST_CASE(get_process_pid_with_an_unreadable_pid_path_is_an_error)
{
    // A directory where the pid file should be: stat succeeds, reading fails. Works the same for root and non-root.
    ScratchDirectory directory;
    MUST(Core::System::mkdir(directory.pid_file_path(), 0700));
    EXPECT(Core::IPCProcess::get_process_pid("anything"sv, directory.pid_file_path()).is_error());
}

TEST_CASE(taking_over_a_socket_that_was_not_passed_is_an_error)
{
    auto result = Core::take_over_socket_from_system_server("a-socket-nobody-passed"sv);
    EXPECT(result.is_error());
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

namespace {

// A live child whose argv[0] is a known name, so identity matching can be tested against a process that is not us.
// The child is always killed and reaped, including when a test fails.
class NamedChild {
public:
    explicit NamedChild(char const* name)
    {
        m_pid = ::fork();
        VERIFY(m_pid >= 0);
        if (m_pid == 0) {
            ::execl("/bin/sleep", name, "60", nullptr);
            ::_exit(127);
        }
        // Until exec has happened the child still carries our argv[0]; wait (bounded) for the new name to show up.
        for (int attempt = 0; attempt < 500; ++attempt) {
            auto current = Core::Process::get_name(m_pid);
            if (!current.is_error() && current.value() == StringView { name, strlen(name) })
                return;
            ::usleep(10'000);
        }
        // A failed constructor does not run the destructor; clean up the child explicitly before failing.
        ::kill(m_pid, SIGKILL);
        int status = 0;
        while (::waitpid(m_pid, &status, 0) < 0 && errno == EINTR) {
        }
        m_pid = -1;
        VERIFY_NOT_REACHED();
    }

    ~NamedChild()
    {
        if (m_pid <= 0)
            return;
        ::kill(m_pid, SIGKILL);
        int status = 0;
        while (::waitpid(m_pid, &status, 0) < 0 && errno == EINTR) {
        }
    }

    pid_t pid() const { return m_pid; }

private:
    pid_t m_pid { -1 };
};

}

TEST_CASE(get_process_pid_matches_another_live_process_by_name)
{
    NamedChild child { "exile-ipc-test-child" };
    ScratchDirectory directory;
    directory.write_pid_file(ByteString::number(child.pid()));

    auto result = Core::IPCProcess::get_process_pid("exile-ipc-test-child"sv, directory.pid_file_path());
    EXPECT(!result.is_error());
    EXPECT(result.value().has_value());
    EXPECT_EQ(result.value().value(), child.pid());

    auto wrong = Core::IPCProcess::get_process_pid("exile-ipc-other-name"sv, directory.pid_file_path());
    EXPECT(!wrong.is_error());
    EXPECT(!wrong.value().has_value());
}
#    endif

#endif
