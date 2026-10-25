#include "daylight.h"

namespace lb {

namespace {

constexpr int kN = 14; // zen3 hor3 amb3 sun3 sunStr mist
struct Key {
    float h;
    float v[kN];
};

// Colours stay in blue/green/amber families (no purple): G >= R wherever blue dominates.
const Key kKeys[] = {
    { 0.0f,  { 0.020f, 0.040f, 0.085f, 0.060f, 0.095f, 0.150f, 0.17f, 0.23f, 0.36f, 0.0f, 0.0f, 0.0f, 0.00f, 0.20f } },
    { 3.0f,  { 0.022f, 0.045f, 0.095f, 0.100f, 0.135f, 0.200f, 0.16f, 0.22f, 0.35f, 0.0f, 0.0f, 0.0f, 0.00f, 0.30f } },
    { 4.6f,  { 0.045f, 0.080f, 0.160f, 0.160f, 0.190f, 0.260f, 0.19f, 0.25f, 0.38f, 0.0f, 0.0f, 0.0f, 0.00f, 0.32f } },
    { 6.0f,  { 0.200f, 0.320f, 0.520f, 0.800f, 0.740f, 0.640f, 0.38f, 0.44f, 0.54f, 1.0f, 0.74f, 0.52f, 0.50f, 0.52f } },
    { 7.5f,  { 0.230f, 0.410f, 0.670f, 0.720f, 0.790f, 0.820f, 0.50f, 0.56f, 0.64f, 1.0f, 0.88f, 0.72f, 0.82f, 0.24f } },
    { 9.0f,  { 0.210f, 0.430f, 0.730f, 0.680f, 0.790f, 0.880f, 0.56f, 0.63f, 0.71f, 1.0f, 0.95f, 0.86f, 1.00f, 0.10f } },
    { 12.0f, { 0.170f, 0.390f, 0.730f, 0.640f, 0.770f, 0.900f, 0.62f, 0.68f, 0.76f, 1.0f, 0.98f, 0.92f, 1.10f, 0.00f } },
    { 15.0f, { 0.190f, 0.390f, 0.710f, 0.680f, 0.770f, 0.860f, 0.58f, 0.63f, 0.68f, 1.0f, 0.93f, 0.80f, 0.96f, 0.00f } },
    { 17.0f, { 0.200f, 0.340f, 0.580f, 0.820f, 0.720f, 0.580f, 0.50f, 0.50f, 0.52f, 1.0f, 0.80f, 0.55f, 0.72f, 0.02f } },
    { 18.5f, { 0.140f, 0.210f, 0.400f, 0.930f, 0.520f, 0.300f, 0.34f, 0.34f, 0.40f, 1.0f, 0.55f, 0.30f, 0.42f, 0.05f } },
    { 19.7f, { 0.060f, 0.110f, 0.250f, 0.300f, 0.240f, 0.210f, 0.19f, 0.22f, 0.32f, 0.0f, 0.0f, 0.0f, 0.00f, 0.10f } },
    { 21.0f, { 0.026f, 0.052f, 0.125f, 0.095f, 0.130f, 0.210f, 0.17f, 0.22f, 0.35f, 0.0f, 0.0f, 0.0f, 0.00f, 0.15f } },
};
constexpr int kKeyCount = sizeof(kKeys) / sizeof(kKeys[0]);

float catmull(float p0, float p1, float p2, float p3, float t)
{
    float t2 = t * t, t3 = t2 * t;
    return 0.5f * ((2 * p1) + (-p0 + p2) * t + (2 * p0 - 5 * p1 + 4 * p2 - p3) * t2 + (-p0 + 3 * p1 - 3 * p2 + p3) * t3);
}

void interp(float hour, float out[kN])
{
    int i = 0;
    for (int k = 0; k < kKeyCount; ++k)
        if (kKeys[k].h <= hour) i = k;
    int i1 = (i + 1) % kKeyCount;
    float h0 = kKeys[i].h, h1 = kKeys[i1].h + (i1 <= i ? 24.0f : 0.0f);
    float t = (hour - h0) / (h1 - h0);
    int im = (i + kKeyCount - 1) % kKeyCount, i2 = (i1 + 1) % kKeyCount;
    for (int c = 0; c < kN; ++c) {
        float v = catmull(kKeys[im].v[c], kKeys[i].v[c], kKeys[i1].v[c], kKeys[i2].v[c], t);
        out[c] = v;
    }
}

} // namespace

Daylight compute_daylight(float hour)
{
    hour = wrap24(hour);
    float v[kN];
    interp(hour, v);
    Daylight d;
    d.zenith = { std::max(0.f, v[0]), std::max(0.f, v[1]), std::max(0.f, v[2]) };
    d.horizon = { std::max(0.f, v[3]), std::max(0.f, v[4]), std::max(0.f, v[5]) };
    d.ambient = { std::max(0.f, v[6]), std::max(0.f, v[7]), std::max(0.f, v[8]) };
    d.sun_color = { saturate(v[9]), saturate(v[10]), saturate(v[11]) };
    d.sun_strength = std::max(0.f, v[12]);
    d.mist = saturate(v[13]);

    // Sun arc 06:00 -> 18:30.
    float u = (hour - 6.0f) / 12.5f;
    if (u >= 0 && u <= 1) {
        // fade direct light in/out over the first/last ~45 minutes so sunrise/sunset never pop
        d.sun_strength *= ramp(u, 0.0f, 0.06f) * (1.0f - ramp(u, 0.94f, 1.0f));
        d.sun_elev = std::sin(float(M_PI) * u);
        d.sun_x = kRefW * (0.12f + 0.76f * u);
        d.sun_y = kHorizon + 40.0f - d.sun_elev * 360.0f;
    } else {
        d.sun_elev = 0;
        d.sun_strength = 0;
    }
    // Moon arc 18:30 -> 06:00.
    float um = hour >= 18.5f ? (hour - 18.5f) / 11.5f : (hour + 24.0f - 18.5f) / 11.5f;
    if (um >= 0 && um <= 1 && !(hour > 6.0f && hour < 18.5f)) {
        float me = std::sin(float(M_PI) * um);
        d.moon_x = kRefW * (0.14f + 0.72f * um);
        d.moon_y = kHorizon + 30.0f - me * 330.0f;
        d.moon_strength = saturate(me * 3.0f) * saturate(1.0f - d.sun_elev * 3.0f);
    }
    // Night factor: 1 in deep night, 0 in day.
    float day = ramp(hour, 5.2f, 7.0f) * (1.0f - ramp(hour, 18.2f, 20.0f));
    d.night = 1.0f - day;
    // Twilight glow: east at dawn (04:00-08:00), west at dusk (17:00-20:30).
    float gd = std::pow(ramp(hour, 3.0f, 6.0f), 1.5f) * (1.0f - ramp(hour, 6.0f, 8.2f));
    float gs = ramp(hour, 16.8f, 18.6f) * (1.0f - ramp(hour, 18.8f, 20.6f));
    if (gd >= gs) { d.glow = gd; d.glow_x = kRefW * 0.12f; d.glow_color = { 1.0f, 0.70f, 0.42f }; }
    else { d.glow = gs; d.glow_x = kRefW * 0.88f; d.glow_color = { 1.0f, 0.50f, 0.22f }; }
    // Shadows: long and soft at low sun, short at noon; faint moon shadows at night.
    float el = std::max(0.12f, d.sun_elev);
    d.shadow_shear = d.sun_elev > 0 ? clampf(-(d.sun_x - kRefW * 0.5f) / (kRefW * 0.5f) * (0.35f / el) * 0.8f, -4.0f, 4.0f) : 0.9f;
    d.shadow_alpha = d.sun_elev > 0 ? 0.50f * saturate(d.sun_elev * 4.0f) : 0.16f * d.moon_strength;
    d.exposure = 1.0f;
    d.lantern = hour >= 12.0f ? ramp(hour, 17.9f, 19.4f) : 1.0f - ramp(hour, 5.3f, 6.6f); // lit through the night, wraps past 24:00
    d.fire_base = fire_level(hour, 0.0f);
    return d;
}

float fire_level(float hour, float off)
{
    hour = wrap24(hour);
    // Midday cooking fires (small), evening ignition, steady burn, lower before dawn, extinguished after.
    float noon = 0.28f * ramp(hour, 10.8f, 11.6f) * (1.0f - ramp(hour, 13.2f, 14.2f));
    float evening = ramp(hour, 17.5f + off, 18.2f + off);
    float burn = 1.0f;
    if (hour < 12.0f) {
        evening = 1.0f;
        burn = 1.0f - 0.55f * ramp(hour, 3.0f, 5.0f);
        burn *= 1.0f - ramp(hour, 5.6f + off * 0.5f, 6.9f + off * 0.5f);
    }
    return std::max(noon, evening * burn);
}

} // namespace lb
