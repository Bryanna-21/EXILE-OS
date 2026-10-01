/*
 * Copyright (c) 2018-2020, Andreas Kling <kling@serenityos.org>
 * Copyright (c) 2023, Sam Atkins <atkinssj@serenityos.org>
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#pragma once

#include <AK/ByteString.h>
#include <LibCore/DirectoryEntry.h>
#include <dirent.h>
#include <string.h>

#if defined(AK_OS_WINDOWS)
#    include <winsock2.h>
#    include <ws2tcpip.h>
#    include <windows.h>
#endif

namespace Core {

class DirIterator {
public:
    enum Flags {
        NoFlags = 0x0,
        SkipDots = 0x1,
        SkipParentAndBaseDir = 0x2,
        NoStat = 0x4,
    };

    explicit DirIterator(ByteString path, Flags = Flags::NoFlags);
    ~DirIterator();

    DirIterator(DirIterator&&);
    DirIterator(DirIterator const&) = delete;

    bool has_error() const { return m_error.has_value(); }
    Error error() const { return Error::copy(m_error.value()); }
    bool has_next();
    Optional<DirectoryEntry> next();
    ByteString next_path();
    ByteString next_full_path();
    int fd() const;

private:
#if defined(AK_OS_WINDOWS)
    HANDLE m_find_handle = INVALID_HANDLE_VALUE;
    WIN32_FIND_DATAA m_find_data {};
    bool m_first_entry = true;
#else
    DIR* m_dir = nullptr;
#endif
    Optional<Error> m_error;
    Optional<DirectoryEntry> m_next;
    ByteString m_path;
    int m_flags;

    bool advance_next();
};

}
