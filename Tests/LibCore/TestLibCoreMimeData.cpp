/*
 * Copyright (c) 2026, the Exile OS developers.
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include <AK/StringView.h>
#include <LibCore/MimeData.h>
#include <LibTest/TestCase.h>

TEST_CASE(extension_matching_is_case_insensitive)
{
    EXPECT_EQ(Core::guess_mime_type_based_on_filename("report.PDF"sv), "application/pdf"sv);
    EXPECT_EQ(Core::guess_mime_type_based_on_filename("photo.JpEg"sv), "image/jpeg"sv);
    EXPECT_EQ(Core::guess_mime_type_based_on_filename("archive.7Z"sv), "application/x-7z-compressed"sv);
    EXPECT_EQ(Core::guess_mime_type_based_on_filename("font.TTF"sv), "font/ttf"sv);
    EXPECT_EQ(Core::guess_mime_type_based_on_filename("font.WOFF2"sv), "font/woff2"sv);
}

TEST_CASE(plaintext_special_names)
{
    EXPECT_EQ(Core::guess_mime_type_based_on_filename("CMakeLists.txt"sv), "text/plain"sv);
    EXPECT_EQ(Core::guess_mime_type_based_on_filename("src/CMakeLists.txt"sv), "text/plain"sv);
    EXPECT_EQ(Core::guess_mime_type_based_on_filename(".history"sv), "text/plain"sv);
    EXPECT_EQ(Core::guess_mime_type_based_on_filename("home/.history"sv), "text/plain"sv);
    EXPECT_EQ(Core::guess_mime_type_based_on_filename(".shellrc"sv), "text/plain"sv);
    EXPECT_EQ(Core::guess_mime_type_based_on_filename("home/.shellrc"sv), "text/plain"sv);
}

TEST_CASE(special_names_do_not_match_as_suffixes)
{
    EXPECT_EQ(Core::guess_mime_type_based_on_filename("project.history"sv), "application/octet-stream"sv);
    EXPECT_EQ(Core::guess_mime_type_based_on_filename("project.shellrc"sv), "application/octet-stream"sv);
}

TEST_CASE(directory_names_do_not_determine_mime_type)
{
    EXPECT_EQ(Core::guess_mime_type_based_on_filename("folder.html/file.unknown"sv), "application/octet-stream"sv);
}

TEST_CASE(unknown_extension_falls_back_to_binary)
{
    EXPECT_EQ(Core::guess_mime_type_based_on_filename("mystery.unregistered-extension"sv), "application/octet-stream"sv);
}
