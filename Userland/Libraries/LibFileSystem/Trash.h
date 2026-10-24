/*
 * Copyright (c) 2026, the Exile OS developers.
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#pragma once

#include <AK/ByteString.h>
#include <AK/Error.h>
#include <AK/Vector.h>
#include <sys/types.h>

namespace FileSystem {

// A real Trash: deleted items are *moved* into <root>/files and described by <root>/info/<name>.trashinfo
// (the freedesktop.org Trash layout), so they can be listed, restored, or permanently removed later.
// Nothing here ever falls back to permanent deletion: if an item cannot be moved, trash() fails.
struct TrashedItem {
    ByteString trash_name;       // Name inside <root>/files (unique). Pass this to restore()/delete_permanently().
    ByteString original_path;    // Absolute path the item came from. Empty if the metadata is missing or invalid.
    time_t deletion_time { 0 };  // Unix time of deletion; 0 if unknown.
    bool is_directory { false };
    bool has_valid_metadata { false };
};

class Trash {
public:
    // Opens (creating if needed) a trash rooted at `root`, e.g. <data directory>/Trash.
    static ErrorOr<Trash> open(ByteString root);
    // The per-user trash: <StandardPaths::data_directory()>/Trash.
    static ErrorOr<Trash> open_default();

    ByteString const& root() const { return m_root; }
    ByteString files_directory() const;
    ByteString info_directory() const;

    // Absolute paths that trash() must refuse to move (in addition to the built-in protections).
    void add_protected_path(ByteString);

    // Moves `path` (a file, directory tree or symlink - links are moved, never followed) into the trash.
    ErrorOr<TrashedItem> trash(StringView path);

    ErrorOr<Vector<TrashedItem>> list() const;

    // Moves the item back to its original location. If something already exists there, the item is restored
    // next to it under a new name ("name (restored)", "name (restored 2)", ...). Fails if the original parent
    // directory no longer exists or the metadata is invalid. Returns the path it was restored to.
    ErrorOr<ByteString> restore(StringView trash_name);

    // Permanently removes one item. Symlinks are unlinked, never followed.
    ErrorOr<void> delete_permanently(StringView trash_name);
    // Permanently removes every item. Returns how many items were removed.
    ErrorOr<size_t> empty();

private:
    explicit Trash(ByteString root)
        : m_root(move(root))
    {
    }

    ErrorOr<void> check_trashable(StringView absolute_path) const;
    ErrorOr<TrashedItem> read_item(StringView trash_name) const;

    ByteString m_root;
    Vector<ByteString> m_protected_paths;
};

// Exposed for tests: the encoding used for the Path= key of a .trashinfo file.
ByteString trash_percent_encode(StringView);
ErrorOr<ByteString> trash_percent_decode(StringView);

// Removes `path` permanently without ever following symlinks (a symlink is unlinked, not traversed).
ErrorOr<void> remove_tree_without_following_links(StringView path);

}
