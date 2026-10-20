# BRANDING_PALETTE_AUDIT.md

Requirement: black, white, green and blue; no purple. Scope of this audit: `Base/res/themes/*.ini` and the wallpapers. **No theme or asset was changed**; inherited themes are SerenityOS-derived and altering them is a product decision.

Method: a script flagged colour values with purple/magenta/violet hues (named colours such as `magenta`, `violet`, and hex values in the purple hue range). Icons and other images were **not** scanned.

## Results
- Most inherited themes contain purple-hued values. Typical: `VisitedLink=magenta`, `SyntaxType=#800080` or violet, `SyntaxKeyword=#a42ea2`.
- Heaviest: `Plum.ini` (15 of 54 colour values), `Contrast.ini` (11 of 80), `Pumpkin.ini` (9 of 74).
- `Gruvbox Dark.ini`: none found.
- Wallpapers: `sunset-retro.png` about 5.8% purple pixels; `grid.png` and `tile.png` 0%.
- Exile Store icon: blue, green and near-black only.

## Recommendation (not applied)
1. Pick an Exile default theme and fix its `VisitedLink` and syntax colours first.
2. Decide whether the Plum theme and `sunset-retro.png` stay as optional inherited content or are removed.
3. Scan icons and remaining images before claiming the palette requirement is met.

Status of requirement 19: **Partial / unverified.**
