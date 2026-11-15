#!/usr/bin/env python3
"""Generate EXILE-OS boot artwork and animation. Original geometry, no third-party branding.
Usage: generate-boot-assets.py OUTDIR   (requires Pillow)
Palette: black, white, green #2EE59D, blue #3A8DFF. No purple."""
import math, os, sys
from PIL import Image, ImageDraw, ImageFont, ImageFilter

BG = (6, 8, 10); WHITE = (240, 244, 246); DIM = (110, 120, 128)
GREEN = (46, 229, 157); BLUE = (58, 141, 255); GRID = (18, 24, 28)
BOLD = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"
MONO = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"
VERSION = "v@VERSION@"; BUILD = "build @BUILD@"
STATUS = ["init kernel", "mount root", "start services", "ready"]

def font(p, s): return ImageFont.truetype(p, max(8, int(s)))

def spaced(d, xy, text, f, fill, spacing):
    x, y = xy
    for ch in text:
        d.text((x, y), ch, font=f, fill=fill)
        x += d.textlength(ch, font=f) + spacing

def spaced_width(d, text, f, spacing):
    return sum(d.textlength(c, font=f) + spacing for c in text) - spacing

def hexagon(cx, cy, r, rot):
    return [(cx + r * math.cos(rot + k * math.pi / 3), cy + r * math.sin(rot + k * math.pi / 3)) for k in range(6)]

def render(W, H, t=1.0, menu_safe=True, ss=2):
    """t in [0,1] animation progress. Supersampled for clean edges."""
    w, h = W * ss, H * ss; u = h / 1080.0
    im = Image.new("RGB", (w, h), BG); d = ImageDraw.Draw(im)
    step = int(60 * u)
    for x in range(0, w, step): d.line([(x, 0), (x, h)], fill=GRID, width=max(1, int(u)))
    for y in range(0, h, step): d.line([(0, y), (w, y)], fill=GRID, width=max(1, int(u)))
    cx, cy = w / 2, h * (0.15 if menu_safe else 0.40)
    sc = 0.6 if menu_safe else 1.0
    R = 120 * u * sc
    # emblem: nested hexagons, one open "exile" gap that rotates with t
    d.polygon(hexagon(cx, cy, R * 1.5, math.pi / 6), outline=(30, 38, 44), width=max(1, int(2 * u)))
    d.polygon(hexagon(cx, cy, R * 1.2, math.pi / 6), outline=(44, 54, 62), width=max(1, int(2 * u)))
    rot = math.pi / 6 + t * math.pi / 3
    pts = hexagon(cx, cy, R, rot)
    draw_n = 6 if t >= 1 else max(1, int(1 + t * 5.999))
    for k in range(5):  # 5 of 6 edges: the missing edge is the gap
        a, b = pts[k], pts[k + 1]
        col = GREEN if k % 2 == 0 else BLUE
        if k < draw_n: d.line([a, b], fill=col, width=max(2, int(6 * u)))
    inner = hexagon(cx, cy, R * 0.45, -rot)
    d.polygon(inner, outline=WHITE, width=max(1, int(3 * u)))
    for p in pts[:5]:
        r = 5 * u; d.ellipse([p[0]-r, p[1]-r, p[0]+r, p[1]+r], fill=WHITE)
    # wordmark
    f = font(BOLD, 96 * u * sc); sp = 22 * u * sc; text = "EXILE-OS"
    tw = spaced_width(d, text, f, sp); ty = cy + R * 1.75
    spaced(d, (cx - tw / 2, ty), text, f, WHITE, sp)
    bar_y = ty + 112 * u * sc
    d.line([(cx - tw / 2, bar_y), (cx - tw / 2 + tw * 0.16, bar_y)], fill=GREEN, width=max(2, int(5 * u)))
    d.line([(cx - tw / 2 + tw * 0.16, bar_y), (cx + tw / 2, bar_y)], fill=BLUE, width=max(1, int(2 * u)))
    fm = font(MONO, 26 * u)
    v = f"{VERSION}  |  {BUILD}"; vw = d.textlength(v, font=fm)
    d.text((cx - vw / 2, bar_y + 22 * u), v, font=fm, fill=DIM)
    # bottom: status + loader
    by = h - 150 * u; bw = 520 * u; bx = cx - bw / 2; bh = 8 * u
    d.rectangle([bx, by, bx + bw, by + bh], outline=(50, 60, 68), width=max(1, int(u)))
    fill = bw * t
    if fill > 0:
        mid = bx + fill * 0.5
        d.rectangle([bx, by, mid, by + bh], fill=GREEN)
        d.rectangle([mid, by, bx + fill, by + bh], fill=BLUE)
    idx = min(len(STATUS) - 1, int(t * len(STATUS))) if t < 1 else len(STATUS) - 1
    s = f"[ {int(t * 100):3d}% ]  {STATUS[idx]}"
    fs = font(MONO, 28 * u); sw = d.textlength(s, font=fs)
    d.text((cx - sw / 2, by + 28 * u), s, font=fs, fill=WHITE)
    info = "x86_64  |  kernel @KERNEL_ID@  |  serial ttyS0"
    fi = font(MONO, 20 * u); iw = d.textlength(info, font=fi)
    d.text((cx - iw / 2, h - 60 * u), info, font=fi, fill=DIM)
    # corner brackets
    m = 48 * u; L = 70 * u; wd = max(1, int(3 * u))
    for (x, y, dx, dy) in [(m, m, 1, 1), (w-m, m, -1, 1), (m, h-m, 1, -1), (w-m, h-m, -1, -1)]:
        d.line([(x, y), (x + dx * L, y)], fill=GREEN, width=wd); d.line([(x, y), (x, y + dy * L)], fill=BLUE, width=wd)
    return im.resize((W, H), Image.LANCZOS)

def main(out):
    for sub in ("static", "wallpaper", "animation/frames"): os.makedirs(os.path.join(out, sub), exist_ok=True)
    render(3840, 2160, 1.0, menu_safe=False).save(os.path.join(out, "static/exile-boot-artwork-3840x2160.png"), optimize=True)
    for (W, H) in [(1024, 768), (1920, 1080)]:
        render(W, H, 1.0, menu_safe=True).save(os.path.join(out, f"wallpaper/exile-wallpaper-{W}x{H}.png"), optimize=True)
    N = 48; frames = []
    for i in range(N):
        im = render(1024, 768, i / (N - 1), menu_safe=False)
        im.save(os.path.join(out, f"animation/frames/frame-{i:03d}.png"), optimize=True); frames.append(im)
    # spritesheet: 8 columns x 6 rows of 256x192 thumbnails
    sw, sh = 256, 192; sheet = Image.new("RGB", (sw * 8, sh * 6), BG)
    for i, f in enumerate(frames): sheet.paste(f.resize((sw, sh), Image.LANCZOS), ((i % 8) * sw, (i // 8) * sh))
    sheet.save(os.path.join(out, "animation/spritesheet-8x6-256x192.png"), optimize=True)
    pal = [f.resize((640, 480), Image.LANCZOS).quantize(colors=64, method=Image.MEDIANCUT) for f in frames]
    pal[0].save(os.path.join(out, "animation/exile-boot-preview.gif"), save_all=True, append_images=pal[1:] + [pal[-1]] * 12, duration=60, loop=0, optimize=True)

if __name__ == "__main__": main(sys.argv[1])
