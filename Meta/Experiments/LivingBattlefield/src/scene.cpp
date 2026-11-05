#include "scene.h"
#include <cstdio>

namespace lb {

namespace {

enum Pose { StandA = 0, StandB, Walk0, Walk1, Walk2, Walk3, Sit, Crouch, Ride, PoseCount };
enum Acc { AccSpear = 0, AccCrate, AccLantern, AccOfficer, AccNone, AccCount };
constexpr char const* kPoseNames[] = { "stand_a", "stand_b", "walk0", "walk1", "walk2", "walk3", "sit", "crouch", "ride" };
constexpr char const* kAccNames[] = { "spear", "crate", "lantern", "officer", "none" };
constexpr char const* kHorsePose[] = { "stand_a", "stand_b", "graze", "walk0", "walk1", "walk2", "walk3" };
constexpr char const* kCoat[] = { "bay", "grey", "black" };
enum HPose { HStandA = 0, HStandB, HGraze, HWalk0, HWalk1, HWalk2, HWalk3 };

constexpr int kFires = 6;
constexpr float kHumanMpp = 80.0f;
constexpr float kHorseMpp = 58.0f;

float formation_weight(float t)
{
    if (t < 4.5f) return 0;
    if (t < 6.0f) return smooth01((t - 4.5f) / 1.5f);
    if (t < 16.5f) return 1;
    if (t < 19.5f) return 1.0f - smooth01((t - 16.5f) / 3.0f);
    return 0;
}
float patrol_weight(float h) { return h >= 12.0f ? ramp(h, 17.8f, 19.2f) : 1.0f - ramp(h, 5.3f, 6.5f); }
float carry_weight(float h)
{
    float sup = ramp(h, 10.8f, 11.4f) * (1.0f - ramp(h, 20.6f, 21.2f));
    float night = h >= 12.0f ? 0.7f * ramp(h, 21.2f, 21.8f) : 0.7f * (1.0f - ramp(h, 1.6f, 2.4f));
    return std::max(sup, night);
}
float inspect_weight(float h)
{
    float a = ramp(h, 9.0f, 9.4f) * (1.0f - ramp(h, 11.2f, 11.6f));
    float b = ramp(h, 14.3f, 14.7f) * (1.0f - ramp(h, 16.0f, 16.4f));
    return std::min(1.0f, a + b);
}
float tri(float u) // 0..1..0 triangle wave, period 1
{
    u -= std::floor(u);
    return u < 0.5f ? u * 2.0f : 2.0f - u * 2.0f;
}
float tri_dir(float u)
{
    u -= std::floor(u);
    return u < 0.5f ? 1.0f : -1.0f;
}

} // namespace

float wind_at(float anim) { return 0.65f + 0.30f * std::sin(anim * 0.27f) + 0.18f * std::sin(anim * 0.71f + 1.3f); }

float Scene::Cum::at(float hour) const
{
    hour = wrap24(hour);
    float f = hour * 96.0f;
    int i = std::min(int(f), int(v.size()) - 2);
    float t = f - float(i);
    return (v[size_t(i)] * (1 - t) + v[size_t(i) + 1] * t) / total;
}

Scene::Scene(Atlas const& atlas, uint32_t seed)
    : m_atlas(atlas)
{
    build(seed);
}

int Scene::human(int army, int var, int acc, int pose) const
{
    int id = m_human[army][var][acc][pose];
    if (id >= 0) return id;
    id = m_human[army][var][acc][StandA];
    if (id >= 0) return id;
    return m_human[army][var][AccSpear][StandA];
}

float Scene::tent_up(Tent const& t, float h) const
{
    if (t.permanent) return 1.0f;
    float d = 0.09f * float(t.order);
    return h >= 12.0f ? ramp(h, 17.2f + d, 18.4f + d) : 1.0f - ramp(h, 5.2f + d * 1.5f, 6.8f + d * 1.5f);
}

void Scene::build(uint32_t seed)
{
    // --- sprite id tables ---
    for (int a = 0; a < 2; ++a)
        for (int v = 0; v < 3; ++v)
            for (int c = 0; c < AccCount; ++c)
                for (int p = 0; p < PoseCount; ++p) {
                    char n[96];
                    std::snprintf(n, sizeof n, "h_%d_%d_%s_%s", a, v, kAccNames[c], kPoseNames[p]);
                    m_human[a][v][c][p] = m_atlas.find(n);
                }
    for (int c = 0; c < 3; ++c)
        for (int b = 0; b < 3; ++b)
            for (int p = 0; p < 7; ++p) {
                char n[96];
                std::snprintf(n, sizeof n, "horse_%s_%d_%s", kCoat[c], b - 1, kHorsePose[p]);
                m_horse[c][b][p] = m_atlas.find(n);
            }
    static char const* kKind[] = { "aframe", "pavilion", "shelter" };
    for (int k = 0; k < 3; ++k)
        for (int t = 0; t < 3; ++t) {
            char n[96];
            std::snprintf(n, sizeof n, "tent_%s_%d", kKind[k], t - 1);
            m_sp_tent[k][t] = m_atlas.find(n);
        }
    m_sp_campfire = m_atlas.find("campfire");
    m_sp_pole[0] = m_atlas.find("pole_w"); m_sp_pole[1] = m_atlas.find("pole_e");
    m_sp_cloth[0] = m_atlas.find("cloth_w"); m_sp_cloth[1] = m_atlas.find("cloth_e");

    // --- schedule integrals ---
    m_cum_patrol.build([](float h) { return patrol_weight(h); });
    m_cum_carry.build([](float h) { return carry_weight(h); });
    m_cum_inspect.build([](float h) { return inspect_weight(h); });

    // --- camps ---
    m_camp_x[0] = -11.0f; m_camp_z[0] = 22.0f;
    m_camp_x[1] = 11.0f;  m_camp_z[1] = 23.5f;
    static const float kFire[kFires][2] = { { -3.0f, -1.6f }, { 3.2f, -2.0f }, { -2.6f, 2.6f }, { 3.6f, 2.4f }, { 0.4f, -4.4f }, { -0.6f, 4.8f } };
    struct TD { float dx, dz; int kind; bool perm; };
    static const TD kTents[9] = {
        { -6.8f, 5.6f, 1, true }, { -3.2f, 8.0f, 0, true }, { 0.8f, 8.6f, 0, true }, { 4.8f, 7.6f, 0, false },
        { 7.6f, 3.8f, 0, false }, { -8.0f, -0.8f, 0, false }, { 8.4f, -1.6f, 0, false }, { -5.0f, -5.0f, 0, false },
        { 5.5f, -5.4f, 0, false } };
    for (int a = 0; a < 2; ++a) {
        float sx = a == 0 ? 1.0f : -1.0f; // mirror dx for the east camp
        for (int f = 0; f < kFires; ++f)
            m_fires.push_back({ m_camp_x[a] + sx * kFire[f][0], m_camp_z[a] + kFire[f][1], 0.12f * float(f) + 0.05f * float(a) });
        for (int t = 0; t < 9; ++t)
            m_tents.push_back({ m_camp_x[a] + sx * kTents[t].dx, m_camp_z[a] + kTents[t].dz, kTents[t].kind, a, kTents[t].perm, t, a });
        // wagons, crates, barrels, firewood
        int wag = m_atlas.find("wagon"), cr = m_atlas.find("crates"), ba = m_atlas.find("barrels"), fw = m_atlas.find("firewood");
        m_props.push_back({ m_camp_x[a] + sx * -9.2f, m_camp_z[a] + 3.4f, wag, 48.0f, true, 0 });
        m_props.push_back({ m_camp_x[a] + sx * 9.0f, m_camp_z[a] + 5.4f, wag, 48.0f, true, 0 });
        m_props.push_back({ m_camp_x[a] + sx * -4.0f, m_camp_z[a] + 4.8f, cr, 69.0f, true, 0 });
        m_props.push_back({ m_camp_x[a] + sx * 2.4f, m_camp_z[a] + 5.6f, ba, 69.0f, true, 0 });
        m_props.push_back({ m_camp_x[a] + sx * -7.0f, m_camp_z[a] + -3.0f, fw, 64.0f, true, 0 });
        m_props.push_back({ m_camp_x[a] + sx * 6.5f, m_camp_z[a] + -3.6f, cr, 69.0f, true, 0 });
        for (int p = 0; p < 3; ++p) { // lantern posts at tent doors (art is a 4.6 m pole; scaled to 1.8 m)
            m_props.push_back({ m_camp_x[a] + sx * (-4.5f + 4.4f * float(p)), m_camp_z[a] + 3.0f + 0.4f * float(p), m_sp_pole[a], 110.0f, true, float(-1000 - p) });
        }
    }
    // --- trees / rocks (screen-edge targets, solved back into world space) ---
    auto edge_prop = [&](float sx, float Z, int sprite, float mpp) {
        m_props.push_back({ (sx - kRefW * 0.5f) * Z / kCamF, Z, sprite, mpp, true, 0 });
    };
    int tr0 = m_atlas.find("tree0"), tr1 = m_atlas.find("tree1"), tr2 = m_atlas.find("tree2"), rk0 = m_atlas.find("rock0"), rk1 = m_atlas.find("rock1");
    float edge[][3] = { { 40, 36, 0 }, { 150, 44, 1 }, { 1850, 38, 1 }, { 1760, 47, 0 }, { 20, 52, 1 }, { 1900, 55, 0 }, { 250, 84, 0 }, { 1700, 90, 1 } };
    for (auto& e : edge) {
        int k = int(e[2]);
        edge_prop(e[0], e[1], k == 0 ? tr0 : tr1, k == 0 ? 24.0f : 21.0f);
    }
    for (int i = 0; i < 46; ++i) { // distant treeline at the base of the hills
        float u = hash01(seed * 7 + 11u * uint32_t(i));
        float Z = 105.0f + 70.0f * hash01(seed + 31u * uint32_t(i));
        int k = hash32(seed + 5u * uint32_t(i)) % 3;
        edge_prop(-200.0f + u * (kRefW + 400.0f), Z, k == 0 ? tr0 : (k == 1 ? tr1 : tr2), k == 0 ? 24.0f : (k == 1 ? 21.0f : 40.0f));
    }
    for (int i = 0; i < 14; ++i) {
        float sx = 80.0f + 1760.0f * hash01(seed * 3 + 17u * uint32_t(i));
        float Z = 30.0f + 40.0f * hash01(seed * 5 + 19u * uint32_t(i));
        if (std::fabs(sx - 960.0f) < 120.0f && Z > 36.0f) sx += 260.0f;
        edge_prop(sx, Z, (i & 1) ? rk0 : rk1, 50.0f);
    }

    // --- entities ---
    auto add = [&](Entity e) -> int { e.seed = hash32(seed ^ uint32_t(m_entities.size() * 2654435761u)); m_entities.push_back(e); return int(m_entities.size()) - 1; };
    int fire_count[2][kFires] = {};
    for (int a = 0; a < 2; ++a) {
        float sign = a == 0 ? -1.0f : 1.0f;
        float cx = m_camp_x[a], cz = m_camp_z[a];
        int tent0 = a * 9;
        auto jitter = [&](uint32_t s, float amp) { return (hash01(s) - 0.5f) * 2.0f * amp; };
        auto assign_camp = [&](Entity& e) {
            int tn = int(e.seed % 9);
            Tent const& T = m_tents[size_t(tent0 + tn)];
            e.tent = tn;
            e.ax = T.X + jitter(e.seed + 1, 1.4f); e.az = T.Z - 1.6f + jitter(e.seed + 2, 0.8f);
            int f = int(hash32(e.seed + 3) % kFires);
            e.fire = f;
            int k = fire_count[a][f]++;
            int ring = k < 6 ? 0 : (k < 16 ? 1 : 2);
            int kk = ring == 0 ? k : (ring == 1 ? k - 6 : k - 16);
            int cap = ring == 0 ? 6 : (ring == 1 ? 10 : 14);
            float ang = 6.2831853f * float(kk) / float(cap) + 0.45f * float(ring) + jitter(e.seed + 4, 0.18f);
            float rad = 1.95f + 1.0f * float(ring) + jitter(e.seed + 5, 0.14f);
            FireDef const& F = m_fires[size_t(a * kFires + f)];
            e.bx = F.X + rad * std::cos(ang); e.bz = F.Z + rad * std::sin(ang) * 0.8f;
            if ((e.seed >> 4) & 1) { // half the men bivouac in small groups at a tent door instead of a fire ring
                Tent const& D = m_tents[size_t(tent0 + int((e.seed >> 6) % 9))];
                e.bx = D.X + jitter(e.seed + 11, 2.6f); e.bz = D.Z - 2.3f + jitter(e.seed + 12, 0.7f);
            }
        };
        auto mk_soldier = [&](float fx, float fz, int sub_hint) {
            Entity e; e.army = uint8_t(a); e.role = Soldier;
            e.seed = hash32(seed + uint32_t(m_entities.size()) * 977u + uint32_t(a) * 13u);
            e.var = uint8_t(e.seed % 3); e.sub = uint8_t(sub_hint >= 0 ? sub_hint : (e.seed >> 8) % 4);
            e.shift = (hash01(e.seed + 7) - 0.5f) * 0.6f;
            e.fx = fx; e.fz = fz; e.arc = (hash01(e.seed + 9) - 0.5f) * 3.0f;
            assign_camp(e);
            add(e);
        };
        // Unit A (front), Unit B (second line)
        int filesA = a == 0 ? 12 : 11;
        for (int r = 0; r < 5; ++r)
            for (int f = 0; f < filesA; ++f)
                mk_soldier(sign * (18.0f + float(r) * 1.7f + jitter(uint32_t(r * 131 + f * 17 + a), 0.18f)),
                    40.0f + float(f) * 1.5f + (a ? 2.0f : 0.0f) + (r & 1 ? 0.35f : 0.0f) + jitter(uint32_t(r * 71 + f * 29 + a * 5), 0.15f), -1);
        for (int r = 0; r < 3; ++r)
            for (int f = 0; f < 10; ++f)
                mk_soldier(sign * (29.0f + float(r) * 1.7f + jitter(uint32_t(r * 53 + f * 19 + a * 3), 0.18f)),
                    44.0f + float(f) * 1.5f + (a ? 2.0f : 0.0f) + jitter(uint32_t(r * 47 + f * 31 + a * 7), 0.15f), -1);
        // Officers
        for (int i = 0; i < 6; ++i) {
            Entity e; e.army = uint8_t(a); e.role = Officer; e.seed = hash32(seed + 5000u + uint32_t(i) + uint32_t(a) * 77u);
            e.var = 1; e.shift = (hash01(e.seed) - 0.5f) * 0.2f;
            e.fx = sign * 15.6f; e.u0 = float(i) / 6.0f + 0.04f * hash01(e.seed + 1);
            e.zmin = 40.0f + (a ? 2.0f : 0.0f); e.zmax = 64.0f + (a ? 2.0f : 0.0f);
            Tent const& P = m_tents[size_t(tent0)];
            e.ax = e.bx = P.X + (float(i) - 2.5f) * 0.95f; e.az = e.bz = P.Z - 2.4f - 0.3f * float(i % 2);
            e.fz = e.zmin; e.fire = i % kFires;
            add(e);
        }
        // Standards
        float sxs[3] = { 21.5f, 31.5f, 19.0f }, szs[3] = { 52.0f, 51.0f, 35.0f };
        for (int i = 0; i < 3; ++i) {
            Entity e; e.army = uint8_t(a); e.role = Standard; e.seed = hash32(seed + 6000u + uint32_t(i) + uint32_t(a) * 79u);
            e.var = 0; e.shift = 0.05f * float(i); e.fx = sign * sxs[i]; e.fz = szs[i] + (a ? 2.0f : 0.0f);
            Tent const& P = m_tents[size_t(tent0)];
            e.ax = e.bx = P.X + sign * -1.0f * (-1.2f + 1.2f * float(i)); e.az = e.bz = P.Z - 1.6f - 0.5f * float(i);
            e.fire = i % kFires;
            add(e);
        }
        // Sentries (perimeter route around the camp), carriers, attendants
        for (int i = 0; i < 8; ++i) {
            Entity e; e.army = uint8_t(a); e.role = Sentry; e.seed = hash32(seed + 7000u + uint32_t(i) + uint32_t(a) * 83u);
            e.var = uint8_t(e.seed % 3); e.u0 = float(i) / 8.0f; e.shift = 0; add(e);
        }
        for (int i = 0; i < 12; ++i) {
            Entity e; e.army = uint8_t(a); e.role = Carrier; e.seed = hash32(seed + 8000u + uint32_t(i) + uint32_t(a) * 89u);
            e.var = uint8_t(e.seed % 2); e.u0 = float(i) / 12.0f; e.shift = 0; add(e);
        }
        for (int i = 0; i < 9; ++i) {
            Entity e; e.army = uint8_t(a); e.role = Attendant; e.seed = hash32(seed + 9000u + uint32_t(i) + uint32_t(a) * 97u);
            e.var = uint8_t(e.seed % 3); e.fire = i % kFires;
            FireDef const& F = m_fires[size_t(a * kFires + e.fire)];
            float ang = 1.1f + 2.1f * float(i / kFires) + 0.5f * float(i % kFires);
            e.ax = e.bx = F.X + 1.15f * std::cos(ang); e.az = e.bz = F.Z + 0.95f * std::sin(ang);
            add(e);
        }
        // Cavalry: 8 riders on the far flank, 6 on the near flank. Horse + rider are paired.
        for (int i = 0; i < 14; ++i) {
            float fx = sign * (i < 8 ? 19.5f : 20.0f);
            float fz = (i < 8 ? 68.0f + float(i) * 1.8f : 29.5f + float(i - 8) * 1.6f) + (a ? 2.0f : 0.0f);
            uint32_t s = hash32(seed + 9500u + uint32_t(i) + uint32_t(a) * 101u);
            Entity h; h.army = uint8_t(a); h.role = Horse; h.seed = s; h.coat = int(s % 3); h.sub = uint8_t((s >> 5) % 4);
            h.shift = (hash01(s + 3) - 0.5f) * 0.3f; h.fx = fx; h.fz = fz; h.arc = (hash01(s + 4) - 0.5f) * 2.0f;
            h.ax = h.bx = cx + sign * -1.0f * (-8.0f + 1.15f * float(i)) * -1.0f; h.az = h.bz = cz + 11.0f + 0.4f * std::sin(float(i));
            int hid = add(h);
            Entity r; r.army = uint8_t(a); r.role = Rider; r.seed = hash32(s + 99); r.var = uint8_t(r.seed % 3);
            r.sub = h.sub; r.shift = h.shift; r.fx = fx; r.fz = fz; r.arc = h.arc; r.pair = hid;
            r.ax = h.ax; r.az = h.az;
            int f = int(r.seed % kFires); r.fire = f;
            int k = fire_count[a][f]++; float ang = 6.2831853f * float(k % 8) / 8.0f + 0.9f;
            FireDef const& F = m_fires[size_t(a * kFires + f)];
            r.bx = F.X + 1.7f * std::cos(ang); r.bz = F.Z + 1.4f * std::sin(ang);
            add(r);
        }
        // Spare draft horses by the wagons (always standing / grazing).
        for (int i = 0; i < 3; ++i) {
            Entity h; h.army = uint8_t(a); h.role = Horse; h.seed = hash32(seed + 9700u + uint32_t(i) + uint32_t(a) * 7u);
            h.coat = int(h.seed % 3); h.sub = uint8_t(i); h.pair = -2;
            h.ax = h.bx = cx + sign * -1.0f * (-12.0f + 1.5f * float(i)) * -1.0f; h.az = h.bz = cz + 5.0f + 0.5f * float(i);
            h.fx = h.ax; h.fz = h.az; add(h);
        }
    }
}

void Scene::evaluate(float hour, float anim, Daylight const& d, Frame& out) const
{
    out.clear();
    hour = wrap24(hour);
    float wind = wind_at(anim);
    float lantern = d.lantern;

    // Fires (sprite base + animated fire drawable at the same depth)
    for (size_t i = 0; i < m_fires.size(); ++i) {
        FireDef const& f = m_fires[i];
        float lvl = fire_level(hour, f.off);
        float flick = 0.88f + 0.12f * std::sin(anim * 9.0f + float(i) * 1.7f) + 0.06f * std::sin(anim * 21.3f + float(i) * 3.1f);
        Drawable base; base.X = f.X; base.Z = f.Z; base.sprite = m_sp_campfire; base.mpp = 59.0f; base.shadow = false;
        out.drawables.push_back(base);
        if (lvl > 0.01f) {
            Drawable fd; fd.kind = DKind::Fire; fd.X = f.X; fd.Z = f.Z - 0.05f; fd.fire = int(out.fires.size());
            out.drawables.push_back(fd);
            out.fires.push_back({ f.X, f.Z, lvl, uint32_t(i) });
            out.lights.push_back({ f.X, f.Z, 0.5f, { 1.0f, 0.50f, 0.18f }, 6.0f, 0.78f * lvl * flick });
        }
    }
    // Tents (erection / striking), lamps, lantern posts
    for (size_t i = 0; i < m_tents.size(); ++i) {
        Tent const& t = m_tents[i];
        float up = tent_up(t, hour);
        if (up < 0.01f) continue;
        int trim = (t.kind == 1 || (t.order % 3) == 0) ? t.army : -1;
        Drawable dd; dd.X = t.X; dd.Z = t.Z; dd.sprite = m_sp_tent[t.kind][trim + 1];
        dd.mpp = t.kind == 1 ? 40.0f : (t.kind == 0 ? 45.0f : 43.0f);
        dd.sy = lerpf(0.10f, 1.0f, up); dd.alpha = saturate(up * 6.0f);
        out.drawables.push_back(dd);
        if (t.kind == 1) { // pennant on the pavilion apex
            Drawable cl; cl.kind = DKind::Cloth; cl.X = t.X; cl.Z = t.Z - 0.02f; cl.lift = 4.6f * up - 0.35f;
            cl.sprite = m_sp_cloth[t.army]; cl.mpp = 90.0f; cl.wave = 0.55f * wind; cl.shadow = false;
            if (up > 0.95f) out.drawables.push_back(cl);
        }
        if (lantern > 0.05f && up > 0.95f && (t.order % 2 == 0)) { // lit tent doors
            Drawable g; g.kind = DKind::Glow; g.X = t.X; g.Z = t.Z - 0.1f; g.lift = 0.75f; g.col = { 1.0f, 0.72f, 0.38f };
            g.radius = 0.75f; g.intensity = 0.55f * lantern; out.drawables.push_back(g);
            out.lights.push_back({ t.X, t.Z - 1.0f, 0.8f, { 1.0f, 0.72f, 0.38f }, 3.4f, 0.35f * lantern });
        }
    }
    for (Prop const& p : m_props) {
        if (p.sprite < 0) continue;
        Drawable dd; dd.X = p.X; dd.Z = p.Z; dd.sprite = p.sprite; dd.mpp = p.mpp; dd.shadow = p.shadow;
        out.drawables.push_back(dd);
        if (p.lift <= -1000.0f && lantern > 0.05f) { // lantern post light (steady, cooler than fire)
            Drawable g; g.kind = DKind::Glow; g.X = p.X; g.Z = p.Z - 0.02f; g.lift = 1.65f; g.col = { 1.0f, 0.86f, 0.55f };
            g.radius = 0.55f; g.intensity = 0.9f * lantern; g.star = true; out.drawables.push_back(g);
            out.lights.push_back({ p.X, p.Z, 1.6f, { 1.0f, 0.86f, 0.55f }, 4.2f, 0.45f * lantern });
        }
    }
    for (Entity const& e : m_entities) emit_entity(e, hour, anim, d, out);
}

void Scene::entity_poses(float hour, float anim, std::vector<EntityPose>& out) const
{
    Daylight d = compute_daylight(hour);
    Frame tmp;
    out.assign(m_entities.size(), EntityPose {});
    for (size_t i = 0; i < m_entities.size(); ++i) {
        Entity const& e = m_entities[i];
        tmp.clear();
        emit_entity(e, hour, anim, d, tmp);
        EntityPose& p = out[i];
        p.role = e.role; p.army = e.army; p.pair = e.pair;
        for (Drawable const& dr : tmp.drawables)
            if (dr.kind == DKind::Sprite && dr.sprite >= 0) { p.X = dr.X; p.Z = dr.Z; p.lift = dr.lift; p.valid = true; break; }
    }
}

void Scene::emit_entity(Entity const& e, float hour, float anim, Daylight const& d, Frame& out) const
{
    float te = wrap24(hour + e.shift);
    float wind = wind_at(anim);
    float sign = e.army == 0 ? -1.0f : 1.0f;
    float enemy_dir = -sign; // facing toward the opposing army in formation
    auto push = [&](int sprite, float X, float Z, float lift, bool flip, float mpp, bool shadow = true) -> Drawable& {
        Drawable dd; dd.X = X; dd.Z = Z; dd.lift = lift; dd.sprite = sprite; dd.flip = flip; dd.mpp = mpp; dd.shadow = shadow;
        out.drawables.push_back(dd);
        return out.drawables.back();
    };
    auto idle = [&](float seedoff) { return fmodf(anim * 0.21f + hash01(e.seed + uint32_t(seedoff)) * 13.0f, 3.0f) < 2.15f ? int(StandA) : int(StandB); };
    auto walk_pose = [&](float dist) { return int(Walk0) + (int(std::floor(dist / 0.42f)) & 3); };
    auto add_lantern = [&](Drawable& dd, SpriteMeta const& m, bool flip, float mpp, float X, float Z, float lift) {
        if (!m.has_hand) return;
        float dx = (m.hx - m.ax) / mpp * (flip ? -1.0f : 1.0f);
        float lf = lift + (m.ay - m.hy) / mpp;
        dd.lantern_lift = lf; dd.lantern_dx = dx; dd.intensity = d.lantern;
        out.lights.push_back({ X + dx, Z - 0.05f, lf, { 1.0f, 0.86f, 0.56f }, 3.6f, 0.95f * d.lantern });
    };

    switch (e.role) {
    case Soldier:
    case Standard:
    case Officer:
    case Rider: {
        float F = formation_weight(te);
        bool is_officer = e.role == Officer;
        // formation position (officers pace the front line during inspection windows)
        float fx = e.fx, fz = e.fz, insp_dist = 0.0f, insp_dir = 1.0f;
        float insp_w = 0.0f;
        if (is_officer) {
            float u = e.u0 + 6.0f * m_cum_inspect.at(te);
            float zr = e.zmax - e.zmin;
            fz = e.zmin + zr * tri(u);
            insp_dist = u * 2.0f * zr;
            insp_dir = tri_dir(u);
            insp_w = inspect_weight(te);
        }
        // Riders walk ring->picket before the morning march (04:00-04:24) so they join their horse
        // exactly where it stands; every other role rests at its ring slot until it marches out.
        bool rider = e.role == Rider;
        float ab = te >= 12.0f ? ramp(te, 20.3f, 21.1f) : (rider ? 1.0f - ramp(te, 4.0f, 4.4f) : 1.0f);
        float cx = lerpf(e.ax, e.bx, ab), cz = lerpf(e.az, e.bz, ab);
        float px = lerpf(fx, cx, 1.0f - F), pz = lerpf(fz, cz, 1.0f - F);
        float bow = std::sin(float(M_PI) * (1.0f - F)) * e.arc;
        px += bow * 0.0f; pz += bow; // bow sideways in depth so the column is not a straight line
        float dist = std::hypot(cx - fx, cz - fz) * (1.0f - F)
            + std::hypot(e.bx - e.ax, e.bz - e.az) * (te >= 12.0f ? ab : 1.0f - ab) + insp_dist;
        bool marching = F > 0.003f && F < 0.997f;
        bool shuffling = F < 0.003f && ((te >= 20.3f && te <= 21.1f) || (rider && te >= 4.0f && te <= 4.4f));
        bool pacing = is_officer && insp_w > 0.04f && F > 0.997f;
        bool moving = marching || shuffling || pacing;
        float face;
        if (pacing) face = insp_dir > 0 ? 1.0f : -1.0f; // along +Z/-Z: show as facing the enemy-ward/away side
        else if (marching) face = (te >= 12.0f ? (cx - fx) : (fx - cx)) >= 0 ? 1.0f : -1.0f;
        else if (shuffling) face = ((e.bx - e.ax) >= 0) == (te >= 12.0f) ? 1.0f : -1.0f;
        else if (F >= 0.997f) face = enemy_dir;
        else {
            FireDef const& fd = m_fires[size_t(e.army * kFires + e.fire)];
            face = (fd.X - px) >= 0 ? 1.0f : -1.0f;
        }
        if (pacing) face = enemy_dir; // officers look along the line while pacing; keep sprite facing outward
        int pose;
        if (moving) pose = walk_pose(dist);
        else if (F >= 0.997f) {
            pose = idle(1);
            if (e.role == Soldier || e.role == Rider) {
                if (te >= 11.5f && te < 14.0f) pose = (e.sub == 0 || e.sub == 3) ? int(Sit) : (e.sub == 2 ? int(Crouch) : pose);
                else if (te >= 14.0f && te < 16.5f && e.sub == 1) pose = Crouch;
            }
        } else if (F > 0.003f) pose = idle(1);
        else {
            // in camp
            if (te >= 19.0f || te < 4.4f) {
                bool erecting = te < 20.4f && te >= 17.5f && e.sub == 0 && te >= 12.0f;
                if (erecting) pose = Crouch;
                else if (ab < 0.5f && te >= 12.0f) pose = idle(2);
                else if ((te >= 4.3f && te < 4.5f) || (te >= 21.1f && te < 21.25f)) pose = Crouch; // rising / settling
                else pose = (te >= 3.0f && te < 4.4f) ? int(Sit) : ((e.sub == 3 && te < 3.0f) ? idle(3) : int(Sit));
            } else pose = idle(4);
            if (e.role == Officer || e.role == Standard) pose = idle(5);
        }
        int acc = is_officer ? int(AccOfficer) : ((F > 0.3f || e.role == Standard) ? int(AccSpear) : int(AccNone));
        float lift = 0.0f;
        float Z = pz;
        if (e.role == Rider) {
            float M = F > 0.06f ? 1.0f : (F < 0.02f ? 0.0f : (F - 0.02f) / 0.04f);
            if (M > 0.5f) { pose = Ride; acc = AccSpear; }
            lift = 0.87f * M;
            Z -= 0.02f;
        }
        int sid = human(e.army, e.var, acc, pose);
        if (sid < 0) break;
        SpriteMeta const& m = m_atlas.meta(sid);
        Drawable& dd = push(sid, px, Z, lift, face < 0, kHumanMpp);
        (void)m; (void)dd;
        if (e.role == Standard) { // pole + cloth at the bearer's forward hand
            float bx = px + face * 0.38f;
            Drawable& pole = push(m_sp_pole[e.army], bx, Z - 0.01f, 0.0f, false, 43.0f);
            pole.shadow = true;
            Drawable cl; cl.kind = DKind::Cloth; cl.X = bx; cl.Z = Z - 0.015f; cl.lift = 4.45f; cl.sprite = m_sp_cloth[e.army];
            cl.mpp = 43.0f; cl.flip = face < 0; cl.wave = wind; cl.shadow = false;
            out.drawables.push_back(cl);
        }
        break;
    }
    case Sentry: {
        float cx = m_camp_x[e.army], cz = m_camp_z[e.army] + 1.5f;
        float rx = 9.2f, rz = 6.4f;
        float perim = 6.2831853f * std::sqrt((rx * rx + rz * rz) * 0.5f);
        perim = std::round(10.0f * perim / 1.68f) * 1.68f / 10.0f; // whole stride cycles per day => walk phase is seamless at 24:00
        float u = e.u0 + 10.0f * m_cum_patrol.at(te);
        float ang = 6.2831853f * u;
        float px = cx + rx * std::cos(ang), pz = cz + rz * std::sin(ang);
        float tx = -rx * std::sin(ang); // tangent x
        bool moving = patrol_weight(te) > 0.04f;
        float face = tx >= 0 ? 1.0f : -1.0f;
        int acc = d.lantern > 0.2f ? int(AccLantern) : int(AccSpear);
        int pose = moving ? walk_pose(u * perim) : idle(6);
        if (!moving) face = e.army == 0 ? 1.0f : -1.0f;
        int sid = human(e.army, e.var, acc, pose);
        if (sid < 0) break;
        Drawable& dd = push(sid, px, pz, 0.0f, face < 0, kHumanMpp);
        if (acc == AccLantern) add_lantern(dd, m_atlas.meta(sid), face < 0, kHumanMpp, px, pz, 0.0f);
        break;
    }
    case Carrier: {
        float cx = m_camp_x[e.army], cz = m_camp_z[e.army] + 1.8f;
        float rx = 7.4f, rz = 4.4f;
        float perim = 6.2831853f * std::sqrt((rx * rx + rz * rz) * 0.5f);
        perim = std::round(14.0f * perim / 1.68f) * 1.68f / 14.0f;
        float u = e.u0 - 14.0f * m_cum_carry.at(te);
        float ang = 6.2831853f * u;
        float px = cx + rx * std::cos(ang), pz = cz + rz * std::sin(ang);
        float tx = -rx * std::sin(ang) * -1.0f;
        bool active = carry_weight(te) > 0.05f;
        float face = tx >= 0 ? 1.0f : -1.0f;
        int acc = active ? int(AccCrate) : int(AccNone);
        int pose = active ? walk_pose(-u * perim) : int(Sit);
        if (!active) face = (cx - px) >= 0 ? 1.0f : -1.0f;
        int sid = human(e.army, e.var, acc, pose);
        if (sid < 0) break;
        push(sid, px, pz, 0.0f, face < 0, kHumanMpp);
        break;
    }
    case Attendant: {
        FireDef const& fd = m_fires[size_t(e.army * kFires + e.fire)];
        float lvl = fire_level(hour, fd.off);
        int pose = lvl > 0.15f ? (fmodf(anim * 0.5f + hash01(e.seed) * 9.0f, 7.0f) < 5.5f ? int(Crouch) : idle(8)) : idle(8);
        float face = (fd.X - e.ax) >= 0 ? 1.0f : -1.0f;
        int sid = human(e.army, e.var, AccNone, pose);
        if (sid < 0) break;
        push(sid, e.ax, e.az, 0.0f, face < 0, kHumanMpp);
        break;
    }
    case Horse: {
        float px, pz, face, dist = 0.0f;
        bool moving = false;
        bool draft = e.pair == -2;
        float F = draft ? 0.0f : formation_weight(te);
        if (draft) { px = e.ax; pz = e.az; face = e.army == 0 ? 1.0f : -1.0f; }
        else {
            px = lerpf(e.fx, e.ax, 1.0f - F); pz = lerpf(e.fz, e.az, 1.0f - F) + std::sin(float(M_PI) * (1.0f - F)) * e.arc;
            dist = std::hypot(e.ax - e.fx, e.az - e.fz) * (1.0f - F);
            moving = F > 0.003f && F < 0.997f;
            if (moving) face = (te >= 12.0f ? (e.ax - e.fx) : (e.fx - e.ax)) >= 0 ? 1.0f : -1.0f;
            else face = F >= 0.997f ? -sign : (e.army == 0 ? 1.0f : -1.0f);
        }
        int pose;
        if (moving) pose = int(HWalk0) + (int(std::floor(dist / 0.62f)) & 3);
        else {
            bool graze = (!draft && F < 0.003f && (e.sub % 2 == 0)) || (te >= 12.3f && te < 14.3f && (e.sub % 2 == 0)) || (draft && (e.sub % 2 == 0));
            if (graze) pose = fmodf(anim * 0.07f + hash01(e.seed) * 5.0f, 4.0f) < 3.0f ? int(HGraze) : int(HStandA);
            else pose = fmodf(anim * 0.17f + hash01(e.seed + 3) * 11.0f, 3.0f) < 2.2f ? int(HStandA) : int(HStandB);
        }
        int bl = draft ? 0 : (e.army + 1);
        int sid = m_horse[e.coat][bl][pose];
        if (sid < 0) break;
        push(sid, px, pz, 0.0f, face < 0, kHorseMpp);
        break;
    }
    }
}

} // namespace lb
