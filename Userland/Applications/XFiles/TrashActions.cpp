/*
 * Copyright (c) 2026, the Exile OS developers.
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include "TrashActions.h"
#include <AK/LexicalPath.h>
#include <AK/StringBuilder.h>
#include <LibFileSystem/Trash.h>
#include <LibGUI/MessageBox.h>

namespace FileManager {

static ErrorOr<FileSystem::Trash> open_trash() { return FileSystem::Trash::open_default(); }

static void show_trash_unavailable(GUI::Window* window, Error const& error)
{
    GUI::MessageBox::show_error(window, ByteString::formatted("Trash is not available: {}\n\nNothing was deleted.", error));
}

// Maps "<trash>/files/<name>" back to the trash item name; returns an empty string for anything else.
static ByteString trash_name_for_path(FileSystem::Trash const& trash, ByteString const& path)
{
    LexicalPath lexical { path };
    if (lexical.dirname() != trash.files_directory().view())
        return {};
    return lexical.basename();
}

static void report_failures(GUI::Window* window, StringView action, Vector<ByteString> const& failures)
{
    if (failures.is_empty())
        return;
    StringBuilder builder;
    builder.appendff("{} failed for {} item(s):\n", action, failures.size());
    for (size_t i = 0; i < failures.size() && i < 10; ++i)
        builder.appendff("\n{}", failures[i]);
    GUI::MessageBox::show_error(window, builder.string_view());
}

ByteString trash_files_directory()
{
    auto trash = open_trash();
    if (trash.is_error())
        return {};
    return trash.value().files_directory();
}

void move_paths_to_trash(Vector<ByteString> const& paths, bool should_confirm, GUI::Window* window)
{
    if (paths.is_empty())
        return;

    if (should_confirm) {
        ByteString message = paths.size() == 1
            ? ByteString::formatted("Move \"{}\" to Trash?", LexicalPath::basename(paths[0]))
            : ByteString::formatted("Move {} items to Trash?", paths.size());
        auto result = GUI::MessageBox::show(window, message, "Move to Trash"sv, GUI::MessageBox::Type::Warning, GUI::MessageBox::InputType::OKCancel);
        if (result == GUI::MessageBox::ExecResult::Cancel)
            return;
    }

    auto trash = open_trash();
    if (trash.is_error()) {
        show_trash_unavailable(window, trash.error());
        return;
    }

    Vector<ByteString> failures;
    for (auto const& path : paths) {
        auto result = trash.value().trash(path);
        if (result.is_error())
            failures.append(ByteString::formatted("{}: {}", LexicalPath::basename(path), result.error()));
    }
    report_failures(window, "Moving to Trash"sv, failures);
}

void restore_paths_from_trash(Vector<ByteString> const& paths, GUI::Window* window)
{
    auto trash = open_trash();
    if (trash.is_error()) {
        show_trash_unavailable(window, trash.error());
        return;
    }

    Vector<ByteString> failures;
    for (auto const& path : paths) {
        auto name = trash_name_for_path(trash.value(), path);
        if (name.is_empty()) {
            failures.append(ByteString::formatted("{}: not an item in Trash", LexicalPath::basename(path)));
            continue;
        }
        auto result = trash.value().restore(name);
        if (result.is_error())
            failures.append(ByteString::formatted("{}: {}", name, result.error()));
    }
    report_failures(window, "Restore"sv, failures);
}

void delete_paths_from_trash_permanently(Vector<ByteString> const& paths, GUI::Window* window)
{
    if (paths.is_empty())
        return;
    ByteString message = paths.size() == 1
        ? ByteString::formatted("Permanently delete \"{}\"?\n\nThis cannot be undone.", LexicalPath::basename(paths[0]))
        : ByteString::formatted("Permanently delete {} items?\n\nThis cannot be undone.", paths.size());
    if (GUI::MessageBox::show(window, message, "Delete Permanently"sv, GUI::MessageBox::Type::Warning, GUI::MessageBox::InputType::OKCancel) == GUI::MessageBox::ExecResult::Cancel)
        return;

    auto trash = open_trash();
    if (trash.is_error()) {
        show_trash_unavailable(window, trash.error());
        return;
    }

    Vector<ByteString> failures;
    for (auto const& path : paths) {
        auto name = trash_name_for_path(trash.value(), path);
        if (name.is_empty()) {
            failures.append(ByteString::formatted("{}: not an item in Trash", LexicalPath::basename(path)));
            continue;
        }
        auto result = trash.value().delete_permanently(name);
        if (result.is_error())
            failures.append(ByteString::formatted("{}: {}", name, result.error()));
    }
    report_failures(window, "Permanent deletion"sv, failures);
}

void empty_trash(GUI::Window* window)
{
    auto trash = open_trash();
    if (trash.is_error()) {
        show_trash_unavailable(window, trash.error());
        return;
    }
    auto items = trash.value().list();
    if (items.is_error()) {
        GUI::MessageBox::show_error(window, ByteString::formatted("Could not read Trash: {}", items.error()));
        return;
    }
    if (items.value().is_empty()) {
        GUI::MessageBox::show(window, "Trash is already empty."sv, "Empty Trash"sv, GUI::MessageBox::Type::Information);
        return;
    }
    auto message = ByteString::formatted("Permanently delete all {} item(s) in Trash?\n\nThis cannot be undone.", items.value().size());
    if (GUI::MessageBox::show(window, message, "Empty Trash"sv, GUI::MessageBox::Type::Warning, GUI::MessageBox::InputType::OKCancel) == GUI::MessageBox::ExecResult::Cancel)
        return;
    auto result = trash.value().empty();
    if (result.is_error())
        GUI::MessageBox::show_error(window, ByteString::formatted("Emptying Trash stopped early: {}", result.error()));
}

}
