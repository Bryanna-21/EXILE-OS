#pragma once
// ".expkg": the development package container used by the local backend.
//
// Layout (all ASCII header, then raw bytes):
//   EXPKG1\n
//   file <size> <relative/path>\n      (repeated)
//   data\n
//   <file bytes concatenated, in header order>
//
// It is deliberately NOT a general archive format: no symlinks, no hard links,
// no permissions, no absolute paths. Every path is validated before anything is
// written, and extraction stays inside the destination directory. This is the
// store's dev format; the production Exile package format is a separate
// integration point (docs/INTEGRATION.md).

#include <cstdint>
#include <filesystem>
#include <optional>
#include <string>
#include <vector>

#include "core/transport.h"

namespace exile {

struct ExpkgEntry {
    std::string path;
    uint64_t size = 0;
    uint64_t offset = 0;  // relative to dataOffset
};

struct ExpkgLimits {
    size_t maxEntries = 4096;
    uint64_t maxTotalBytes = 8ull * 1024 * 1024 * 1024;
    uint64_t maxHeaderBytes = 1024 * 1024;
};

struct ExpkgManifest {
    std::vector<ExpkgEntry> entries;
    uint64_t dataOffset = 0;
    uint64_t totalBytes = 0;
};

std::optional<ExpkgManifest> parseExpkg(const std::filesystem::path& file, const ExpkgLimits& limits, std::string& error);

// Extracts into destDir (created if needed). Returns false and fills `error` on any problem.
bool extractExpkg(const std::filesystem::path& file, const ExpkgManifest& manifest, const std::filesystem::path& destDir,
                  const CancelToken* cancel, std::string& error);

// Builds a package from a directory (regular files only, sorted => reproducible).
bool packExpkg(const std::filesystem::path& sourceDir, const std::filesystem::path& outFile, std::string& error);

}  // namespace exile
