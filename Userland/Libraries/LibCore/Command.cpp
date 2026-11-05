/*
 * Copyright (c) 2020, Itamar S. <itamar8910@gmail.com>
 * Copyright (c) 2022, David Tuin <davidot@serenityos.org>
 * Copyright (c) 2023, Shannon Booth <shannon@serenityos.org>
 * Copyright (c) 2024, the SerenityOS developers.
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include "Command.h"
#include <AK/Format.h>
#include <AK/ScopeGuard.h>
#include <LibCore/Environment.h>
#include <LibCore/File.h>
#include <LibCore/System.h>
#include <fcntl.h>
#include <stdio.h>
#include <AK/StringBuilder.h>
#if defined(AK_OS_WINDOWS)
#    include <windows.h>
#    include <io.h>
#else
#    include <sys/wait.h>
#    include <unistd.h>
#endif

namespace Core {

ErrorOr<NonnullOwnPtr<Command>> Command::create(StringView command, char const* const arguments[])
{
#if defined(AK_OS_WINDOWS)
    HANDLE child_stdin_read = nullptr;
    HANDLE parent_stdin_write = nullptr;
    HANDLE parent_stdout_read = nullptr;
    HANDLE child_stdout_write = nullptr;
    HANDLE parent_stderr_read = nullptr;
    HANDLE child_stderr_write = nullptr;

    SECURITY_ATTRIBUTES security_attributes {};
    security_attributes.nLength = sizeof(SECURITY_ATTRIBUTES);
    security_attributes.bInheritHandle = TRUE;

    if (!::CreatePipe(&child_stdin_read, &parent_stdin_write, &security_attributes, 0))
        return Error::from_errno(EIO);

    if (!::CreatePipe(&parent_stdout_read, &child_stdout_write, &security_attributes, 0)) {
        ::CloseHandle(child_stdin_read);
        ::CloseHandle(parent_stdin_write);
        return Error::from_errno(EIO);
    }

    if (!::CreatePipe(&parent_stderr_read, &child_stderr_write, &security_attributes, 0)) {
        ::CloseHandle(child_stdin_read);
        ::CloseHandle(parent_stdin_write);
        ::CloseHandle(parent_stdout_read);
        ::CloseHandle(child_stdout_write);
        return Error::from_errno(EIO);
    }

    ArmedScopeGuard close_all_handles { [&] {
        if (child_stdin_read)
            ::CloseHandle(child_stdin_read);
        if (parent_stdin_write)
            ::CloseHandle(parent_stdin_write);
        if (parent_stdout_read)
            ::CloseHandle(parent_stdout_read);
        if (child_stdout_write)
            ::CloseHandle(child_stdout_write);
        if (parent_stderr_read)
            ::CloseHandle(parent_stderr_read);
        if (child_stderr_write)
            ::CloseHandle(child_stderr_write);
    }};

    ::SetHandleInformation(parent_stdin_write, HANDLE_FLAG_INHERIT, 0);
    ::SetHandleInformation(parent_stdout_read, HANDLE_FLAG_INHERIT, 0);
    ::SetHandleInformation(parent_stderr_read, HANDLE_FLAG_INHERIT, 0);

    ByteString command_line;
    {
        StringBuilder builder;
        builder.append(command);

        for (size_t i = 1; arguments[i]; ++i) {
            builder.append(' ');
            builder.append('"');
            builder.append(StringView(arguments[i], __builtin_strlen(arguments[i])));
            builder.append('"');
        }

        command_line = builder.to_byte_string();
    }

    STARTUPINFOA startup_info {};
    startup_info.cb = sizeof(STARTUPINFOA);
    startup_info.dwFlags = STARTF_USESTDHANDLES;
    startup_info.hStdInput = child_stdin_read;
    startup_info.hStdOutput = child_stdout_write;
    startup_info.hStdError = child_stderr_write;

    PROCESS_INFORMATION process_info {};

    Vector<char> mutable_command_line;
    mutable_command_line.ensure_capacity(command_line.length() + 1);
    for (auto ch : command_line)
        mutable_command_line.append(ch);
    mutable_command_line.append('\0');

    if (!::CreateProcessA(
            nullptr,
            mutable_command_line.data(),
            nullptr,
            nullptr,
            TRUE,
            0,
            nullptr,
            nullptr,
            &startup_info,
            &process_info)) {
        return Error::from_errno(EIO);
    }

    ::CloseHandle(process_info.hThread);
    process_info.hThread = nullptr;

    ::CloseHandle(child_stdin_read);
    child_stdin_read = nullptr;
    ::CloseHandle(child_stdout_write);
    child_stdout_write = nullptr;
    ::CloseHandle(child_stderr_write);
    child_stderr_write = nullptr;

    auto stdin_fd = ::_open_osfhandle(reinterpret_cast<intptr_t>(parent_stdin_write), _O_WRONLY | _O_BINARY);
    if (stdin_fd < 0) {
        ::TerminateProcess(process_info.hProcess, 1);
        ::CloseHandle(process_info.hProcess);
        return Error::from_errno(errno);
    }
    parent_stdin_write = nullptr;

    auto stdout_fd = ::_open_osfhandle(reinterpret_cast<intptr_t>(parent_stdout_read), _O_RDONLY | _O_BINARY);
    if (stdout_fd < 0) {
        ::_close(stdin_fd);
        ::TerminateProcess(process_info.hProcess, 1);
        ::CloseHandle(process_info.hProcess);
        return Error::from_errno(errno);
    }
    parent_stdout_read = nullptr;

    auto stderr_fd = ::_open_osfhandle(reinterpret_cast<intptr_t>(parent_stderr_read), _O_RDONLY | _O_BINARY);
    if (stderr_fd < 0) {
        ::_close(stdin_fd);
        ::_close(stdout_fd);
        ::TerminateProcess(process_info.hProcess, 1);
        ::CloseHandle(process_info.hProcess);
        return Error::from_errno(errno);
    }
    parent_stderr_read = nullptr;

    auto stdin_file = TRY(Core::File::adopt_fd(stdin_fd, Core::File::OpenMode::Write));
    auto stdout_file = TRY(Core::File::adopt_fd(stdout_fd, Core::File::OpenMode::Read));
    auto stderr_file = TRY(Core::File::adopt_fd(stderr_fd, Core::File::OpenMode::Read));

    close_all_handles.disarm();

    return adopt_nonnull_own_or_enomem(new (nothrow) Command(
        process_info.hProcess,
        move(stdin_file),
        move(stdout_file),
        move(stderr_file)));
#else
    // FIXME: Close pipes in every branch, probably with something nicer than 6 (Armed)ScopeGuards
    //        (maybe introduce some new variant/api of Core::File that doesn't allocate, just returns RAII owner?).
    auto stdin_fds = TRY(Core::System::pipe2(O_CLOEXEC));
    auto stdout_fds = TRY(Core::System::pipe2(O_CLOEXEC));
    auto stderr_fds = TRY(Core::System::pipe2(O_CLOEXEC));

    posix_spawn_file_actions_t file_actions;
    posix_spawn_file_actions_init(&file_actions);
    posix_spawn_file_actions_adddup2(&file_actions, stdin_fds[0], STDIN_FILENO);
    posix_spawn_file_actions_adddup2(&file_actions, stdout_fds[1], STDOUT_FILENO);
    posix_spawn_file_actions_adddup2(&file_actions, stderr_fds[1], STDERR_FILENO);

    ScopeGuard destroy_file_actions { [&file_actions] { posix_spawn_file_actions_destroy(&file_actions); } };

    auto pid = TRY(Core::System::posix_spawnp(command, &file_actions, nullptr, const_cast<char**>(arguments), Core::Environment::raw_environ()));

    ArmedScopeGuard runner_kill { [&pid] { kill(pid, SIGKILL); } };

    TRY(Core::System::close(stdin_fds[0]));
    TRY(Core::System::close(stdout_fds[1]));
    TRY(Core::System::close(stderr_fds[1]));

    auto stdin_file = TRY(Core::File::adopt_fd(stdin_fds[1], Core::File::OpenMode::Write));
    auto stdout_file = TRY(Core::File::adopt_fd(stdout_fds[0], Core::File::OpenMode::Read));
    auto stderr_file = TRY(Core::File::adopt_fd(stderr_fds[0], Core::File::OpenMode::Read));

    runner_kill.disarm();

    return adopt_nonnull_own_or_enomem(new (nothrow) Command(pid, move(stdin_file), move(stdout_file), move(stderr_file)));
#endif
}

#if defined(AK_OS_WINDOWS)
Command::Command(void* process_handle, NonnullOwnPtr<Core::File> stdin_file, NonnullOwnPtr<Core::File> stdout_file, NonnullOwnPtr<Core::File> stderr_file)
    : m_process_handle(process_handle)
    , m_stdin(move(stdin_file))
    , m_stdout(move(stdout_file))
    , m_stderr(move(stderr_file))
{
}
#else
Command::Command(pid_t pid, NonnullOwnPtr<Core::File> stdin_file, NonnullOwnPtr<Core::File> stdout_file, NonnullOwnPtr<Core::File> stderr_file)
    : m_pid(pid)
    , m_stdin(move(stdin_file))
    , m_stdout(move(stdout_file))
    , m_stderr(move(stderr_file))
{
}
#endif

Command::~Command()
{
#if defined(AK_OS_WINDOWS)
    if (m_process_handle)
        ::CloseHandle(static_cast<HANDLE>(m_process_handle));
#endif
}

ErrorOr<void> Command::write(StringView input)
{
    TRY(m_stdin->write_until_depleted(input.bytes()));
    m_stdin->close();
    return {};
}

ErrorOr<void> Command::write_lines(Span<ByteString> lines)
{
    // It's possible the process dies before we can write everything to the
    // stdin. So make sure that we don't crash but just stop writing.

#if !defined(AK_OS_WINDOWS)
    struct sigaction action_handler {};
    action_handler.sa_handler = SIG_IGN;

    struct sigaction old_action_handler;
    TRY(Core::System::sigaction(SIGPIPE, &action_handler, &old_action_handler));

    auto close_stdin = ScopeGuard([this, &old_action_handler] {
        // Ensure that the input stream ends here, whether we were able to write all lines or not
        m_stdin->close();

        // It's not really a problem if this signal failed
        if (sigaction(SIGPIPE, &old_action_handler, nullptr) < 0)
            perror("sigaction");
    });
#else
    auto close_stdin = ScopeGuard([this] {
        // Windows reports a broken pipe as a write error instead of delivering
        // POSIX SIGPIPE.
        m_stdin->close();
    });
#endif

    for (ByteString const& line : lines) {
        auto result = m_stdin->write_until_depleted(ByteString::formatted("{}\n", line));

#if defined(AK_OS_WINDOWS)
        if (result.is_error())
            break;
        TRY(result);
#else
        TRY(result);
#endif
    }

    return {};
}

ErrorOr<Command::ProcessOutputs> Command::read_all()
{
    return ProcessOutputs { TRY(m_stdout->read_until_eof()), TRY(m_stderr->read_until_eof()) };
}

ErrorOr<Command::ProcessResult> Command::status(int options)
{
#if defined(AK_OS_WINDOWS)
    if (!m_process_handle)
        return ProcessResult::Unknown;

    m_stdin->close();

    auto process_handle = static_cast<HANDLE>(m_process_handle);

    // Command currently uses a zero/non-zero options distinction here:
    // blocking wait for normal status(), non-blocking check for a non-zero
    // options value.
    DWORD wait_timeout = options == 0 ? INFINITE : 0;
    DWORD wait_result = ::WaitForSingleObject(process_handle, wait_timeout);

    if (wait_result == WAIT_TIMEOUT)
        return ProcessResult::Running;

    if (wait_result != WAIT_OBJECT_0)
        return ProcessResult::Unknown;

    DWORD exit_code = 0;
    if (!::GetExitCodeProcess(process_handle, &exit_code))
        return ProcessResult::Unknown;

    ::CloseHandle(process_handle);
    m_process_handle = nullptr;

    if (exit_code == 0)
        return ProcessResult::DoneWithZeroExitCode;

    return ProcessResult::Failed;
#else
    if (m_pid == -1)
        return ProcessResult::Unknown;

    m_stdin->close();

    auto wait_result = TRY(Core::System::waitpid(m_pid, options));
    if (wait_result.pid == 0) {
        // Attempt to kill it, since it has not finished yet somehow
        return ProcessResult::Running;
    }
    m_pid = -1;

    if (WIFSIGNALED(wait_result.status) && WTERMSIG(wait_result.status) == SIGALRM)
        return ProcessResult::FailedFromTimeout;

    if (WIFEXITED(wait_result.status) && WEXITSTATUS(wait_result.status) == 0)
        return ProcessResult::DoneWithZeroExitCode;

    return ProcessResult::Failed;
#endif
}

// Only supported in serenity mode because we use `posix_spawn_file_actions_addchdir`
#ifdef AK_OS_SERENITY

ErrorOr<CommandResult> command(ByteString const& command_string, Optional<LexicalPath> chdir)
{
    auto parts = command_string.split(' ');
    if (parts.is_empty())
        return Error::from_string_literal("empty command");
    auto program = parts[0];
    parts.remove(0);
    return command(program, parts, chdir);
}

ErrorOr<CommandResult> command(ByteString const& program, Vector<ByteString> const& arguments, Optional<LexicalPath> chdir)
{
    int stdout_pipe[2] = {};
    int stderr_pipe[2] = {};
    if (pipe2(stdout_pipe, O_CLOEXEC)) {
        return Error::from_errno(errno);
    }
    if (pipe2(stderr_pipe, O_CLOEXEC)) {
        perror("pipe2");
        return Error::from_errno(errno);
    }

    auto close_pipes = ScopeGuard([stderr_pipe, stdout_pipe] {
        // The write-ends of these pipes are closed manually
        close(stdout_pipe[0]);
        close(stderr_pipe[0]);
    });

    Vector<char const*> parts = { program.characters() };
    for (auto const& part : arguments) {
        parts.append(part.characters());
    }
    parts.append(nullptr);

    char const** argv = parts.data();

    posix_spawn_file_actions_t action;
    posix_spawn_file_actions_init(&action);
    if (chdir.has_value()) {
        posix_spawn_file_actions_addchdir(&action, chdir.value().string().characters());
    }
    posix_spawn_file_actions_adddup2(&action, stdout_pipe[1], STDOUT_FILENO);
    posix_spawn_file_actions_adddup2(&action, stderr_pipe[1], STDERR_FILENO);

    pid_t pid;
    if ((errno = posix_spawnp(&pid, program.characters(), &action, nullptr, const_cast<char**>(argv), environ))) {
        perror("posix_spawn");
        VERIFY_NOT_REACHED();
    }

    // close the write-ends so reading wouldn't block
    close(stdout_pipe[1]);
    close(stderr_pipe[1]);

    auto read_all_from_pipe = [](int pipe[2]) -> ErrorOr<ByteBuffer> {
        auto result_file_or_error = Core::File::adopt_fd(pipe[0], Core::File::OpenMode::Read, Core::File::ShouldCloseFileDescriptor::Yes);
        auto result_file = TRY(result_file_or_error);
        return result_file->read_until_eof();
    };
    auto output = TRY(read_all_from_pipe(stdout_pipe));
    auto error = TRY(read_all_from_pipe(stderr_pipe));

    int wstatus { 0 };
    waitpid(pid, &wstatus, 0);
    posix_spawn_file_actions_destroy(&action);
    int exit_code = WEXITSTATUS(wstatus);

    if (exit_code != 0) {
#    ifdef DBG_FAILED_COMMANDS
        dbgln("command failed. stderr: {}", );
#    endif
    }

    return CommandResult { WEXITSTATUS(wstatus), output, error };
}

#endif

}
