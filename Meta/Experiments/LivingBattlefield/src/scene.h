// Shared scene definition + group activity system.
//
// One deterministic world (seeded): terrain geometry is baked in assets/ground.png,
// formations / camps / routes are defined here. Everything an entity does is a pure
// function of (simulated hour, animation seconds) - no per-frame state - so a manual
// time jump, pause or wrap at 24h can never make anything teleport.
#pragma once
#include "atlas.h"
#include "common.h"
#include "daylight.h"
#include <vector>

namespace lb {

enum class DKind : uint8_t { Sprite, Cloth, Fire, Glow };

struct Drawable {
    float X = 0, Z = 30;    // ground position (m); Z is depth
    float lift = 0;         // height above ground (m)
    int sprite = -1;
    bool flip = false;
    float mpp = 80;         // sprite pixels per metre of the artwork
    float sy = 1;           // vertical squash (tent erection)
    float alpha = 1;
    bool shadow = true;
    DKind kind = DKind::Sprite;
    float wave = 0;         // cloth amplitude
    int fire = -1;
    V3 col { 1, 1, 1 };     // Glow colour
    float radius = 1;       // Glow radius (m)
    float intensity = 1;    // Glow intensity
    bool star = false;      // lantern glint
    float lantern_lift = -1;// if >=0, a raised lantern glow sits at (X + lantern_dx, lift = this)
    float lantern_dx = 0;
};

struct FireInst { float X, Z, level; uint32_t id; };
struct LightInst { float X, Z, lift; V3 col; float radius; float intensity; };

struct Frame {
    std::vector<Drawable> drawables;
    std::vector<FireInst> fires;
    std::vector<LightInst> lights;
    Frame()
    {
        drawables.reserve(2400);
        fires.reserve(16);
        lights.reserve(128);
    }
    void clear() { drawables.clear(); fires.clear(); lights.clear(); }
};

float wind_at(float anim);

class Scene {
public:
    Scene(Atlas const& atlas, uint32_t seed);
    void evaluate(float hour, float anim, Daylight const& d, Frame& out) const;
    size_t entity_count() const { return m_entities.size(); }
    struct EntityPose { float X = 0, Z = 0, lift = 0; bool valid = false; uint8_t role = 0, army = 0; int pair = -1; };
    // Position of every entity at `hour` (first drawable). Used by --selftest to detect teleports.
    void entity_poses(float hour, float anim, std::vector<EntityPose>& out) const;

    enum Role : uint8_t { Soldier, Officer, Standard, Sentry, Carrier, Attendant, Rider, Horse };
    struct Entity {
        uint8_t army = 0, role = Soldier, var = 0, sub = 0;
        float shift = 0;           // personal schedule offset (hours)
        float fx = 0, fz = 0;      // formation slot
        float ax = 0, az = 0;      // camp work site
        float bx = 0, bz = 0;      // camp rest slot (ring around a fire)
        float u0 = 0;              // route phase
        int fire = 0, tent = 0, coat = 0, pair = -1;
        float arc = 0;             // sideways bow while marching (m)
        float zmin = 0, zmax = 0;  // officer inspection range
        uint32_t seed = 0;
    };
    struct Tent { float X, Z; int kind, trim; bool permanent; int order; int army; };
    struct FireDef { float X, Z, off; };

private:
    // Cumulative weight tables so route progress is a pure function of time.
    struct Cum {
        std::vector<float> v;
        float total = 1;
        float at(float hour) const;
        template <class F> void build(F&& weight)
        {
            int n = 24 * 96;
            v.assign(size_t(n) + 1, 0.0f);
            for (int i = 1; i <= n; ++i) v[size_t(i)] = v[size_t(i - 1)] + weight((i - 0.5f) / 96.0f) / 96.0f;
            total = std::max(1e-4f, v.back());
        }
    };

    Atlas const& m_atlas;
    std::vector<Entity> m_entities;
    std::vector<Tent> m_tents;
    std::vector<FireDef> m_fires; // both camps
    struct Prop { float X, Z; int sprite; float mpp; bool shadow; float lift; };
    std::vector<Prop> m_props;
    Cum m_cum_patrol, m_cum_carry, m_cum_inspect;
    float m_camp_x[2], m_camp_z[2];
    int m_human[2][3][5][9];
    int m_horse[3][3][7];
    int m_sp_campfire = -1, m_sp_pole[2] = { -1, -1 }, m_sp_cloth[2] = { -1, -1 }, m_sp_pole_any = -1;
    int m_sp_tent[3][3];

    void build(uint32_t seed);
    int human(int army, int var, int acc, int pose) const;
    void emit_entity(Entity const& e, float hour, float anim, Daylight const& d, Frame& out) const;
    float tent_up(Tent const& t, float hour) const;
};

} // namespace lb
