#!/usr/bin/env python3
"""Seeded, reproducible asset generator for Exile OS Living Battlefield.

Everything is drawn procedurally (numpy + Pillow); no third-party artwork, no AI
image model, no photographs. Output is stylised painterly concept art, NOT
photorealism, and is not historical imagery.

Outputs (into ../assets):
  sky_clouds.png    horizontally tileable cloud alpha layer
  hills_far.png     distant ridge, neutral albedo + alpha
  hills_near.png    nearer ridge, neutral albedo + alpha
  ground.png        battlefield ground albedo, authored in ground-plane
                    coordinates and projected with the SAME camera the
                    renderer uses (see CAM_* below / src/scene.h)
  sprites.png       texture atlas (people, horses, tents, props)
  sprites.json      atlas rectangles, anchors, hand/lantern points

All colours are neutral/unlit "albedo"; the renderer applies time-of-day light.
"""
import json, math, os, sys
import numpy as np
from PIL import Image, ImageDraw, ImageFilter

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets")
os.makedirs(OUT, exist_ok=True)

W, H = 1920, 1080
HORIZON = 430
# Camera (must match src/scene.h): pinhole above the field.
CAM_F, CAM_H = 1000.0, 8.0
SEED = 20261022

# ----------------------------------------------------------------- noise ----
def _hash(ix, iy, seed):
    h = (ix.astype(np.int64) * 374761393 + iy.astype(np.int64) * 668265263 + seed * 1442695041) & 0xFFFFFFFF
    h = ((h ^ (h >> 13)) * 1274126177) & 0xFFFFFFFF
    h = h ^ (h >> 16)
    return (h & 0xFFFFFF).astype(np.float64) / float(0xFFFFFF)

def vnoise(x, y, seed):
    xi = np.floor(x); yi = np.floor(y)
    fx = x - xi; fy = y - yi
    fx = fx * fx * (3 - 2 * fx); fy = fy * fy * (3 - 2 * fy)
    a = _hash(xi, yi, seed); b = _hash(xi + 1, yi, seed)
    c = _hash(xi, yi + 1, seed); d = _hash(xi + 1, yi + 1, seed)
    return (a * (1 - fx) + b * fx) * (1 - fy) + (c * (1 - fx) + d * fx) * fy

def fbm(x, y, seed, octaves=5, gain=0.5, period_x=None):
    """Fractal value noise in [0,1]. period_x makes x wrap (integer lattice period)."""
    tot = 0.0; amp = 1.0; norm = 0.0; fx = 1.0
    for o in range(octaves):
        xx = x * fx
        if period_x is not None:
            p = max(1, int(round(period_x * fx)))
            xx = np.mod(xx, p)
            xi = np.floor(xx); fr = xx - xi
            fr = fr * fr * (3 - 2 * fr)
            yy = y * fx; yi = np.floor(yy); fy = yy - yi; fy = fy * fy * (3 - 2 * fy)
            x0 = xi % p; x1 = (xi + 1) % p
            a = _hash(x0, yi, seed + o); b = _hash(x1, yi, seed + o)
            c = _hash(x0, yi + 1, seed + o); d = _hash(x1, yi + 1, seed + o)
            n = (a * (1 - fr) + b * fr) * (1 - fy) + (c * (1 - fr) + d * fr) * fy
        else:
            n = vnoise(xx, y * fx, seed + o)
        tot += n * amp; norm += amp; amp *= gain; fx *= 2.0
    return tot / norm

def to_img(arr_rgb=None, arr_a=None):
    if arr_a is None:
        return Image.fromarray(np.clip(arr_rgb * 255 + 0.5, 0, 255).astype(np.uint8), "RGB")
    out = np.dstack([np.clip(arr_rgb, 0, 1), np.clip(arr_a, 0, 1)])
    return Image.fromarray((out * 255 + 0.5).astype(np.uint8), "RGBA")

def save(im, name):
    p = os.path.join(OUT, name); im.save(p, optimize=True); print("wrote", name, im.size)

# ---------------------------------------------------------------- clouds ----
def gen_clouds():
    w, h = 1920, 430
    yy, xx = np.mgrid[0:h, 0:w].astype(np.float64)
    u = xx / w * 6.0; v = yy / h * 3.0
    n = fbm(u, v * 1.0, SEED + 11, octaves=6, period_x=6)
    n2 = fbm(u * 0.5 + 3, v * 0.7, SEED + 17, octaves=4, period_x=3)
    dens = np.clip((n * 0.7 + n2 * 0.5 - 0.62) * 3.2, 0, 1)
    # fewer clouds near the horizon, thinning upward fade
    fade = np.clip(1.0 - yy / h * 0.55, 0, 1) * np.clip(yy / 60.0, 0, 1)
    a = dens * fade * 0.85
    shade = 0.78 + 0.22 * np.clip((fbm(u * 2, v * 2, SEED + 23, 4, period_x=12)), 0, 1)
    rgb = np.dstack([shade, shade, shade * 1.02])
    save(to_img(rgb, a), "sky_clouds.png")

# ----------------------------------------------------------------- hills ----
def ridge(width, seed, base, amp, rough):
    x = np.arange(width, dtype=np.float64)
    prof = np.zeros(width)
    a = amp
    for k in range(6):
        f = (2 ** k) / width * 2.4
        ph = np.random.default_rng(seed + k).uniform(0, 6.28)
        prof += a * np.sin(x * f * 6.28318 + ph) * (0.7 + 0.3 * np.sin(x * f * 1.7 + ph))
        a *= rough
    prof += (fbm(x * 0.02, np.zeros_like(x), seed, 4) - 0.5) * amp * 0.5
    return base - prof

