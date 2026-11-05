/*
 * Copyright (c) 2026, the Exile OS developers.
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#pragma once

#include <AK/ByteString.h>
#include <AK/Vector.h>
#include <LibGUI/Forward.h>

namespace FileManager {

// All functions show their own confirmation / error dialogs. None of them fall back to permanent deletion
// unless the user explicitly chose a permanent-delete action.

// Directory that holds trashed items ("<data dir>/Trash/files"), or an empty string if Trash is unavailable.
ByteString trash_files_directory();

void move_paths_to_trash(Vector<ByteString> const& paths, bool should_confirm, GUI::Window*);
void restore_paths_from_trash(Vector<ByteString> const& paths_in_trash, GUI::Window*);
void delete_paths_from_trash_permanently(Vector<ByteString> const& paths_in_trash, GUI::Window*);
void empty_trash(GUI::Window*);

}
