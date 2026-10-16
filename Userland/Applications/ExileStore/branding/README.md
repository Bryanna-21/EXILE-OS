# Exile Store brand assets

The mark is an **X** (for Exile): a blue bar passing *under* a green arrow that points up and out. It is an original drawing made for this project, in the store's palette (black, green, blue; no purple).

| File | Use |
|---|---|
| `icon.svg` | Master vector. Also embedded as the web favicon (`web/icon.svg`). |
| `icon-maskable.svg` | Full-bleed version with a safe zone, for launchers that crop icons to their own shape. |
| `icon-1024.png`, `icon-512.png`, `icon-192.png` | Raster exports for app launchers and shells. |
| `apple-touch-icon.png` (180) | Embedded in the web UI for home-screen shortcuts. |
| `icon-48.png`, `icon-32.png` | Small-size checks; the mark stays legible at 32 px. |

Rules: keep the tile dark, never recolor the arrow or bar into purple or other off-palette colors, and keep clear space of at least a quarter of the tile's width around it. Regenerate PNGs from the SVGs after any change to the SVGs (the web copies in `web/` must be updated too).

Not included: two AI-generated concept mockups were shared during design (a gold knotted "S" and a glowing cyan "X"). They are raster photos-of-a-tablet, not usable as shipped icons, and the gold is outside the palette, so neither is used. The X direction informed this design.
