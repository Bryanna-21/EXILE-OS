# EXILE-OS Build, Boot and Scheduler Validation Guide

Scope: how to build the kernel, make a disk image, boot it in QEMU and validate the SCHED-PRIO-1 scheduler
(commit `4313f3e9`). Every script reports honest stage states: PASS, FAIL, NOT RUN or BLOCKED. A host pass is
never a guest pass.

## Scripts (`Meta/Exile/scripts/`)

| Script | Purpose |
|---|---|
| `build-kernel.sh` | Checked wrapper over the superbuild. Success needs a fresh kernel ELF plus Scheduler and FinalizerTask objects. |
| `build-image.sh` | Wraps the CMake image targets (`--bootloader qemu|limine|grub|extlinux`). `--validate-only IMAGE` needs no root or toolchain. |
| `run-qemu.sh` | Boots headless or interactive, detects panics, matches `--expect` patterns, writes logs. `--profile scheduler` boots self-test and requires the verdict lines (it does not judge them; the validate script does). |
| `validate-scheduler-priority.sh` | Seven stages: 1-3 host (mapping, ordering, distribution), 4-6 guest (priority, low-priority limit, finalizer), 7 guest on 2 CPUs with `smp=on`. |

Exit codes: 0 pass, 1 usage, 2 missing host tool, 3 BLOCKED prerequisite, 4 step failed, 5 image validation
failed, 6 boot failed. The last output line is `RESULT <PASS|FAIL|BLOCKED|USAGE> step=<name> reason=<text>`.

## Full sequence on a machine that has the toolchain

```sh
Meta/Exile/scripts/build-kernel.sh --arch x86_64 --toolchain GNU
Meta/Exile/scripts/build-image.sh --bootloader qemu
Meta/Exile/scripts/run-qemu.sh --profile scheduler
Meta/Exile/scripts/validate-scheduler-priority.sh
```

Toolchain: build with `Toolchain/BuildGNU.sh` (or `Toolchain/Dockerfile`). Needs cmake, ninja, python3, QEMU >= 6.2,
mke2fs; the non-qemu images also need root (or fuse2fs), parted, losetup, and mkfs.vfat/git (limine) or grub-install (grub).
Run each script with `--help` for exact options.

## Evidence rules

- Host tests (`Meta/Exile/tests/test-sched-priority-map.sh`) check the pure mapping header only.
- Guest tests (`Tests/Kernel/TestSchedulerPriority.cpp`, `TestFinalizerUnderLoad.cpp`) print `result=pass|fail`;
  the judge checks numbers (`high_min_count>0`, `low_count*10<=high_min`, `reaped==started`) and absence of panic.
- `Meta/Exile/tests/test-build-validation-scripts.sh` (58 checks) uses stubs and synthetic logs. It tests script
  logic only and proves nothing about the kernel.
- The scheduler uses strict priority with no aging and no priority inheritance; starvation of low priorities is by design.

## Boot experience (`Meta/Exile/boot/`)

- `source/generate-boot-assets.py OUTDIR` regenerates everything (Pillow, numpy). Placeholders `@VERSION@`, `@BUILD@`,
  `@KERNEL_ID@` are literal text to substitute before shipping.
- `generated/wallpaper/exile-wallpaper-1024x768.png` is a drop-in for `Base/res/wallpapers/grid.png` (same size and
  format); a 1920x1080 variant is included. Limine documents BMP, PNG, JPEG and QOI wallpapers, default style
  `stretched`, and `interface_resolution` as `<w>x<h>` (CONFIG.md on Limine trunk; not checked against the pinned
  v12.5.2). The menu area is left empty on purpose.
- `generated/static/exile-boot-artwork-3840x2160.png` is presentation artwork, not for the bootloader.
- `generated/animation/` holds 48 frames, an 8x6 spritesheet and a GIF preview.
- **The animation is a preview only.** Limine draws one static wallpaper; GRUB here is text-only; the QEMU `-kernel`
  path has no bootloader. Nothing in the repo can play it. Playing it needs a custom UEFI/early-framebuffer
  stage, which is not implemented.
- To use the wallpaper (not applied, nothing in `Meta/` was changed): copy it over `Base/res/wallpapers/grid.png`
  or change the source path at `Meta/build-image-limine.sh` lines 103-105. `Meta/limine.conf` already points at
  `boot():/wallpaper.png`.
- Not tested on a real Limine boot; no bootloader was run here.