def gen_hills(name, h, base, amp, rough, seed, col_top, col_bot, trees):
    w = W
    prof = ridge(w, seed, base, amp, rough)
    yy, xx = np.mgrid[0:h, 0:w].astype(np.float64)
    inside = (yy >= prof[None, :]).astype(np.float64)
    # soften edge by 1.5px
    edge = np.clip((yy - prof[None, :]) / 1.5 + 0.5, 0, 1)
    t = np.clip((yy - prof[None, :]) / 120.0, 0, 1)
    n = fbm(xx / 60.0, yy / 40.0, seed + 5, 5)
    n2 = fbm(xx / 8.0, yy / 8.0, seed + 9, 3)
    top = np.array(col_top); bot = np.array(col_bot)
    rgb = top[None, None, :] * (1 - t[..., None]) + bot[None, None, :] * t[..., None]
    rgb = rgb * (0.85 + 0.3 * n[..., None]) * (0.94 + 0.12 * n2[..., None])
    # forested specks along upper slopes
    if trees:
        sp = fbm(xx / 5.0, yy / 6.0, seed + 31, 3)
        mask = ((sp > 0.58) & (t < 0.6)).astype(np.float64)
        rgb = rgb * (1 - 0.35 * mask[..., None])
    # subtle ridge-light rim
    rim = np.exp(-np.clip(yy - prof[None, :], 0, 50) / 4.0)
    rgb = np.clip(rgb + rim[..., None] * 0.05, 0, 1)
    save(to_img(rgb, edge), name)

# ---------------------------------------------------------------- ground ----
def gen_ground():
    h = H - HORIZON  # 650 rows below horizon
    yy, xx = np.mgrid[0:h, 0:W].astype(np.float64)
    rr = yy + 1.0                                  # pixels below horizon
    z = CAM_F * CAM_H / rr                         # depth (m)
    X = (xx - W / 2.0) * z / CAM_F                 # lateral (m)
    # --- coloured terrain ---
    big = fbm(X * 0.035 + 7, z * 0.035, SEED + 101, 5)
    mid = fbm(X * 0.22, z * 0.22, SEED + 103, 5)
    fine = fbm(X * 2.2, z * 2.2, SEED + 107, 4)
    grit = fbm(X * 9.0, z * 9.0, SEED + 109, 3)
    grass = np.array([0.30, 0.38, 0.22]); dry = np.array([0.50, 0.45, 0.28])
    dirtc = np.array([0.36, 0.28, 0.19]); steel = np.array([0.40, 0.42, 0.42])
    m = np.clip(big * 1.5 - 0.35 + (mid - 0.5) * 0.5, 0, 1)
    rgb = grass[None, None, :] * (1 - m[..., None]) + dry[None, None, :] * m[..., None]
    # trampled central field + churned ground under the two armies
    field_c = np.exp(-(X / 34.0) ** 2) * np.exp(-((z - 58.0) / 40.0) ** 2)
    rgb = rgb * (1 - 0.45 * field_c[..., None]) + dirtc[None, None, :] * 0.45 * field_c[..., None]
    # tracks: from each army block down to its camp
    def path(x0, z0, x1, z1, wm):
        dx, dz = x1 - x0, z1 - z0; L2 = dx * dx + dz * dz
        t = np.clip(((X - x0) * dx + (z - z0) * dz) / L2, 0, 1)
        px = x0 + t * dx + (fbm(t * 4, 0 * t, SEED + 7, 3) - 0.5) * 4.0
        pz = z0 + t * dz
        dist = np.sqrt((X - px) ** 2 + (z - pz) ** 2)
        return np.clip(1.0 - dist / wm, 0, 1) ** 1.5
    tracks = np.zeros_like(X)
    for (x0, z0, x1, z1) in [(-34, 56, -20, 26), (34, 56, 20, 26), (-20, 26, 20, 26), (-34, 70, -48, 34), (34, 70, 48, 34)]:
        tracks = np.maximum(tracks, path(x0, z0, x1, z1, 2.2))
    rgb = rgb * (1 - 0.7 * tracks[..., None]) + dirtc[None, None, :] * 0.7 * tracks[..., None] * (0.8 + 0.4 * fine[..., None])
    # camp ground: trodden mud ellipses
    for cx in (-20.0, 20.0):
        camp = np.exp(-(((X - cx) / 13.0) ** 2 + ((z - 26.0) / 9.0) ** 2))
        rgb = rgb * (1 - 0.55 * camp[..., None]) + dirtc[None, None, :] * 0.55 * camp[..., None]
    # rocky patches
    rock = np.clip((fbm(X * 0.12 + 40, z * 0.12, SEED + 111, 4) - 0.64) * 6, 0, 1)
    rgb = rgb * (1 - 0.5 * rock[..., None]) + steel[None, None, :] * 0.5 * rock[..., None]
    # texture
    rgb = rgb * (0.80 + 0.35 * fine[..., None]) * (0.92 + 0.16 * grit[..., None])
    # grass blades read at near range only (scale with proximity)
    near = np.clip((z - 3.0) / -20.0 + 1.0, 0, 1)
    rgb = rgb * (1 - 0.18 * near[..., None] * (grit[..., None] > 0.55))
    # bake a very soft long-range tonal falloff so the far field is not flat
    rgb = np.clip(rgb, 0, 1)
    save(to_img(rgb), "ground.png")

