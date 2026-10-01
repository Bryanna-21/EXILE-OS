/*
 * Copyright (c) 2021, Peter Elliott <pelliott@serenityos.org>
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include <LibCore/Directory.h>
#include <LibCore/LockFile.h>

#if defined(AK_OS_WINDOWS)
#    include <windows.h>
#else
#    include <errno.h>
#    include <fcntl.h>
#    include <sys/file.h>
#    include <unistd.h>
#endif

namespace Core {

LockFile::LockFile(char const* filename, Type type)
    : m_filename(filename)
{
    if (Core::Directory::create(LexicalPath(m_filename).parent(), Core::Directory::CreateDirectories::Yes).is_error())
        return;

#if defined(AK_OS_WINDOWS)
    HANDLE handle = ::CreateFileA(
        filename,
        GENERIC_READ | GENERIC_WRITE,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        nullptr,
        OPEN_ALWAYS,
        FILE_ATTRIBUTE_NORMAL,
        nullptr);

    if (handle == INVALID_HANDLE_VALUE) {
        m_errno = EACCES;
        return;
    }

    OVERLAPPED overlapped {};
    DWORD flags = LOCKFILE_FAIL_IMMEDIATELY;

    // Windows LockFileEx does not have a shared-lock equivalent.
    // LockFile currently has no callers requiring Type::Shared in
    // the Windows build, so both modes use a non-blocking file lock.
    if (type == Type::Exclusive)
        flags |= LOCKFILE_EXCLUSIVE_LOCK;

    if (!::LockFileEx(
            handle,
            flags,
            0,
            1,
            0,
            &overlapped)) {
        m_errno = EACCES;
        ::CloseHandle(handle);
        return;
    }

    m_handle = handle;
#else
    if (Core::Directory::create(LexicalPath(m_filename).parent(), Core::Directory::CreateDirectories::Yes).is_error())
        return;

    m_fd = open(filename, O_RDONLY | O_CREAT | O_CLOEXEC, 0666);
    if (m_fd == -1) {
        m_errno = errno;
        return;
    }

    if (flock(m_fd, LOCK_NB | ((type == Type::Exclusive) ? LOCK_EX : LOCK_SH)) == -1) {
        m_errno = errno;
        close(m_fd);
        m_fd = -1;
    }
#endif
}

LockFile::~LockFile()
{
    release();
}

bool LockFile::is_held() const
{
#if defined(AK_OS_WINDOWS)
    return m_handle != INVALID_HANDLE_VALUE;
#else
    return m_fd != -1;
#endif
}

void LockFile::release()
{
#if defined(AK_OS_WINDOWS)
    if (m_handle == INVALID_HANDLE_VALUE)
        return;

    OVERLAPPED overlapped {};
    ::UnlockFileEx(m_handle, 0, 1, 0, &overlapped);
    ::CloseHandle(m_handle);
    ::DeleteFileA(m_filename);

    m_handle = INVALID_HANDLE_VALUE;
#else
    if (m_fd == -1)
        return;

    unlink(m_filename);
    flock(m_fd, LOCK_NB | LOCK_UN);
    close(m_fd);

    m_fd = -1;
#endif
}

}
