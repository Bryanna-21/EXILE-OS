// Time-of-day lighting: smooth Catmull-Rom interpolation of authored keyframes.
#pragma once
#include "common.h"

namespace lb {

struct Daylight {
    V3 zenith, horizon, ambient, sun_color, glow_color;
    float sun_strength = 0, mist = 0, exposure = 1;
    float sun_elev = 0;                // 0..1 while the sun is up, else 0
    float sun_x = 0, sun_y = 0;        // reference px
    float moon_x = 0, moon_y = 0, moon_strength = 0;
    float night = 0;                   // 0 day .. 1 deep night
    float glow = 0, glow_x = 0;        // horizon twilight glow
    float shadow_shear = 0, shadow_alpha = 0; // horizontal shear per unit height; opacity
    float fire_base = 0;               // global fire availability (see fire_level)
    float lantern = 0;                 // lanterns lit 0..1
};

Daylight compute_daylight(float hour);

// Individual fire level (0..1); `offset` staggers progressive ignition in the evening.
float fire_level(float hour, float offset);

} // namespace lb