# --------------------------------------------------------------- sprites ----
SS = 4
OX = 0
def col(r, g, b): return (int(r * 255), int(g * 255), int(b * 255), 255)
def shade_c(c, k): return (min(255, int(c[0] * k)), min(255, int(c[1] * k)), min(255, int(c[2] * k)), 255)

ARMY = [
    dict(tunic=col(0.075, 0.150, 0.330), trim=col(0.84, 0.83, 0.78), armor=col(0.50, 0.52, 0.53), cloak=col(0.06, 0.10, 0.22)),   # West: deep blue / white
    dict(tunic=col(0.040, 0.285, 0.195), trim=col(0.07, 0.07, 0.08), armor=col(0.46, 0.48, 0.48), cloak=col(0.03, 0.19, 0.13)),   # East: emerald / black
]
SKINS = [col(0.80, 0.62, 0.48), col(0.62, 0.44, 0.32), col(0.45, 0.30, 0.22), col(0.86, 0.70, 0.58)]
LEATHER = col(0.30, 0.20, 0.12); WOOD = col(0.40, 0.28, 0.16); STEEL = col(0.62, 0.64, 0.66)

def limb(o, a1, a2, l1, l2):
    a1r = math.radians(a1); a2r = math.radians(a1 + a2)
    j = (o[0] + l1 * math.sin(a1r), o[1] + l1 * math.cos(a1r))
    e = (j[0] + l2 * math.sin(a2r), j[1] + l2 * math.cos(a2r))
    return j, e

def tline(d, p0, p1, w, fill):
    d.line([((p0[0] + OX) * SS, p0[1] * SS), ((p1[0] + OX) * SS, p1[1] * SS)], width=int(w * SS), fill=fill)
    r = w * SS / 2.0
    for p in (p0, p1):
        d.ellipse([(p[0] + OX) * SS - r, p[1] * SS - r, (p[0] + OX) * SS + r, p[1] * SS + r], fill=fill)

def tpoly(d, pts, fill): d.polygon([((x + OX) * SS, y * SS) for x, y in pts], fill=fill)
def tell(d, c, rx, ry, fill): d.ellipse([(c[0] + OX - rx) * SS, (c[1] - ry) * SS, (c[0] + OX + rx) * SS, (c[1] + ry) * SS], fill=fill)

_FINISH_COUNTER = [0]

