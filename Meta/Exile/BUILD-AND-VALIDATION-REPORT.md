# Build and Validation Report

| Stage | Status | Evidence |
|---|---|---|
| bash -n on all scripts + test suite | PASS | run here |
| Script logic tests (stubs, synthetic logs; 58 checks) | PASS 58/58 | `test-results/script-logic-tests-stub.log`. Tests script logic only, not the kernel. |
| Host scheduler mapping (stages 1-3) | PASS | `test-results/host-test.log`: 99/50/30/10/1 -> 0/15/21/28/31, 32 distinct buckets. Host arithmetic only. |
| Kernel compile | BLOCKED | no `Toolchain/Local/x86_64/bin/x86_64-serenity-g++`; GNU mirror returned 403, no build network. `kernel-build.log` |
| Image generation | BLOCKED | `Build/x86_64` not configured (needs kernel + userland). `image-build.log` |
| QEMU boot | BLOCKED | qemu-system-x86_64 not installed. `qemu-boot.log` |
| Guest scheduler test (stages 4-5) | BLOCKED | needs QEMU + kernel. `guest-scheduler-test.log` |
| Finalizer test (stage 6) | BLOCKED | same. Test written; host sanity run reaped 20/20 (host only). |
| SMP 2-CPU test (stage 7) | BLOCKED | same; SMP is off by default in the kernel. |

## Finish elsewhere
```sh
Toolchain/BuildGNU.sh        # or docker build -t exile-toolchain -f Toolchain/Dockerfile .
Meta/Exile/scripts/build-kernel.sh --arch x86_64 --toolchain GNU
Meta/exile.sh build x86_64 GNU          # installed userland, needed for an image
Meta/Exile/scripts/build-image.sh --bootloader qemu
Meta/Exile/scripts/validate-scheduler-priority.sh
```

## Defects found
1. `TestSchedulerPriority` printed "complete" even when its checks failed. Fixed: it now prints `result=pass|fail`, and the scripts judge numbers, not markers.
2. Host run of `TestSchedulerPriority` fails on Linux (POSIX priorities ignored for SCHED_OTHER). Expected, and shows the test can fail; it is not evidence about the kernel.
3. My earlier `PREEMPTION.md` mis-cites the trap-exit location (correct: `Kernel/Arch/Processor.cpp:272`) and wrongly says SystemServer starts no tests in self-test mode on current main. Not corrected in this package.
No scheduler defect was demonstrated; the scheduler was not replaced.

## Boot experience
- Static art, wallpapers (same format and 1024x768 size as the existing `grid.png`) and animation are original: black, white, green #2EE59D, blue #3A8DFF, no purple. Placeholders `@VERSION@`, `@BUILD@`, `@KERNEL_ID@` remain literal.
- Limine options (wallpaper, wallpaper_style, BMP/PNG/JPEG/QOI) were read from Limine trunk docs, not the pinned v12.5.2. Wallpapers were never shown by a real Limine.
- **The bootloader cannot play the animation** (Limine = static wallpaper, GRUB text-only, QEMU -kernel has no bootloader). It is a preview only.
- `Meta/limine.conf` and the image scripts were not modified.

## Repo state
Uncommitted in worktree `<build-worktree>` (branch `build/boot-package`): 2 modified (`Tests/Kernel/CMakeLists.txt`, `TestSchedulerPriority.cpp`) and new `Meta/Exile/{scripts,boot,tests/test-build-validation-scripts.sh,EXILE-OS-BUILD-AND-VALIDATION.md}`, `Tests/Kernel/TestFinalizerUnderLoad.cpp`. No `Kernel/` or `AK/` changes. Nothing pushed.
