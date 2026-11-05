# Native Exile OS integration - status and blockers

**Status: NOT integrated. The Living Battlefield runs as a standalone Linux host program.** Nothing here has been built or run inside Exile OS.

## What exists in the repo (read from source)
- `GUI::Desktop::set_wallpaper(bitmap, path)` sends `set_wallpaper(Gfx::ShareableBitmap)` to WindowServer; the Compositor draws that single bitmap (Center/Tile/Stretch/Fill).
- There is no wallpaper window, layer, or "live wallpaper" API.

## Consequences
1. A separate-process live wallpaper can only work by pushing a full bitmap through `set_wallpaper` every frame (1920x1080x4 = 8.3 MB/frame, ~250 MB/s at 30 fps over IPC, plus a Compositor repaint). This has **not been measured** and is likely heavy.
2. I could not compile LibGfx/LibGUI here, so no native client was written. Writing one blind would misrepresent the work.

## Proposed path (needs your approval; not done)
- A small `LivingWallpaper` client app that links the renderer as a library (`src/` has no OS dependencies except libpng/pthreads), renders at `render_scale`, and calls `set_wallpaper` at 30/15 fps; single-instance via the existing app lock mechanism.
- Better long-term: a WindowServer "desktop layer" window below icons, so frames use shared-memory bitmaps instead of IPC per frame.

## What the host build does provide
`--stream` writes raw RGBA frames to stdout, takes stdin commands, single-instance lock (exit code 3 on duplicate), clean SIGINT/SIGTERM exit. This is the interface a native shim would wrap.
