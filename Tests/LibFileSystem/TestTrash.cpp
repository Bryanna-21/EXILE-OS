/*
 * Copyright (c) 2026, the Exile OS developers.
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include <AK/LexicalPath.h>
#include <LibCore/DirIterator.h>
#include <LibCore/File.h>
#include <LibCore/System.h>
#include <LibFileSystem/FileSystem.h>
#include <LibFileSystem/TempFile.h>
#include <LibFileSystem/Trash.h>
#include <LibTest/TestCase.h>
#include <unistd.h>

struct Sandbox {
    NonnullOwnPtr<FileSystem::TempFile> temp;
    ByteString work;  // where "user" files live
    ByteString trash; // trash root
};

static Sandbox make_sandbox()
{
    auto temp = MUST(FileSystem::TempFile::create_temp_directory());
    auto base = MUST(FileSystem::real_path(temp->path()));
    auto work = ByteString::formatted("{}/work", base);
    MUST(Core::System::mkdir(work, 0755));
    return { move(temp), work, ByteString::formatted("{}/trash", base) };
}

static void write_file(StringView path, StringView content)
{
    auto file = MUST(Core::File::open(path, Core::File::OpenMode::Write | Core::File::OpenMode::Truncate));
    MUST(file->write_until_depleted(content.bytes()));
}

static ByteString read_file(StringView path)
{
    auto file = MUST(Core::File::open(path, Core::File::OpenMode::Read));
    auto contents = MUST(file->read_until_eof());
    return ByteString(StringView(contents.bytes()));
}

TEST_CASE(trash_moves_file_and_records_metadata)
{
    auto sb = make_sandbox();
    auto trash = TRY_OR_FAIL(FileSystem::Trash::open(sb.trash));
    auto path = ByteString::formatted("{}/hello.txt", sb.work);
    write_file(path, "hi"sv);

    auto item = TRY_OR_FAIL(trash.trash(path));
    EXPECT(!FileSystem::exists(path));
    EXPECT_EQ(item.original_path, path);
    EXPECT(!item.is_directory);
    EXPECT(FileSystem::exists(ByteString::formatted("{}/{}", trash.files_directory(), item.trash_name)));

    auto items = TRY_OR_FAIL(trash.list());
    EXPECT_EQ(items.size(), 1u);
    EXPECT(items[0].has_valid_metadata);
    EXPECT_EQ(items[0].original_path, path);
    EXPECT(items[0].deletion_time > 0);
}

TEST_CASE(restore_returns_item_to_original_location)
{
    auto sb = make_sandbox();
    auto trash = TRY_OR_FAIL(FileSystem::Trash::open(sb.trash));
    auto path = ByteString::formatted("{}/a.txt", sb.work);
    write_file(path, "content"sv);
    auto item = TRY_OR_FAIL(trash.trash(path));

    auto restored = TRY_OR_FAIL(trash.restore(item.trash_name));
    EXPECT_EQ(restored, path);
    EXPECT_EQ(read_file(path), "content"sv);
    EXPECT_EQ(TRY_OR_FAIL(trash.list()).size(), 0u);
}

TEST_CASE(same_name_collisions_in_trash_keep_both_items)
{
    auto sb = make_sandbox();
    auto trash = TRY_OR_FAIL(FileSystem::Trash::open(sb.trash));
    auto path = ByteString::formatted("{}/dup.txt", sb.work);

    write_file(path, "first"sv);
    auto first = TRY_OR_FAIL(trash.trash(path));
    write_file(path, "second"sv);
    auto second = TRY_OR_FAIL(trash.trash(path));

    EXPECT(first.trash_name != second.trash_name);
    EXPECT_EQ(TRY_OR_FAIL(trash.list()).size(), 2u);
    EXPECT_EQ(read_file(ByteString::formatted("{}/{}", trash.files_directory(), first.trash_name)), "first"sv);
    EXPECT_EQ(read_file(ByteString::formatted("{}/{}", trash.files_directory(), second.trash_name)), "second"sv);
}

TEST_CASE(restore_never_overwrites_existing_file)
{
    auto sb = make_sandbox();
    auto trash = TRY_OR_FAIL(FileSystem::Trash::open(sb.trash));
    auto path = ByteString::formatted("{}/report.txt", sb.work);
    write_file(path, "old"sv);
    auto item = TRY_OR_FAIL(trash.trash(path));
    write_file(path, "new"sv); // something took its place

    auto restored = TRY_OR_FAIL(trash.restore(item.trash_name));
    EXPECT_EQ(restored, ByteString::formatted("{}/report (restored).txt", sb.work));
    EXPECT_EQ(read_file(path), "new"sv);
    EXPECT_EQ(read_file(restored), "old"sv);
}

TEST_CASE(nested_directories_round_trip)
{
    auto sb = make_sandbox();
    auto trash = TRY_OR_FAIL(FileSystem::Trash::open(sb.trash));
    auto dir = ByteString::formatted("{}/project", sb.work);
    MUST(Core::System::mkdir(dir, 0755));
    MUST(Core::System::mkdir(ByteString::formatted("{}/src", dir), 0755));
    MUST(Core::System::mkdir(ByteString::formatted("{}/src/deep", dir), 0755));
    write_file(ByteString::formatted("{}/src/deep/file.txt", dir), "deep"sv);
    write_file(ByteString::formatted("{}/top.txt", dir), "top"sv);

    auto item = TRY_OR_FAIL(trash.trash(dir));
    EXPECT(item.is_directory);
    EXPECT(!FileSystem::exists(dir));

    TRY_OR_FAIL(trash.restore(item.trash_name));
    EXPECT_EQ(read_file(ByteString::formatted("{}/src/deep/file.txt", dir)), "deep"sv);
    EXPECT_EQ(read_file(ByteString::formatted("{}/top.txt", dir)), "top"sv);
}

TEST_CASE(symlink_is_moved_not_followed)
{
    auto sb = make_sandbox();
    auto trash = TRY_OR_FAIL(FileSystem::Trash::open(sb.trash));
    auto target_dir = ByteString::formatted("{}/target", sb.work);
    MUST(Core::System::mkdir(target_dir, 0755));
    auto precious = ByteString::formatted("{}/precious.txt", target_dir);
    write_file(precious, "keep me"sv);
    auto link = ByteString::formatted("{}/link", sb.work);
    MUST(Core::System::symlink(target_dir, link));

    auto item = TRY_OR_FAIL(trash.trash(link));
    EXPECT(!item.is_directory); // it is a link, not a directory
    EXPECT(FileSystem::exists(precious));

    // Permanently deleting the trashed link must not touch what it pointed at.
    TRY_OR_FAIL(trash.delete_permanently(item.trash_name));
    EXPECT(FileSystem::exists(precious));
    EXPECT_EQ(read_file(precious), "keep me"sv);
}

TEST_CASE(delete_permanently_and_empty)
{
    auto sb = make_sandbox();
    auto trash = TRY_OR_FAIL(FileSystem::Trash::open(sb.trash));
    auto a = ByteString::formatted("{}/a", sb.work);
    auto b = ByteString::formatted("{}/b", sb.work);
    write_file(a, "a"sv);
    MUST(Core::System::mkdir(b, 0755));
    write_file(ByteString::formatted("{}/inner", b), "x"sv);

    auto item_a = TRY_OR_FAIL(trash.trash(a));
    TRY_OR_FAIL(trash.trash(b));
    TRY_OR_FAIL(trash.delete_permanently(item_a.trash_name));
    EXPECT_EQ(TRY_OR_FAIL(trash.list()).size(), 1u);
    EXPECT(!FileSystem::exists(ByteString::formatted("{}/{}.trashinfo", trash.info_directory(), item_a.trash_name)));

    EXPECT_EQ(TRY_OR_FAIL(trash.empty()), 1u);
    EXPECT_EQ(TRY_OR_FAIL(trash.list()).size(), 0u);
    EXPECT(trash.delete_permanently("missing"sv).is_error());
}

TEST_CASE(failed_trash_leaves_original_untouched_and_never_deletes)
{
    auto sb = make_sandbox();
    auto trash = TRY_OR_FAIL(FileSystem::Trash::open(sb.trash));

    // Nonexistent source
    EXPECT(trash.trash(ByteString::formatted("{}/nope", sb.work)).is_error());
    EXPECT_EQ(TRY_OR_FAIL(trash.list()).size(), 0u);
    // No leftover metadata from the failed attempt
    EXPECT_EQ(Core::DirIterator(trash.info_directory(), Core::DirIterator::SkipParentAndBaseDir).has_next(), false);

    // Relative / empty paths are refused
    EXPECT(trash.trash("relative.txt"sv).is_error());
    EXPECT(trash.trash(""sv).is_error());
}

TEST_CASE(protected_and_unsafe_paths_are_refused)
{
    auto sb = make_sandbox();
    auto trash = TRY_OR_FAIL(FileSystem::Trash::open(sb.trash));

    EXPECT(trash.trash("/"sv).is_error());
    EXPECT(trash.trash("/etc"sv).is_error());
    EXPECT(trash.trash("/usr"sv).is_error());

    // Trash cannot be put into itself, nor can a directory that contains it.
    EXPECT(trash.trash(sb.trash).is_error());
    EXPECT(trash.trash(trash.files_directory()).is_error());
    EXPECT(trash.trash(LexicalPath::dirname(sb.trash)).is_error());

    // Caller-specified protected path
    auto special = ByteString::formatted("{}/special", sb.work);
    write_file(special, "x"sv);
    trash.add_protected_path(special);
    EXPECT(trash.trash(special).is_error());
    EXPECT(FileSystem::exists(special));
}

TEST_CASE(invalid_or_missing_metadata_is_handled_safely)
{
    auto sb = make_sandbox();
    auto trash = TRY_OR_FAIL(FileSystem::Trash::open(sb.trash));
    auto path = ByteString::formatted("{}/victim.txt", sb.work);
    write_file(path, "v"sv);
    auto item = TRY_OR_FAIL(trash.trash(path));
    auto info_path = ByteString::formatted("{}/{}.trashinfo", trash.info_directory(), item.trash_name);

    // Tampered metadata pointing outside with ".." must not be restorable.
    write_file(info_path, "[Trash Info]\nPath=/tmp/../etc/passwd\nDeletionTimestamp=1\n"sv);
    auto items = TRY_OR_FAIL(trash.list());
    EXPECT_EQ(items.size(), 1u);
    EXPECT(!items[0].has_valid_metadata);
    EXPECT(trash.restore(item.trash_name).is_error());

    // Garbage metadata, then missing metadata: still listed, not restorable, but deletable.
    write_file(info_path, "garbage"sv);
    EXPECT(trash.restore(item.trash_name).is_error());
    MUST(Core::System::unlink(info_path));
    EXPECT(trash.restore(item.trash_name).is_error());
    EXPECT_EQ(TRY_OR_FAIL(trash.list()).size(), 1u);
    TRY_OR_FAIL(trash.delete_permanently(item.trash_name));
    EXPECT_EQ(TRY_OR_FAIL(trash.list()).size(), 0u);

    // Names that try to escape the trash directory are refused.
    EXPECT(trash.restore("../work"sv).is_error());
    EXPECT(trash.delete_permanently("../work"sv).is_error());
}

TEST_CASE(restore_fails_when_original_folder_is_gone)
{
    auto sb = make_sandbox();
    auto trash = TRY_OR_FAIL(FileSystem::Trash::open(sb.trash));
    auto folder = ByteString::formatted("{}/sub", sb.work);
    MUST(Core::System::mkdir(folder, 0755));
    auto path = ByteString::formatted("{}/f.txt", folder);
    write_file(path, "f"sv);
    auto item = TRY_OR_FAIL(trash.trash(path));
    MUST(Core::System::rmdir(folder));

    EXPECT(trash.restore(item.trash_name).is_error());
    // The item must still be in the trash after a failed restore.
    EXPECT_EQ(TRY_OR_FAIL(trash.list()).size(), 1u);
}

TEST_CASE(unusual_names_round_trip)
{
    auto sb = make_sandbox();
    auto trash = TRY_OR_FAIL(FileSystem::Trash::open(sb.trash));
    auto path = ByteString::formatted("{}/weird name %41 éè #1.txt", sb.work);
    write_file(path, "w"sv);
    auto item = TRY_OR_FAIL(trash.trash(path));
    auto items = TRY_OR_FAIL(trash.list());
    EXPECT_EQ(items[0].original_path, path);
    EXPECT_EQ(TRY_OR_FAIL(trash.restore(item.trash_name)), path);
}

TEST_CASE(percent_encoding)
{
    EXPECT_EQ(FileSystem::trash_percent_encode("/a b/%.txt"sv), "/a%20b/%25.txt"sv);
    EXPECT_EQ(TRY_OR_FAIL(FileSystem::trash_percent_decode("/a%20b/%25.txt"sv)), "/a b/%.txt"sv);
    EXPECT(FileSystem::trash_percent_decode("%4"sv).is_error());
    EXPECT(FileSystem::trash_percent_decode("%zz"sv).is_error());
}

TEST_CASE(remove_tree_does_not_follow_symlinks)
{
    auto sb = make_sandbox();
    auto outside = ByteString::formatted("{}/outside", sb.work);
    MUST(Core::System::mkdir(outside, 0755));
    write_file(ByteString::formatted("{}/keep.txt", outside), "keep"sv);
    auto tree = ByteString::formatted("{}/tree", sb.work);
    MUST(Core::System::mkdir(tree, 0755));
    MUST(Core::System::symlink(outside, ByteString::formatted("{}/escape", tree)));

    TRY_OR_FAIL(FileSystem::remove_tree_without_following_links(tree));
    EXPECT(!FileSystem::exists(tree));
    EXPECT_EQ(read_file(ByteString::formatted("{}/keep.txt", outside)), "keep"sv);
}
