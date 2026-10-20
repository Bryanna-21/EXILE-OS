/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include "expkg.h"

#include <algorithm>
#include <fstream>
#include <set>
#include <system_error>

#include "util.h"

namespace fs = std::filesystem;

namespace exile {

namespace {

bool isUnder(fs::path const& child, fs::path const& parent)
{
    auto c = child.begin();
    for (auto p = parent.begin(); p != parent.end(); ++p, ++c)
        if (c == child.end() || *c != *p)
            return false;
    return true;
}

} // namespace

std::optional<ExpkgManifest> parseExpkg(fs::path const& file, ExpkgLimits const& limits, std::string& error)
{
    std::error_code ec;
    uint64_t fileSize = fs::file_size(file, ec);
    if (ec) {
        error = "cannot read package: " + ec.message();
        return std::nullopt;
    }
    std::ifstream in(file, std::ios::binary);
    if (!in) {
        error = "cannot open package";
        return std::nullopt;
    }

    uint64_t consumed = 0;
    auto readLine = [&](std::string& line) -> bool {
        if (!std::getline(in, line))
            return false;
        consumed += line.size() + 1;
        return consumed <= limits.maxHeaderBytes && line.size() <= 2048;
    };

    std::string line;
    if (!readLine(line) || line != "EXPKG1") {
        error = "not a valid package (bad signature line)";
        return std::nullopt;
    }

    ExpkgManifest m;
    std::set<std::string> lowered;
    uint64_t offset = 0;
    bool sawData = false;
    while (readLine(line)) {
        if (line == "data") {
            sawData = true;
            break;
        }
        if (line.rfind("file ", 0) != 0) {
            error = "package header contains an unknown line";
            return std::nullopt;
        }
        if (m.entries.size() >= limits.maxEntries) {
            error = "package has too many files";
            return std::nullopt;
        }
        size_t sp = line.find(' ', 5);
        if (sp == std::string::npos || sp == 5 || sp - 5 > 15) {
            error = "malformed file entry";
            return std::nullopt;
        }
        uint64_t size = 0;
        for (size_t i = 5; i < sp; ++i) {
            if (line[i] < '0' || line[i] > '9') {
                error = "malformed file size";
                return std::nullopt;
            }
            size = size * 10 + static_cast<uint64_t>(line[i] - '0');
        }
        std::string path = line.substr(sp + 1);
        if (!util::isSafeRelativePath(path)) {
            error = "package contains an unsafe path: " + path.substr(0, 80);
            return std::nullopt;
        }
        if (!lowered.insert(util::toLower(path)).second) {
            error = "package contains a duplicate path: " + path;
            return std::nullopt;
        }
        if (size > limits.maxTotalBytes || offset + size > limits.maxTotalBytes) {
            error = "package is too large when unpacked";
            return std::nullopt;
        }
        m.entries.push_back({ std::move(path), size, offset });
        offset += size;
    }
    if (!sawData) {
        error = "package header is truncated or too large";
        return std::nullopt;
    }

    // A path may not be both a file and a directory of another entry.
    for (auto const& e : m.entries) {
        std::string lower = util::toLower(e.path);
        for (size_t pos = lower.find('/'); pos != std::string::npos; pos = lower.find('/', pos + 1))
            if (lowered.count(lower.substr(0, pos))) {
                error = "package path conflicts with another entry: " + e.path;
                return std::nullopt;
            }
    }

    m.dataOffset = consumed;
    m.totalBytes = offset;
    if (consumed > fileSize || fileSize - consumed != offset) {
        error = "package size does not match its header (truncated or padded)";
        return std::nullopt;
    }
    return m;
}

bool extractExpkg(fs::path const& file, ExpkgManifest const& m, fs::path const& destDir, CancelToken const* cancel,
    std::string& error)
{
    std::error_code ec;
    fs::create_directories(destDir, ec);
    if (ec) {
        error = "cannot create " + destDir.string();
        return false;
    }
    fs::path root = fs::weakly_canonical(destDir, ec);
    if (ec) {
        error = "cannot resolve destination";
        return false;
    }

    std::ifstream in(file, std::ios::binary);
    if (!in) {
        error = "cannot open package";
        return false;
    }

    char buf[64 * 1024];
    for (auto const& e : m.entries) {
        if (cancel && cancel->cancelled()) {
            error = "cancelled";
            return false;
        }
        fs::path dest = root / e.path;
        fs::path parent = fs::weakly_canonical(dest.parent_path(), ec);
        if (ec || !isUnder(parent, root)) {
            error = "refusing to write outside the install folder: " + e.path;
            return false;
        }
        fs::create_directories(parent, ec);
        if (ec) {
            error = "cannot create folder for " + e.path;
            return false;
        }
        // Never follow or overwrite an existing symlink.
        if (fs::is_symlink(fs::symlink_status(dest, ec))) {
            error = "refusing to write through a symlink: " + e.path;
            return false;
        }

        in.clear();
        in.seekg(static_cast<std::streamoff>(m.dataOffset + e.offset));
        std::ofstream out(dest, std::ios::binary | std::ios::trunc);
        if (!out) {
            error = "cannot create " + e.path;
            return false;
        }
        uint64_t left = e.size;
        while (left > 0) {
            size_t want = static_cast<size_t>(std::min<uint64_t>(left, sizeof buf));
            in.read(buf, static_cast<std::streamsize>(want));
            if (static_cast<size_t>(in.gcount()) != want) {
                error = "package ended unexpectedly";
                return false;
            }
            out.write(buf, static_cast<std::streamsize>(want));
            if (!out) {
                error = "write failed (disk full?) for " + e.path;
                return false;
            }
            left -= want;
        }
    }
    return true;
}

bool packExpkg(fs::path const& sourceDir, fs::path const& outFile, std::string& error)
{
    std::error_code ec;
    if (!fs::is_directory(sourceDir, ec)) {
        error = "not a directory: " + sourceDir.string();
        return false;
    }
    std::vector<std::pair<std::string, fs::path>> files;
    for (auto it = fs::recursive_directory_iterator(sourceDir, ec); it != fs::recursive_directory_iterator(); it.increment(ec)) {
        if (ec) {
            error = "cannot walk " + sourceDir.string();
            return false;
        }
        auto st = it->symlink_status(ec);
        if (fs::is_symlink(st)) {
            error = "symlinks are not allowed in packages: " + it->path().string();
            return false;
        }
        if (!fs::is_regular_file(st))
            continue;
        std::string rel = fs::relative(it->path(), sourceDir, ec).generic_string();
        if (!util::isSafeRelativePath(rel)) {
            error = "unsafe file name: " + rel;
            return false;
        }
        files.emplace_back(std::move(rel), it->path());
    }
    std::sort(files.begin(), files.end());

    std::string header = "EXPKG1\n";
    for (auto const& [rel, path] : files) {
        uint64_t size = fs::file_size(path, ec);
        if (ec) {
            error = "cannot stat " + path.string();
            return false;
        }
        header += "file " + std::to_string(size) + " " + rel + "\n";
    }
    header += "data\n";

    if (outFile.has_parent_path())
        fs::create_directories(outFile.parent_path(), ec);
    std::ofstream out(outFile, std::ios::binary | std::ios::trunc);
    if (!out) {
        error = "cannot create " + outFile.string();
        return false;
    }
    out.write(header.data(), static_cast<std::streamsize>(header.size()));
    char buf[64 * 1024];
    for (auto const& [rel, path] : files) {
        std::ifstream in(path, std::ios::binary);
        while (in) {
            in.read(buf, sizeof buf);
            out.write(buf, in.gcount());
        }
    }
    out.flush();
    if (!out) {
        error = "write failed for " + outFile.string();
        return false;
    }
    return true;
}

} // namespace exile
