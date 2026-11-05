/*
 * Copyright (c) 2026, the Exile OS developers.
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include <AK/LexicalPath.h>
#include <AK/StringBuilder.h>
#include <LibCore/DirIterator.h>
#include <LibCore/File.h>
#include <LibCore/StandardPaths.h>
#include <LibCore/System.h>
#include <LibFileSystem/FileSystem.h>
#include <LibFileSystem/Trash.h>
#include <fcntl.h>
#include <time.h>

namespace FileSystem {

static constexpr StringView info_suffix = ".trashinfo"sv;
static constexpr size_t max_name_length = 200;

ByteString trash_percent_encode(StringView input)
{
    StringBuilder builder;
    for (auto byte : input.bytes()) {
        bool unreserved = (byte >= 'a' && byte <= 'z') || (byte >= 'A' && byte <= 'Z') || (byte >= '0' && byte <= '9')
            || byte == '-' || byte == '_' || byte == '.' || byte == '~' || byte == '/';
        if (unreserved)
            builder.append(static_cast<char>(byte));
        else
            builder.appendff("%{:02X}", byte);
    }
    return builder.to_byte_string();
}

static int hex_value(char c)
{
    if (c >= '0' && c <= '9')
        return c - '0';
    if (c >= 'a' && c <= 'f')
        return c - 'a' + 10;
    if (c >= 'A' && c <= 'F')
        return c - 'A' + 10;
    return -1;
}

ErrorOr<ByteString> trash_percent_decode(StringView input)
{
    StringBuilder builder;
    for (size_t i = 0; i < input.length(); ++i) {
        char c = input[i];
        if (c != '%') {
            builder.append(c);
            continue;
        }
        if (i + 2 >= input.length())
            return Error::from_string_literal("Truncated percent escape in trash metadata");
        int hi = hex_value(input[i + 1]);
        int lo = hex_value(input[i + 2]);
        if (hi < 0 || lo < 0)
            return Error::from_string_literal("Invalid percent escape in trash metadata");
        builder.append(static_cast<char>(hi * 16 + lo));
        i += 2;
    }
    return builder.to_byte_string();
}

ErrorOr<void> remove_tree_without_following_links(StringView path)
{
    auto st = TRY(Core::System::lstat(path));
    if (!S_ISDIR(st.st_mode)) {
        // Regular files, symlinks (even to directories) and special files: just unlink the entry itself.
        return Core::System::unlink(path);
    }

    {
        Core::DirIterator iterator(path, Core::DirIterator::SkipParentAndBaseDir);
        if (iterator.has_error())
            return iterator.error();
        while (iterator.has_next())
            TRY(remove_tree_without_following_links(iterator.next_full_path()));
    }
    return Core::System::rmdir(path);
}

ErrorOr<Trash> Trash::open(ByteString root)
{
    if (root.is_empty() || !root.starts_with('/'))
        return Error::from_string_literal("Trash root must be an absolute path");

    root = LexicalPath::canonicalized_path(root);
    Trash trash { root };

    // Create root, files/ and info/ (private to the user). Existing directories are fine.
    for (auto const& dir : { root, trash.files_directory(), trash.info_directory() }) {
        auto result = Core::System::mkdir(dir, 0700);
        if (result.is_error() && result.error().code() != EEXIST)
            return result.release_error();
        if (!is_directory(dir))
            return Error::from_string_literal("Trash path exists but is not a directory");
    }
    return trash;
}

ErrorOr<Trash> Trash::open_default()
{
    auto data = Core::StandardPaths::data_directory();
    // The data directory itself may not exist yet.
    auto result = Core::System::mkdir(data, 0700);
    if (result.is_error() && result.error().code() != EEXIST)
        return result.release_error();
    return open(ByteString::formatted("{}/Trash", data));
}

ByteString Trash::files_directory() const { return ByteString::formatted("{}/files", m_root); }
ByteString Trash::info_directory() const { return ByteString::formatted("{}/info", m_root); }

void Trash::add_protected_path(ByteString path)
{
    m_protected_paths.append(LexicalPath::canonicalized_path(path));
}

ErrorOr<void> Trash::check_trashable(StringView path) const
{
    if (path.is_empty() || !path.starts_with('/'))
        return Error::from_string_literal("Only absolute paths can be moved to Trash");

    auto canonical = LexicalPath::canonicalized_path(path);
    if (canonical == "/")
        return Error::from_string_literal("The root directory cannot be moved to Trash");

    // Protect every top-level entry (/bin, /etc, /home, ...), the user's home directory, and anything the caller added.
    LexicalPath lexical { canonical };
    if (lexical.parts_view().size() <= 1)
        return Error::from_string_literal("System directories cannot be moved to Trash");
    if (canonical == LexicalPath::canonicalized_path(Core::StandardPaths::home_directory()))
        return Error::from_string_literal("The home directory cannot be moved to Trash");
    for (auto const& protected_path : m_protected_paths) {
        if (canonical == protected_path)
            return Error::from_string_literal("This item is protected and cannot be moved to Trash");
    }

    // Resolve the *parent* (not the final component, which may itself be a symlink we want to move as-is).
    auto parent_real = TRY(real_path(lexical.dirname()));
    auto resolved = ByteString::formatted("{}/{}", parent_real == "/" ? ""sv : parent_real.view(), lexical.basename());
    auto trash_real = TRY(real_path(m_root));

    if (resolved == trash_real || resolved.starts_with(ByteString::formatted("{}/", trash_real)))
        return Error::from_string_literal("Items already in Trash cannot be moved to Trash again");
    // Moving a directory that contains the trash itself would move the trash into itself.
    if (trash_real.starts_with(ByteString::formatted("{}/", resolved)))
        return Error::from_string_literal("This directory contains the Trash and cannot be moved into it");

    if (!can_delete_or_move(canonical))
        return Error::from_errno(EACCES);
    return {};
}

static ErrorOr<ByteString> serialize_info(StringView original_path, time_t deletion_time)
{
    struct tm tm_utc {};
    gmtime_r(&deletion_time, &tm_utc);
    char date[32];
    strftime(date, sizeof(date), "%Y-%m-%dT%H:%M:%S", &tm_utc);
    return ByteString::formatted("[Trash Info]\nPath={}\nDeletionDate={}\nDeletionTimestamp={}\n",
        trash_percent_encode(original_path), date, static_cast<i64>(deletion_time));
}

ErrorOr<TrashedItem> Trash::trash(StringView path)
{
    TRY(check_trashable(path));
    auto canonical = LexicalPath::canonicalized_path(path);
    auto st = TRY(Core::System::lstat(canonical)); // does not follow a final symlink
    auto basename = LexicalPath::basename(canonical);
    if (basename.is_empty() || basename == "."sv || basename == ".."sv)
        return Error::from_string_literal("Invalid file name");

    StringView base_for_name = basename;
    if (basename.length() > max_name_length)
        base_for_name = basename.substring_view(0, max_name_length);
    time_t now = time(nullptr);
    auto info_text = TRY(serialize_info(canonical, now));

    // Reserve a unique name. Creating the .trashinfo with MustBeNew is atomic, so two concurrent trash()
    // calls (or an existing item) can never share a name and nothing is ever overwritten.
    ByteString trash_name;
    ByteString info_path;
    bool reserved = false;
    for (unsigned attempt = 0; attempt < 10000 && !reserved; ++attempt) {
        auto candidate = attempt == 0 ? ByteString(base_for_name) : ByteString::formatted("{}.{}", base_for_name, attempt + 1);
        auto candidate_info = ByteString::formatted("{}/{}{}", info_directory(), candidate, info_suffix);
        auto candidate_file = ByteString::formatted("{}/{}", files_directory(), candidate);
        if (!Core::System::lstat(candidate_file).is_error())
            continue; // orphaned/foreign entry occupies the name
        auto file_or_error = Core::File::open(candidate_info, Core::File::OpenMode::Write | Core::File::OpenMode::MustBeNew, 0600);
        if (file_or_error.is_error()) {
            if (file_or_error.error().code() == EEXIST)
                continue;
            return file_or_error.release_error();
        }
        auto write_result = file_or_error.value()->write_until_depleted(info_text.bytes());
        if (write_result.is_error()) {
            (void)Core::System::unlink(candidate_info);
            return write_result.release_error();
        }
        trash_name = candidate;
        info_path = candidate_info;
        reserved = true;
    }
    if (!reserved)
        return Error::from_string_literal("Could not find a free name in Trash");

    auto destination = ByteString::formatted("{}/{}", files_directory(), trash_name);
    auto move_result = Core::System::rename(canonical, destination);
    if (move_result.is_error()) {
        (void)Core::System::unlink(info_path); // roll back the reservation; the original is untouched
        if (move_result.error().code() == EXDEV)
            return Error::from_string_literal("Cannot move to Trash across filesystems; the item was not deleted");
        return move_result.release_error();
    }

    return TrashedItem { trash_name, canonical, now, S_ISDIR(st.st_mode), true };
}

static ErrorOr<ByteString> read_whole_file(StringView path)
{
    auto file = TRY(Core::File::open(path, Core::File::OpenMode::Read));
    auto contents = TRY(file->read_until_eof(64 * 1024));
    return ByteString(StringView(contents.bytes()));
}

ErrorOr<TrashedItem> Trash::read_item(StringView trash_name) const
{
    if (trash_name.is_empty() || trash_name.contains('/') || trash_name == "."sv || trash_name == ".."sv)
        return Error::from_string_literal("Invalid Trash item name");

    TrashedItem item;
    item.trash_name = trash_name;
    auto file_path = ByteString::formatted("{}/{}", files_directory(), trash_name);
    auto st = TRY(Core::System::lstat(file_path));
    item.is_directory = S_ISDIR(st.st_mode);

    auto contents_or_error = read_whole_file(ByteString::formatted("{}/{}{}", info_directory(), trash_name, info_suffix));
    if (contents_or_error.is_error())
        return item; // no metadata: still listable and deletable, not restorable

    bool saw_header = false;
    ByteString path_value;
    time_t timestamp = 0;
    for (auto line : contents_or_error.value().view().split_view('\n')) {
        if (line == "[Trash Info]"sv) {
            saw_header = true;
        } else if (line.starts_with("Path="sv)) {
            auto decoded = trash_percent_decode(line.substring_view(5));
            if (!decoded.is_error())
                path_value = decoded.release_value();
        } else if (line.starts_with("DeletionTimestamp="sv)) {
            if (auto value = line.substring_view(18).to_number<i64>(); value.has_value())
                timestamp = static_cast<time_t>(*value);
        }
    }
    // A restorable path must be absolute and free of "." / ".." components.
    bool path_ok = saw_header && path_value.starts_with('/') && LexicalPath::canonicalized_path(path_value) == path_value
        && path_value != "/";
    if (path_ok) {
        item.original_path = path_value;
        item.deletion_time = timestamp;
        item.has_valid_metadata = true;
    }
    return item;
}

ErrorOr<Vector<TrashedItem>> Trash::list() const
{
    Vector<TrashedItem> items;
    Core::DirIterator iterator(files_directory(), Core::DirIterator::SkipParentAndBaseDir);
    if (iterator.has_error())
        return iterator.error();
    while (iterator.has_next()) {
        auto name = iterator.next_path();
        auto item_or_error = read_item(name);
        if (!item_or_error.is_error())
            items.append(item_or_error.release_value());
    }
    return items;
}

ErrorOr<ByteString> Trash::restore(StringView trash_name)
{
    auto item = TRY(read_item(trash_name));
    if (!item.has_valid_metadata)
        return Error::from_string_literal("This Trash item has missing or invalid metadata and cannot be restored");

    LexicalPath original { item.original_path };
    auto parent = original.dirname();
    if (!is_directory(parent))
        return Error::from_string_literal("The original folder no longer exists");

    // Never overwrite: pick the original name if free, otherwise "name (restored)", "name (restored 2)", ...
    ByteString target = item.original_path;
    for (unsigned attempt = 0; !Core::System::lstat(target).is_error(); ++attempt) {
        if (attempt > 10000)
            return Error::from_string_literal("Could not find a free name to restore to");
        auto suffix = attempt == 0 ? ByteString(" (restored)") : ByteString::formatted(" (restored {})", attempt + 1);
        auto title = original.title();
        auto extension = original.extension();
        target = ByteString::formatted("{}/{}{}{}", parent == "/"sv ? ""sv : parent, title, suffix,
            extension.is_empty() ? ByteString::empty() : ByteString::formatted(".{}", extension));
    }

    auto source = ByteString::formatted("{}/{}", files_directory(), trash_name);
    TRY(Core::System::rename(source, target));
    // The item is back; its metadata is no longer needed. A leftover info file would be harmless but misleading.
    (void)Core::System::unlink(ByteString::formatted("{}/{}{}", info_directory(), trash_name, info_suffix));
    return target;
}

ErrorOr<void> Trash::delete_permanently(StringView trash_name)
{
    if (trash_name.is_empty() || trash_name.contains('/') || trash_name == "."sv || trash_name == ".."sv)
        return Error::from_string_literal("Invalid Trash item name");
    auto file_path = ByteString::formatted("{}/{}", files_directory(), trash_name);
    TRY(Core::System::lstat(file_path)); // must exist
    TRY(remove_tree_without_following_links(file_path));
    auto info_path = ByteString::formatted("{}/{}{}", info_directory(), trash_name, info_suffix);
    if (auto result = Core::System::unlink(info_path); result.is_error() && result.error().code() != ENOENT)
        return result.release_error();
    return {};
}

ErrorOr<size_t> Trash::empty()
{
    auto items = TRY(list());
    size_t removed = 0;
    for (auto const& item : items) {
        TRY(delete_permanently(item.trash_name));
        ++removed;
    }
    return removed;
}

}