def finish(im, w, h, light=True):
    """Downsample, then give the flat vector drawing a natural material read:
    fabric/leather grain, folds, form shading from a height field, ambient
    occlusion near silhouette edges, muted colour and soft edges. (Procedural
    approximation of natural lighting; not photography.)"""
    _FINISH_COUNTER[0] += 1
    seed = SEED + 1000 + _FINISH_COUNTER[0] * 13
    im = im.resize((w, h), Image.LANCZOS)
    a = np.asarray(im).astype(np.float64) / 255.0
    alpha = a[..., 3]
    if light:
        yy, xx = np.mgrid[0:h, 0:w].astype(np.float64)
        grain = fbm(xx / 1.7, yy / 1.7, seed, 3)               # fibre / pitting
        folds = fbm(xx / 7.0, yy / 2.2, seed + 3, 4)             # cloth wrinkles (stretched)
        wear = fbm(xx / 14.0, yy / 14.0, seed + 5, 3)            # large stains / wear
        tex = (0.86 + 0.28 * grain) * (0.90 + 0.20 * folds) * (0.93 + 0.14 * wear)
        # form shading from blurred alpha as a pseudo height field
        hm = np.asarray(Image.fromarray((alpha * 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(max(1.5, min(w, h) / 22.0)))).astype(np.float64) / 255.0
        gy_, gx_ = np.gradient(hm)
        nz = 1.0
        nx_, ny_ = -gx_ * 9.0, -gy_ * 9.0
        inv = 1.0 / np.sqrt(nx_ * nx_ + ny_ * ny_ + nz * nz)
        lx, ly, lz = -0.45, -0.65, 0.62
        diff = np.clip((nx_ * lx + ny_ * ly + nz * lz) * inv, 0.0, 1.0)
        form = 0.62 + 0.62 * diff
        vert = np.linspace(1.06, 0.84, h)[:, None]
        # ambient occlusion: darker toward the middle of thin shapes' lower/inner regions
        blur2 = np.asarray(Image.fromarray((alpha * 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(2.2))).astype(np.float64) / 255.0
        edge = np.clip(1.0 - (alpha - blur2) * 1.6, 0.5, 1.0)
        k = tex * form * vert * edge
        rgb = a[..., :3]
        lum = (rgb[..., 0] * 0.3 + rgb[..., 1] * 0.59 + rgb[..., 2] * 0.11)[..., None]
        rgb = lum + (rgb - lum) * 0.84                           # muted, less "toy" saturation
        a[..., :3] = np.clip(rgb * k[..., None], 0, 1)
    # softer, anti-aliased silhouette
    soft = np.asarray(Image.fromarray((alpha * 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(0.55))).astype(np.float64) / 255.0
    a[..., 3] = np.clip(np.maximum(soft, alpha * 0.92), 0, 1)
    return Image.fromarray((a * 255 + 0.5).astype(np.uint8), "RGBA")

POSES = {
    #            pelvis      lean   legF        legB         armF        armB
    "stand_a": ((48, 86), 0,   (3, 0),    (-3, 0),    (10, 12),   (-4, 8)),
    "stand_b": ((48, 87), -2,  (7, -2),   (-7, 0),    (6, 10),    (-8, 6)),
    "walk0":   ((48, 85), 2,   (26, 0),   (-22, -10), (-22, 10),  (24, 12)),
    "walk1":   ((48, 87), 2,   (4, -34),  (0, 0),     (-4, 10),   (4, 12)),
    "walk2":   ((48, 85), 2,   (-22, -10), (26, 0),   (24, 12),   (-22, 10)),
    "walk3":   ((48, 87), 2,   (0, 0),    (4, -34),   (4, 12),    (-4, 10)),
    "sit":     ((44, 146), 6,  (105, -165), (95, -150), (30, 80),   (14, 50)),
    "crouch":  ((44, 112), 18, (50, -95), (30, -100), (66, 14),  (40, 22)),
    "ride":    ((48, 104), 0,  (80, -84), (70, -92),  (22, 40),   (18, 44)),
}

def draw_human(pose, army, var, acc):
    """acc in {'spear','crate','lantern','officer','none'} -> (Image 96x160, extras)"""
    pel, lean, legF, legB, armF, armB = POSES[pose]
    if acc == "lantern":
        armF = (100, 40)      # front arm raised, forearm up: lantern held at head height
    pal = ARMY[army]
    tunic = shade_c(pal["tunic"], [1.0, 0.88, 1.12][var % 3])
    skin = SKINS[(var * 3 + army) % len(SKINS)]
    global OX
    OX = 16
    im = Image.new("RGBA", (128 * SS, 160 * SS), (0, 0, 0, 0)); d = ImageDraw.Draw(im)
    lr = math.radians(lean); torso = 40.0
    neck = (pel[0] + math.sin(lr) * torso, pel[1] - math.cos(lr) * torso)
    sh = (pel[0] + math.sin(lr) * 35.0, pel[1] - math.cos(lr) * 35.0)
    head = (neck[0] + math.sin(lr) * 10, neck[1] - math.cos(lr) * 10 - 1)
    trouser = shade_c(LEATHER, 0.8)
    extras = {}
    # officer cloak (behind)
    if acc == "officer" or var == 2:
        c = pal["cloak"]
        tpoly(d, [(sh[0] - 2, sh[1] - 3), (sh[0] + 3, sh[1] + 2), (sh[0] - 10, sh[1] + 62), (sh[0] - 26, sh[1] + 58)], c)
    # back limbs
    jb, hb = limb((sh[0] - 1, sh[1] + 2), armB[0], armB[1], 22, 20)
    tline(d, (sh[0] - 1, sh[1] + 2), jb, 6, shade_c(tunic, 0.78)); tline(d, jb, hb, 5, shade_c(tunic, 0.78)); tell(d, hb, 2.8, 2.8, skin)
    kb, fb = limb((pel[0] - 2, pel[1]), legB[0], legB[1], 32, 32)
    tline(d, (pel[0] - 2, pel[1]), kb, 9, shade_c(trouser, 0.8)); tline(d, kb, fb, 8, shade_c(trouser, 0.8)); tpoly(d, [(fb[0] - 4, fb[1] - 3), (fb[0] + 8, fb[1] - 2), (fb[0] + 8, fb[1] + 3), (fb[0] - 4, fb[1] + 3)], shade_c(LEATHER, 0.6))
    # weapon in back hand (spear)
    if acc in ("spear", "lantern"):
        tline(d, (hb[0] - 1, hb[1] + 26), (hb[0] + 1, hb[1] - 98), 2.2, WOOD)
        tpoly(d, [(hb[0] - 3, hb[1] - 98), (hb[0] + 5, hb[1] - 98), (hb[0] + 1, hb[1] - 116)], STEEL)
    # tunic body + skirt
    tpoly(d, [(sh[0] - 11, sh[1] - 2), (sh[0] + 11, sh[1] - 2), (pel[0] + 10, pel[1] + 4), (pel[0] - 10, pel[1] + 4)], tunic)
    tpoly(d, [(pel[0] - 10, pel[1] + 2), (pel[0] + 10, pel[1] + 2), (pel[0] + 12, pel[1] + 30), (pel[0] - 12, pel[1] + 30)], shade_c(tunic, 0.93))
    # cuirass / trim
    tpoly(d, [(sh[0] - 9, sh[1] - 1), (sh[0] + 9, sh[1] - 1), (sh[0] + 8, sh[1] + 20), (sh[0] - 8, sh[1] + 20)], pal["armor"] if var != 1 else shade_c(LEATHER, 1.1))
    tline(d, (pel[0] - 10, pel[1] + 2), (pel[0] + 10, pel[1] + 2), 3, shade_c(LEATHER, 0.55))
    tline(d, (sh[0] - 9, sh[1] + 22), (sh[0] + 9, sh[1] + 22), 2, pal["trim"])
    # front leg
    kf, ff = limb((pel[0] + 2, pel[1]), legF[0], legF[1], 32, 32)
    tline(d, (pel[0] + 2, pel[1]), kf, 10, trouser); tline(d, kf, ff, 8.5, trouser)
    tpoly(d, [(ff[0] - 4, ff[1] - 3), (ff[0] + 9, ff[1] - 2), (ff[0] + 9, ff[1] + 3), (ff[0] - 4, ff[1] + 3)], shade_c(LEATHER, 0.65))
    # head + helmet
    tell(d, head, 8.2, 9.0, skin)
    tell(d, (head[0] + 3.2, head[1] + 1), 1.0, 1.0, shade_c(skin, 0.45))
    if var == 0 or acc == "officer":
        tpoly(d, [(head[0] - 9, head[1] - 1), (head[0] + 10, head[1] - 1), (head[0] + 7, head[1] - 12), (head[0], head[1] - 15), (head[0] - 8, head[1] - 10)], STEEL)
        tline(d, (head[0] - 9, head[1] - 1), (head[0] + 11, head[1] - 1), 2.4, shade_c(STEEL, 0.8))
        if acc == "officer":
            tpoly(d, [(head[0] - 2, head[1] - 14), (head[0] + 2, head[1] - 14), (head[0] - 9, head[1] - 27), (head[0] - 14, head[1] - 19)], pal["trim"])
    elif var == 1:
        tpoly(d, [(head[0] - 9, head[1] + 3), (head[0] + 9, head[1] + 1), (head[0] + 8, head[1] - 10), (head[0], head[1] - 13), (head[0] - 9, head[1] - 7)], shade_c(LEATHER, 0.9))
    else:
        tpoly(d, [(head[0] - 9, head[1] - 1), (head[0] + 8, head[1] - 3), (head[0] + 5, head[1] - 12), (head[0] - 5, head[1] - 12)], shade_c(pal["cloak"], 1.2))
    # front arm
    jf, hf = limb((sh[0] + 1, sh[1] + 2), armF[0], armF[1], 22, 20)
    tline(d, (sh[0] + 1, sh[1] + 2), jf, 7, tunic); tline(d, jf, hf, 5.5, shade_c(tunic, 1.05)); tell(d, hf, 3.0, 3.0, skin)
    if acc in ("spear", "lantern") and acc == "spear":
        # round shield on front forearm
        sc = (jf[0] + (hf[0] - jf[0]) * 0.5 + 5, jf[1] + (hf[1] - jf[1]) * 0.5)
        tell(d, sc, 13, 15, shade_c(pal["tunic"], 1.05)); tell(d, sc, 13, 15, None) if False else None
        tell(d, sc, 11, 13, pal["tunic"]); tell(d, sc, 5.5, 6.5, pal["trim"]); tell(d, sc, 2.5, 2.5, shade_c(STEEL, 0.9))
    if acc == "crate":
        cx, cy = hf[0] + 5, hf[1] - 6
        tpoly(d, [(cx - 14, cy - 10), (cx + 14, cy - 10), (cx + 14, cy + 12), (cx - 14, cy + 12)], shade_c(WOOD, 1.1))
        tline(d, (cx - 14, cy - 10), (cx + 14, cy + 12), 1.6, shade_c(WOOD, 0.7)); tline(d, (cx - 14, cy + 12), (cx + 14, cy - 10), 1.6, shade_c(WOOD, 0.7))
        tpoly(d, [(cx - 14, cy - 10), (cx + 14, cy - 10), (cx + 14, cy - 6), (cx - 14, cy - 6)], shade_c(WOOD, 0.85))
    if acc == "officer":
        tline(d, (pel[0] - 12, pel[1] + 4), (pel[0] - 22, pel[1] + 34), 2.4, shade_c(LEATHER, 0.7))
        tline(d, (pel[0] - 10, pel[1] + 4), (pel[0] - 20, pel[1] + 30), 2.0, STEEL)
    if acc == "lantern":
        # raised lantern: front arm lifted, lantern on a short hook above the hand
        lx, ly = hf[0] + 1, hf[1] - 5
        tline(d, (hf[0], hf[1]), (lx, ly - 3), 1.4, shade_c(LEATHER, 0.5))
        tpoly(d, [(lx - 5.5, ly), (lx + 5.5, ly), (lx + 4.5, ly + 13), (lx - 4.5, ly + 13)], shade_c(STEEL, 0.45))
        tpoly(d, [(lx - 3.4, ly + 2), (lx + 3.4, ly + 2), (lx + 2.8, ly + 11), (lx - 2.8, ly + 11)], col(1.0, 0.88, 0.50))
        extras["lantern"] = (lx + OX, ly + 6.5)
    OX = 0
    return finish(im, 128, 160), extras

def horse_cell(pose, coat, blanket):
    CW, CH = 224, 150
    im = Image.new("RGBA", (CW * SS, CH * SS), (0, 0, 0, 0)); d = ImageDraw.Draw(im)
    body = {"bay": col(0.34, 0.19, 0.10), "grey": col(0.64, 0.64, 0.62), "black": col(0.11, 0.11, 0.12)}[coat]
    dark = shade_c(body, 0.55); light = shade_c(body, 1.25)
    gy = 142
    leg_ph = {"stand_a": [(0, 0), (4, 0), (-2, 0), (6, 0)], "stand_b": [(2, 0), (-2, 0), (0, 0), (4, 0)],
              "graze": [(0, 0), (4, 0), (-2, 0), (6, 0)],
              "walk0": [(20, 0), (-14, -18), (-18, -18), (16, 0)], "walk1": [(4, -30), (4, 0), (-6, 0), (4, -30)],
              "walk2": [(-14, -18), (20, 0), (16, 0), (-18, -18)], "walk3": [(4, 0), (4, -30), (4, -30), (-6, 0)]}[pose]
    # (front far, front near, back far, back near) x positions on body underside
    anchors = [(150, 98), (138, 98), (66, 96), (56, 96)]
    order = [0, 2, 1, 3]
    for i in order:
        ax, ay = anchors[i]; a1 = leg_ph[i][0]; a2 = leg_ph[i][1]
        k, f = limb((ax, ay), a1 * 0.6, a2 * 0.6, 22, 22)
        f2 = (f[0] + (0), min(gy, f[1] + 1))
        tline(d, (ax, ay), k, 9 if i >= 2 else 7.5, dark if i in (0, 2) else body)
        tline(d, k, f2, 6 if i >= 2 else 5, dark if i in (0, 2) else body)
        tpoly(d, [(f2[0] - 4, f2[1] - 4), (f2[0] + 4, f2[1] - 4), (f2[0] + 5, f2[1] + 3), (f2[0] - 5, f2[1] + 3)], shade_c(col(0.15, 0.12, 0.10), 1.0))
    # body
    tell(d, (104, 84), 56, 27, body); tell(d, (146, 78), 25, 28, body); tell(d, (66, 82), 26, 26, shade_c(body, 0.95))
    # neck + head
    headdown = pose == "graze"
    if headdown:
        tpoly(d, [(150, 62), (166, 66), (188, 118), (172, 124)], body)
        hx, hy = 184, 128; tell(d, (hx, hy), 11, 20, body); tpoly(d, [(hx - 5, hy - 10), (hx + 5, hy - 10), (hx + 3, hy + 22), (hx - 5, hy + 20)], body)
        tline(d, (152, 58), (174, 110), 5, dark)
    else:
        tpoly(d, [(146, 62), (166, 56), (186, 30), (172, 24)], body)
        hx, hy = 188, 28
        tpoly(d, [(hx - 8, hy - 10), (hx + 10, hy - 4), (hx + 22, hy + 14), (hx + 14, hy + 20), (hx - 4, hy + 10)], body)
        tpoly(d, [(hx - 6, hy - 10), (hx - 3, hy - 18), (hx, hy - 10)], dark)
        tline(d, (148, 56), (176, 18), 5, dark)
    # tail
    ts = 4 if pose in ("stand_b", "walk1", "walk3") else 0
    tline(d, (44, 70), (32 - ts, 100), 7, dark); tline(d, (32 - ts, 100), (28 - ts * 2, 128), 5, dark)
    # blanket + saddle
    if blanket >= 0:
        pal = ARMY[blanket]
        tpoly(d, [(92, 56), (126, 56), (128, 96), (88, 96)], pal["tunic"]); tline(d, (90, 94), (126, 94), 3, pal["trim"])
        tpoly(d, [(96, 50), (118, 50), (120, 60), (94, 60)], shade_c(LEATHER, 0.9))
    return finish(im, CW, CH)

def tent_cell(kind, trim):
    CW, CH = (190, 140) if kind != "pavilion" else (210, 190)
    im = Image.new("RGBA", (CW * SS, CH * SS), (0, 0, 0, 0)); d = ImageDraw.Draw(im)
    canvas = col(0.82, 0.77, 0.64); canvas2 = shade_c(canvas, 0.74); tr = ARMY[trim]["tunic"] if trim >= 0 else col(0.45, 0.33, 0.20)
    if kind == "aframe":
        # two roof planes (left lit, right shaded) + dark door + ridge
        tpoly(d, [(10, 132), (92, 18), (110, 22), (60, 134)], canvas)
        tpoly(d, [(92, 18), (110, 22), (182, 132), (110, 134)], canvas2)
        tpoly(d, [(60, 134), (110, 22), (110, 134)], shade_c(canvas, 0.62))
        tpoly(d, [(78, 134), (100, 76), (110, 134)], col(0.08, 0.07, 0.06))
        tline(d, (92, 18), (110, 22), 3, shade_c(WOOD, 0.8)); tline(d, (10, 132), (92, 18), 2, tr); tline(d, (110, 22), (182, 132), 2, tr)
        for p in ((6, 138), (186, 138), (60, 138)): tline(d, p, (p[0], p[1] - 6), 2, WOOD)
    elif kind == "pavilion":
        tpoly(d, [(15, 176), (30, 100), (105, 22), (180, 100), (195, 176)], canvas)
        tpoly(d, [(105, 22), (180, 100), (195, 176), (112, 176)], canvas2)
        tpoly(d, [(15, 176), (195, 176), (190, 150), (20, 150)], tr)
        tpoly(d, [(86, 176), (98, 118), (126, 118), (136, 176)], col(0.08, 0.07, 0.06))
        tline(d, (105, 22), (105, 2), 2.5, WOOD)
        tpoly(d, [(105, 4), (138, 12), (105, 22)], tr)
    else:  # lean-to / shelter
        tpoly(d, [(14, 132), (24, 52), (176, 36), (180, 132)], canvas)
        tpoly(d, [(24, 52), (176, 36), (176, 52), (24, 66)], tr)
        for x in (18, 174): tline(d, (x, 132), (x, 48), 3, WOOD)
        tpoly(d, [(26, 132), (30, 70), (170, 62), (174, 132)], shade_c(canvas2, 0.55))
    return finish(im, CW, CH)

def prop(name):
    if name == "campfire":
        im = Image.new("RGBA", (100 * SS, 50 * SS), (0, 0, 0, 0)); d = ImageDraw.Draw(im)
        tell(d, (50, 32), 40, 11, col(0.20, 0.19, 0.18)); tell(d, (50, 31), 32, 8, col(0.07, 0.06, 0.05))
        for a, l in ((-30, 22), (25, 22), (70, 20), (-75, 18), (115, 20)):
            r = math.radians(a); p = (50 + math.cos(r) * l * 1.3, 31 + math.sin(r) * l * 0.3)
            tline(d, (50, 30), p, 5, shade_c(WOOD, 0.6))
        for i in range(10):
            a = i / 10 * 6.283; tell(d, (50 + math.cos(a) * 39, 32 + math.sin(a) * 10), 4.5, 3.2, col(0.42, 0.41, 0.40))
        return finish(im, 100, 50), (50, 40)
    if name == "wagon":
        im = Image.new("RGBA", (220 * SS, 150 * SS), (0, 0, 0, 0)); d = ImageDraw.Draw(im)
        tpoly(d, [(30, 100), (190, 100), (194, 118), (26, 118)], shade_c(WOOD, 0.9))
        tpoly(d, [(34, 98), (50, 42), (172, 42), (188, 98)], col(0.84, 0.80, 0.68))
        tpoly(d, [(120, 42), (172, 42), (188, 98), (120, 98)], col(0.64, 0.60, 0.50))
        tline(d, (210, 112), (190, 106), 4, WOOD)
        for cx in (62, 158):
            tell(d, (cx, 124), 20, 20, shade_c(WOOD, 0.75)); tell(d, (cx, 124), 15, 15, None) if False else None
            tell(d, (cx, 124), 14, 14, col(0.10, 0.08, 0.06)); tell(d, (cx, 124), 4, 4, shade_c(WOOD, 0.9))
            for a in range(0, 360, 45):
                r = math.radians(a); tline(d, (cx, 124), (cx + math.cos(r) * 14, 124 + math.sin(r) * 14), 1.5, shade_c(WOOD, 0.9))
        return finish(im, 220, 150), (110, 144)
    if name in ("crates", "barrels", "firewood"):
        im = Image.new("RGBA", (90 * SS, 64 * SS), (0, 0, 0, 0)); d = ImageDraw.Draw(im)
        if name == "crates":
            for (x, y, s) in ((8, 36, 26), (36, 38, 24), (22, 12, 26)):
                tpoly(d, [(x, y), (x + s, y), (x + s, y + s), (x, y + s)], shade_c(WOOD, 1.05))
                tline(d, (x, y), (x + s, y + s), 1.6, shade_c(WOOD, 0.65)); tline(d, (x + s, y), (x, y + s), 1.6, shade_c(WOOD, 0.65))
        elif name == "barrels":
            for (x, y) in ((14, 30), (40, 30), (27, 6)):
                tell(d, (x + 11, y + 14), 11, 15, shade_c(WOOD, 1.0)); tline(d, (x + 1, y + 8), (x + 21, y + 8), 2, col(0.2, 0.2, 0.2)); tline(d, (x + 1, y + 21), (x + 21, y + 21), 2, col(0.2, 0.2, 0.2))
        else:
            for r in range(3):
                for c in range(5 - r):
                    tell(d, (12 + c * 14 + r * 7, 54 - r * 12), 7, 6, shade_c(WOOD, 0.9 + 0.1 * ((r + c) % 2))); tell(d, (12 + c * 14 + r * 7, 54 - r * 12), 3.4, 3.0, col(0.62, 0.48, 0.30))
        return finish(im, 90, 64), (45, 62)
    if name.startswith("pole"):
        a = 0 if name == "pole_w" else 1
        im = Image.new("RGBA", (24 * SS, 200 * SS), (0, 0, 0, 0)); d = ImageDraw.Draw(im)
        tline(d, (12, 198), (12, 10), 3.4, shade_c(WOOD, 0.9)); tell(d, (12, 8), 4, 4, shade_c(STEEL, 1.0))
        return finish(im, 24, 200), (12, 198)
    if name.startswith("cloth"):
        a = 0 if name == "cloth_w" else 1
        CW, CHh = 64, 44
        im = Image.new("RGBA", (CW * SS, CHh * SS), (0, 0, 0, 0)); d = ImageDraw.Draw(im)
        pal = ARMY[a]
        tpoly(d, [(0, 0), (64, 0), (56, 22), (64, 44), (0, 44)], pal["tunic"])
        if a == 0:
            tpoly(d, [(0, 12), (64, 4), (60, 16), (0, 24)], pal["trim"])
        else:
            tpoly(d, [(0, 0), (24, 0), (0, 22)], pal["trim"]); tpoly(d, [(0, 44), (24, 44), (0, 22)], pal["trim"])
        return finish(im, CW, CHh, light=False), (0, 2)
    if name.startswith("tree"):
        k = int(name[-1])
        CW, CHh = 150, 230
        im = Image.new("RGBA", (CW * SS, CHh * SS), (0, 0, 0, 0)); d = ImageDraw.Draw(im)
        rg = np.random.default_rng(SEED + k * 7)
        tline(d, (75, 228), (75 + rg.uniform(-4, 4), 120), 9, shade_c(WOOD, 0.62))
        if k == 0:  # broadleaf
            for i in range(46):
                x = 75 + rg.normal(0, 28); y = 82 + rg.normal(0, 28)
                tell(d, (x, y), rg.uniform(14, 24), rg.uniform(12, 20), col(0.16 + rg.uniform(0, 0.08), 0.28 + rg.uniform(0, 0.1), 0.14))
        elif k == 1:  # conifer
            for i in range(7):
                y = 30 + i * 28; w = 18 + i * 8
                tpoly(d, [(75, y - 24), (75 + w, y + 24), (75 - w, y + 24)], col(0.08 + 0.01 * i, 0.22 + 0.01 * i, 0.15))
        else:       # scrub
            for i in range(30):
                x = 75 + rg.normal(0, 30); y = 200 + rg.normal(0, 14)
                tell(d, (x, y), rg.uniform(10, 18), rg.uniform(8, 14), col(0.20 + rg.uniform(0, 0.08), 0.30 + rg.uniform(0, 0.08), 0.15))
        return finish(im, CW, CHh), (75, 226)
    if name.startswith("rock"):
        k = int(name[-1]); CW, CHh = 90, 60
        im = Image.new("RGBA", (CW * SS, CHh * SS), (0, 0, 0, 0)); d = ImageDraw.Draw(im)
        rg = np.random.default_rng(SEED + 99 + k)
        pts = [(45 + math.cos(a) * rg.uniform(26, 40), 38 + math.sin(a) * rg.uniform(14, 22)) for a in np.linspace(math.pi, 2 * math.pi, 9)] + [(86, 56), (6, 56)]
        tpoly(d, pts, col(0.40, 0.40, 0.39)); tpoly(d, [(p[0] + 3, p[1] + 3) for p in pts[:6]] + [(70, 50), (20, 50)], col(0.30, 0.30, 0.30))
        return finish(im, CW, CHh), (45, 56)
    raise KeyError(name)

# ------------------------------------------------------------------ atlas ----
class Atlas:
    def __init__(self, width=4096): self.w = width; self.items = []
    def add(self, name, im, anchor, extra=None): self.items.append((name, im, anchor, extra or {}))
    def pack(self):
        items = sorted(self.items, key=lambda t: -t[1].size[1])
        x = y = rowh = 0; pos = {}
        for name, im, anchor, extra in items:
            w, h = im.size
            if x + w + 2 > self.w: x = 0; y += rowh + 2; rowh = 0
            pos[name] = (x, y); x += w + 2; rowh = max(rowh, h)
        H_ = y + rowh + 2
        atlas = Image.new("RGBA", (self.w, H_), (0, 0, 0, 0)); meta = {}
        for name, im, anchor, extra in items:
            px, py = pos[name]; atlas.paste(im, (px, py))
            e = dict(x=px, y=py, w=im.size[0], h=im.size[1], ax=anchor[0], ay=anchor[1]); e.update(extra); meta[name] = e
        return atlas, meta

def gen_sprites():
    A = Atlas()
    # people
    soldier_poses = ["stand_a", "stand_b", "walk0", "walk1", "walk2", "walk3", "sit", "crouch", "ride"]
    combos = [("spear", soldier_poses), ("crate", ["stand_a", "walk0", "walk1", "walk2", "walk3"]),
              ("lantern", ["stand_a", "walk0", "walk1", "walk2", "walk3"]),
              ("officer", ["stand_a", "stand_b", "walk0", "walk1", "walk2", "walk3"]),
              ("none", ["stand_a", "stand_b", "walk0", "walk1", "walk2", "walk3", "sit", "crouch"])]
    for army in (0, 1):
        for var in range(3):
            for acc, poses in combos:
                if acc in ("crate",) and var == 2: continue
                for p in poses:
                    im, ex = draw_human(p, army, var, acc)
                    extra = {}
                    if "lantern" in ex: extra["hx"], extra["hy"] = ex["lantern"]
                    A.add(f"h_{army}_{var}_{acc}_{p}", im, (64, 152), extra)
    # horses
    for coat in ("bay", "grey", "black"):
        for bl in (-1, 0, 1):
            for p in ("stand_a", "stand_b", "graze", "walk0", "walk1", "walk2", "walk3"):
                A.add(f"horse_{coat}_{bl}_{p}", horse_cell(p, coat, bl), (100, 142))
    # tents (kind, trim)
    for kind in ("aframe", "pavilion", "shelter"):
        for trim in (-1, 0, 1):
            im = tent_cell(kind, trim); A.add(f"tent_{kind}_{trim}", im, (im.size[0] // 2, im.size[1] - 4))
    for n in ("campfire", "wagon", "crates", "barrels", "firewood", "pole_w", "pole_e", "cloth_w", "cloth_e",
              "tree0", "tree1", "tree2", "rock0", "rock1"):
        im, anc = prop(n); A.add(n, im, anc)
    atlas, meta = A.pack()
    save(atlas, "sprites.png")
    with open(os.path.join(OUT, "sprites.json"), "w") as f:
        json.dump(dict(atlas="sprites.png", cam=dict(f=CAM_F, h=CAM_H, horizon=HORIZON, w=W, h_px=H), sprites=meta), f, indent=1)
    print("sprites:", len(meta))

if __name__ == "__main__":
    gen_clouds()
    gen_hills("hills_far.png", 300, 190, 70, 0.55, SEED + 1, (0.46, 0.53, 0.60), (0.40, 0.48, 0.50), False)
    gen_hills("hills_near.png", 260, 150, 52, 0.60, SEED + 2, (0.30, 0.40, 0.30), (0.28, 0.36, 0.24), True)
    gen_ground()
    gen_sprites()
