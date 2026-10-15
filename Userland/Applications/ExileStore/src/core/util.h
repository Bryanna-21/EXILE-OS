#pragma once
// Small, dependency-free helpers shared by every layer.

#include <cstdint>
#include <filesystem>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace exile::util {

// True only for plain relative paths made of normal segments separated by '/'.
// Rejects: empty, absolute, "." / ".." segments, empty segments, backslashes,
// control characters, NUL, a ':' in the first segment, overlong names.
bool isSafeRelativePath(std::string_view path);

bool hasControlChars(std::string_view s, bool allowNewlineAndTab = false);
std::string toLower(std::string_view s);

// Exactly `len` characters, all in [0-9a-f].
bool isLowerHex(std::string_view s, size_t len);
std::optional<std::vector<uint8_t>> hexDecode(std::string_view hex);
std::string hexEncode(const uint8_t* data, size_t size);

std::string humanSize(uint64_t bytes);
std::string nowIso8601();

// Reads a whole file, refusing files larger than maxBytes.
std::optional<std::string> readFile(const std::filesystem::path& path, uint64_t maxBytes, std::string* error = nullptr);
// Writes to "<path>.tmp" then renames over `path`.
bool writeFileAtomic(const std::filesystem::path& path, std::string_view data, std::string& error);

// ASCII case-insensitive substring test.
bool containsIgnoreCase(std::string_view haystack, std::string_view needle);

}  // namespace exile::util
