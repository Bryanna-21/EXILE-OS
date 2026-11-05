# EXILE OS - LIVING BATTLEFIELD

A standalone, offline, procedurally generated live wallpaper: two opposing armies (West = deep blue, East = emerald) in one persistent world through a full simulated day: dawn, morning standoff, midday routine, afternoon, sunset encampment, evening camp, night watch with raised lanterns, predawn, and a seamless return to dawn.

## Honest status
- **Not photorealistic.** All art is original and procedurally drawn (Python/numpy/Pillow, fixed seed) with a natural-materials pass (grain, folds, muting, ambient occlusion). No image model was available, so the look is a *stylised painterly 2.5D*, not photographic, and nothing here claims to be historical or AI-photographed.
- **2D vs 3D:** everything is 2D sprites and layers placed on a pinhole ground-plane camera (depth sorting, perspective scale, projected shadows). There is no 3D geometry.
- **Not integrated into Exile OS natively.** See `docs/NATIVE_INTEGRATION.md`.
- No purple, no logos/watermarks/text, no copyrighted characters, no religious/ideological content.

## Build
Requires: C++17 compiler, CMake >= 3.16, libpng dev, pthreads. Assets are pre-generated; regenerate with `python3 tools/gen_assets.py` (needs numpy, Pillow; about 25 s, deterministic).
```
cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release
cmake --build build
./build/living_battlefield --selftest
```

## Run / controls
```
./build/living_battlefield --render-phases previews          # 8 phase PNGs
./build/living_battlefield --phase night --out night.png
./build/living_battlefield --sequence DIR --from 0 --to 24 --step 0.05
./build/living_battlefield --benchmark 120 --scale 0.6 --threads 2
./build/living_battlefield --stream --config config/living-battlefield.ini   # raw RGBA frames on stdout
```
Stream stdin commands: `pause`, `resume`, `reset` (to 06:00), `phase NAME` (dawn|morning|midday|afternoon|sunset|evening|night|predawn), `hour H`, `speed X`, `cycle SEC`, `realtime on|off`, `lowpower on|off`, `quit`.
Settings (`config/living-battlefield.ini`): cycle length (default 1800 s), speed, start hour, realtime, resolution, fps (30; low-power 15), render scale, threads, grain, bloom, seed. Time transitions are interpolated; entity motion is a pure function of time, so pause, jumps and the 24:00 wrap never teleport anything.

## Architecture
`tools/gen_assets.py` -> `assets/` (sky, two hill layers, ground, 274-sprite atlas + json). `src/daylight` (keyframed sky/light/fire/lantern curves, wrap-safe), `src/scene` (deterministic ~313-entity schedule: formations, officers, standard bearers, sentries, carriers, attendants, cavalry, camps, tents, fires, lanterns), `src/renderer` (float framebuffer, sky/clouds/stars/sun/moon, fog, lightmap, depth-sorted mipmapped sprites, analytic fire/smoke/sparks/lantern glow, tonemap/grain/vignette, row-band threads), `src/main` (CLI, stream mode, single-instance lock). Raised lanterns are small warm points with a glint at ~1.5 m on carriers/posts; campfires are ground flames with smoke and sparks.

## Measured performance (this sandbox: 2 vCPU, Linux, software only)
| Config | Result |
|---|---|
| 1152x648 (scale 0.6), 1 thread | avg 19.2 ms, worst 33.2 ms, ~52 fps max, RSS 61 MB |
| 1152x648, 2 threads | avg 12.6 ms, worst 20.5 ms, ~79 fps max, RSS 61 MB |
| 1920x1080 (scale 1.0), 2 threads | avg 33.6 ms, worst 41.4 ms, ~30 fps max, RSS 93 MB |
| `--stream` 30 fps (default) | ~97% of one core, 29 fps delivered |
| `--stream` low-power 15 fps | ~52% of one core, ~15 fps delivered |
The 30 fps default is met at the default 0.6 render scale only; full-resolution rendering is right at the limit. The renderer output is 1152x648 and must be stretched to the display. These figures exclude any compositor/IPC cost.

## Validation
`--selftest`: 752,765 entity-steps, largest step 0.74 m, 0 jumps over 1 m, riders within 0.02 m of horses, 0 animation-clock dependence: PASS. Rendered sequence: frame-to-frame differences at 3-second steps stay small across sunrise, sunset and the midnight wrap; the largest change is the army marching out around 04:50 (real motion). Single instance (exit 3 on duplicate) and clean quit tested. Works fully offline.

## Known limitations
Stylised not photoreal; sun disc is faint at sunset; the rendering is CPU-only; no audio; native desktop integration missing; Windows/macOS untested.
