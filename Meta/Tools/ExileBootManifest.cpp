#include <Kernel/Boot/Exile/BootManifest.h>

#include <cstdio>
#include <fstream>
#include <vector>

namespace {

static constexpr unsigned int RoundConstants[64] {
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
    0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
    0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152,
    0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3,
    0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85,
    0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354,
    0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1,
    0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
    0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
    0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3,
    0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee,
    0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa,
    0xa4506ceb, 0xbef9a3f7, 0xc67178f2
};

static constexpr unsigned int InitialState[8] {
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
    0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19
};

static constexpr unsigned int rotr(unsigned int value, unsigned int bits)
{
    return (value >> bits) | (value << (32 - bits));
}

static constexpr unsigned int choose(unsigned int x, unsigned int y, unsigned int z)
{
    return (x & y) ^ (~x & z);
}

static constexpr unsigned int majority(unsigned int x, unsigned int y, unsigned int z)
{
    return (x & y) ^ (x & z) ^ (y & z);
}

static constexpr unsigned int big_sigma0(unsigned int x)
{
    return rotr(x, 2) ^ rotr(x, 13) ^ rotr(x, 22);
}

static constexpr unsigned int big_sigma1(unsigned int x)
{
    return rotr(x, 6) ^ rotr(x, 11) ^ rotr(x, 25);
}

static constexpr unsigned int small_sigma0(unsigned int x)
{
    return rotr(x, 7) ^ rotr(x, 18) ^ (x >> 3);
}

static constexpr unsigned int small_sigma1(unsigned int x)
{
    return rotr(x, 17) ^ rotr(x, 19) ^ (x >> 10);
}

static void transform(unsigned int state[8], unsigned char const* block)
{
    unsigned int schedule[64];

    for (size_t i = 0; i < 16; ++i) {
        auto offset = i * 4;
        schedule[i] = (static_cast<unsigned int>(block[offset]) << 24)
            | (static_cast<unsigned int>(block[offset + 1]) << 16)
            | (static_cast<unsigned int>(block[offset + 2]) << 8)
            | static_cast<unsigned int>(block[offset + 3]);
    }

    for (size_t i = 16; i < 64; ++i)
        schedule[i] = small_sigma1(schedule[i - 2]) + schedule[i - 7]
            + small_sigma0(schedule[i - 15]) + schedule[i - 16];

    unsigned int a = state[0];
    unsigned int b = state[1];
    unsigned int c = state[2];
    unsigned int d = state[3];
    unsigned int e = state[4];
    unsigned int f = state[5];
    unsigned int g = state[6];
    unsigned int h = state[7];

    for (size_t i = 0; i < 64; ++i) {
        auto t1 = h + big_sigma1(e) + choose(e, f, g) + RoundConstants[i] + schedule[i];
        auto t2 = big_sigma0(a) + majority(a, b, c);

        h = g;
        g = f;
        f = e;
        e = d + t1;
        d = c;
        c = b;
        b = a;
        a = t1 + t2;
    }

    state[0] += a;
    state[1] += b;
    state[2] += c;
    state[3] += d;
    state[4] += e;
    state[5] += f;
    state[6] += g;
    state[7] += h;
}

void sha256(void const* data, size_t size, unsigned char digest[32])
{
    unsigned int state[8];
    for (size_t i = 0; i < 8; ++i)
        state[i] = InitialState[i];

    auto bytes = static_cast<unsigned char const*>(data);
    auto full_blocks = size / 64;

    for (size_t i = 0; i < full_blocks; ++i)
        transform(state, bytes + i * 64);

    unsigned char block[128] {};
    auto remaining = size % 64;

    for (size_t i = 0; i < remaining; ++i)
        block[i] = bytes[full_blocks * 64 + i];

    block[remaining] = 0x80;

    auto final_size = remaining < 56 ? 64 : 128;
    auto bit_length = static_cast<unsigned long long>(size) * 8;

    for (size_t i = 0; i < 8; ++i)
        block[final_size - 1 - i] = static_cast<unsigned char>(bit_length >> (i * 8));

    transform(state, block);

    if (final_size == 128)
        transform(state, block + 64);

    for (size_t i = 0; i < 8; ++i) {
        digest[i * 4] = static_cast<unsigned char>(state[i] >> 24);
        digest[i * 4 + 1] = static_cast<unsigned char>(state[i] >> 16);
        digest[i * 4 + 2] = static_cast<unsigned char>(state[i] >> 8);
        digest[i * 4 + 3] = static_cast<unsigned char>(state[i]);
    }
}

}

int main(int argc, char** argv)
{
    if (argc != 3) {
        std::fprintf(stderr, "usage: %s <kernel> <manifest>\n", argv[0]);
        return 1;
    }

    std::ifstream input(argv[1], std::ios::binary | std::ios::ate);
    if (!input) {
        std::fprintf(stderr, "failed to open kernel: %s\n", argv[1]);
        return 1;
    }

    auto size = input.tellg();
    if (size < 0 || static_cast<unsigned long long>(size) > 0xFFFFFFFFULL) {
        std::fprintf(stderr, "kernel is too large\n");
        return 1;
    }

    std::vector<unsigned char> data(static_cast<size_t>(size));
    input.seekg(0);

    if (!data.empty())
        input.read(reinterpret_cast<char*>(data.data()), data.size());

    if (!input && !data.empty()) {
        std::fprintf(stderr, "failed to read kernel\n");
        return 1;
    }

    Exile::Boot::BootManifest manifest {};
    manifest.kernel_size = static_cast<unsigned int>(data.size());

    sha256(
        data.data(),
        data.size(),
        manifest.kernel_sha256);

    std::ofstream output(argv[2], std::ios::binary | std::ios::trunc);
    if (!output) {
        std::fprintf(stderr, "failed to create manifest: %s\n", argv[2]);
        return 1;
    }

    output.write(
        reinterpret_cast<char const*>(&manifest),
        sizeof(manifest));

    if (!output) {
        std::fprintf(stderr, "failed to write manifest\n");
        return 1;
    }

    std::printf("Exile Boot Manifest v%u\n", manifest.version);
    std::printf("Kernel size: %u bytes\n", manifest.kernel_size);
    std::printf("Kernel SHA-256: ");

    for (auto byte : manifest.kernel_sha256)
        std::printf("%02x", byte);

    std::printf("\n");

    return 0;
}
