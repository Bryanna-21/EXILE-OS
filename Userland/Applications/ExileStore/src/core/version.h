/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#pragma once

// Semantic-version parsing and ordering (semver 2.0 precedence rules).

#include <cstdint>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace exile {

struct Version {
    uint64_t major = 0;
    uint64_t minor = 0;
    uint64_t patch = 0;
    std::vector<std::string> prerelease; // empty => a normal release

    std::string str() const;
};

// Accepts MAJOR.MINOR.PATCH[-prerelease][+build]. Build metadata is ignored.
std::optional<Version> parseVersion(std::string_view text);

// -1, 0, 1.
int compareVersions(Version const& a, Version const& b);

// Convenience for already-validated strings; unparsable input compares as the lowest version.
int compareVersionStrings(std::string_view a, std::string_view b);

} // namespace exile
