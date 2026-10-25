// Exile OS Living Battlefield - shared small helpers (host build, C++17, no deps).
#pragma once
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <string>
#include <vector>

namespace lb {

// Camera shared with tools/gen_assets.py. Reference frame is 1920x1080.
constexpr float kCamF = 1000.0f;     // focal length (reference px)
constexpr float kCamH = 8.0f;        // camera height above ground (m)
constexpr float kHorizon = 430.0f;   // horizon row (reference px)
constexpr float kRefW = 1920.0f;
constexpr float kRefH = 1080.0f;

struct V3 {
    float r = 0, g = 0, b = 0;
};
inline V3 operator+(V3 a, V3 b) { return { a.r + b.r, a.g + b.g, a.b + b.b }; }
inline V3 operator-(V3 a, V3 b) { return { a.r - b.r, a.g - b.g, a.b - b.b }; }
inline V3 operator*(V3 a, float s) { return { a.r * s, a.g * s, a.b * s }; }
inline V3 operator*(V3 a, V3 b) { return { a.r * b.r, a.g * b.g, a.b * b.b }; }
inline V3 mix(V3 a, V3 b, float t) { return a + (b - a) * t; }

inline float clampf(float x, float lo, float hi) { return x < lo ? lo : (x > hi ? hi : x); }
inline float lerpf(float a, float b, float t) { return a + (b - a) * t; }
inline float saturate(float x) { return clampf(x, 0.0f, 1.0f); }
inline float smooth01(float x)
{
    x = saturate(x);
    return x * x * (3.0f - 2.0f * x);
}
inline float ramp(float x, float a, float b) { return smooth01((x - a) / (b - a)); }
inline float wrap24(float h)
{
    h = std::fmod(h, 24.0f);
    return h < 0 ? h + 24.0f : h;
}

inline uint32_t hash32(uint32_t x)
{
    x ^= x >> 16; x *= 0x7feb352dU;
    x ^= x >> 15; x *= 0x846ca68bU;
    x ^= x >> 16;
    return x;
}
inline float hash01(uint32_t x) { return (hash32(x) & 0xFFFFFF) / float(0x1000000); }

// Ground-plane camera projection (world metres -> reference pixels).
struct Proj {
    float sx, sy, scale; // scale = reference px per metre at that depth
};
inline Proj project(float X, float Z, float lift = 0.0f)
{
    float k = kCamF / Z;
    return { kRefW * 0.5f + X * k, kHorizon + (kCamH - lift) * k, k };
}

} // namespace lb
