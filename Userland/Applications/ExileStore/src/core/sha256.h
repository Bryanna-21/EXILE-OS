#pragma once
// Self-contained SHA-256 (FIPS 180-4) used for package integrity digests.

#include <array>
#include <cstdint>
#include <filesystem>
#include <optional>
#include <string>
#include <string_view>

namespace exile {

class Sha256 {
public:
    Sha256();
    void update(const void* data, size_t size);
    std::array<uint8_t, 32> finish();
    std::string finishHex();

private:
    void block(const uint8_t* p);
    uint32_t h_[8];
    uint8_t buf_[64];
    size_t bufLen_ = 0;
    uint64_t total_ = 0;
};

std::string sha256Hex(std::string_view data);
std::optional<std::string> sha256FileHex(const std::filesystem::path& path, std::string* error = nullptr);

}  // namespace exile
