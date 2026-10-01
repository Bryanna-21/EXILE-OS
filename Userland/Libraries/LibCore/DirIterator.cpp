/*
 * Copyright (c) 2018-2020, Andreas Kling <kling@serenityos.org>
 * Copyright (c) 2023, Sam Atkins <atkinssj@serenityos.org>
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include <AK/Vector.h>
#include <AK/LexicalPath.h>
#include <LibCore/DirIterator.h>
#include <errno.h>
#include <fcntl.h>
#include <sys/stat.h>

namespace Core {

DirIterator::DirIterator(ByteString path, Flags flags)
    : m_path(move(path))
    , m_flags(flags)
{
#if defined(AK_OS_WINDOWS)
    StringBuilder search_path;
    search_path.append(m_path);
    if (!m_path.ends_with('/') && !m_path.ends_with('\\'))
        search_path.append('\\');
    search_path.append('*');

    auto search_path_string = search_path.to_byte_string();
    m_find_handle = ::FindFirstFileA(search_path_string.characters(), &m_find_data);
    if (m_find_handle == INVALID_HANDLE_VALUE) {
        m_error = Error::from_errno(EACCES);
    } else {
    }
#else
    m_dir = opendir(m_path.characters());
    if (!m_dir)
        m_error = Error::from_errno(errno);
#endif
}

DirIterator::~DirIterator()
{
#if defined(AK_OS_WINDOWS)
    if (m_find_handle != INVALID_HANDLE_VALUE) {
        ::FindClose(m_find_handle);
        m_find_handle = INVALID_HANDLE_VALUE;
    }
#else
    if (m_dir) {
        closedir(m_dir);
        m_dir = nullptr;
    }
#endif
}

DirIterator::DirIterator(DirIterator&& other)
#if defined(AK_OS_WINDOWS)
    : m_find_handle(other.m_find_handle)
    , m_find_data(other.m_find_data)
    , m_first_entry(other.m_first_entry)
#else
    : m_dir(other.m_dir)
#endif
    , m_error(move(other.m_error))
    , m_next(move(other.m_next))
    , m_path(move(other.m_path))
    , m_flags(other.m_flags)
{
#if defined(AK_OS_WINDOWS)
    other.m_find_handle = INVALID_HANDLE_VALUE;
#else
    other.m_dir = nullptr;
#endif
}

static constexpr bool dirent_has_d_type =
#if defined(AK_OS_WINDOWS) || defined(AK_OS_SOLARIS) || defined(AK_OS_HAIKU)
    false;
#else
    true;
#endif

bool DirIterator::advance_next()
{
#if defined(AK_OS_WINDOWS)
    if (m_find_handle == INVALID_HANDLE_VALUE)
        return false;

    while (true) {
        if (!m_first_entry) {
            if (!::FindNextFileA(m_find_handle, &m_find_data)) {
                ::FindClose(m_find_handle);
                m_find_handle = INVALID_HANDLE_VALUE;
                m_next.clear();
                return false;
            }
        }

        m_first_entry = false;

        auto name_length = strlen(m_find_data.cFileName);
        if (name_length == 0)
            return false;

        if (m_flags & Flags::SkipDots && m_find_data.cFileName[0] == '.')
            continue;

        if (m_flags & Flags::SkipParentAndBaseDir
            && ((strcmp(m_find_data.cFileName, ".") == 0)
                || (strcmp(m_find_data.cFileName, "..") == 0)))
            continue;

        auto name = StringView { m_find_data.cFileName, name_length };

        DirectoryEntry entry {};
        entry.name = ByteString { name };

        if (m_find_data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY)
            entry.type = DirectoryEntry::Type::Directory;
        else if (m_find_data.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT)
            entry.type = DirectoryEntry::Type::SymbolicLink;
        else
            entry.type = DirectoryEntry::Type::File;

        m_next = move(entry);
        return true;
    }
#else
    if (!m_dir)
        return false;

    while (true) {
        errno = 0;
        auto* de = readdir(m_dir);
        if (!de) {
            if (errno != 0) {
                m_error = Error::from_errno(errno);
                dbgln("DirIteration error: {}", m_error.value());
            }
            m_next.clear();
            return false;
        }

        if constexpr (dirent_has_d_type)
            m_next = DirectoryEntry::from_dirent(*de);
        else {
            StringBuilder path_builder;
            path_builder.append(m_path);
            if (!m_path.ends_with('/'))
                path_builder.append('/');
            path_builder.append(StringView { de->d_name, __builtin_strlen(de->d_name) });
            m_next = DirectoryEntry::from_stat(path_builder.string_view(), *de);
        }

        if (m_next->name.is_empty())
            return false;

        if (m_flags & Flags::SkipDots && m_next->name.starts_with('.'))
            continue;

        if (m_flags & Flags::SkipParentAndBaseDir && (m_next->name == "." || m_next->name == ".."))
            continue;

#if !defined(AK_OS_WINDOWS)
        if constexpr (dirent_has_d_type) {
            if ((m_flags & Flags::NoStat) == 0 && m_next->type == DirectoryEntry::Type::Unknown) {
                struct stat statbuf;
                if (fstatat(dirfd(m_dir), de->d_name, &statbuf, AT_SYMLINK_NOFOLLOW) < 0) {
                    m_error = Error::from_errno(errno);
                    dbgln("DirIteration error: {}", m_error.value());
                    return false;
                }
                m_next->type = DirectoryEntry::directory_entry_type_from_stat(statbuf.st_mode);
            }
        }
#endif

        return !m_next->name.is_empty();
    }
#endif
}

bool DirIterator::has_next()
{
    if (m_next.has_value())
        return true;

    return advance_next();
}

Optional<DirectoryEntry> DirIterator::next()
{
    if (!m_next.has_value())
        advance_next();

    auto result = m_next;
    m_next.clear();
    return result;
}

ByteString DirIterator::next_path()
{
    auto entry = next();
    if (entry.has_value())
        return entry->name;
    return "";
}

ByteString DirIterator::next_full_path()
{
    StringBuilder builder;
    builder.append(m_path);
    if (!m_path.ends_with('/'))
        builder.append('/');
    builder.append(next_path());
    return builder.to_byte_string();
}

int DirIterator::fd() const
{
#if defined(AK_OS_WINDOWS)
    return -1;
#else
    if (!m_dir)
        return -1;
    return dirfd(m_dir);
#endif
}

}
