# EXILE-OS Work Orders WO1 / WO2 / WO4 / WO6 — final submission

Prepared on branch `feature/core-identity-architecture` from base commit `a0220125`. **Nothing committed, pushed or merged.** The integration files are staged additions pending final review.
A pre-edit backup was maintained outside the repository; the original submission archive is retained unchanged.

## Result by work order (PASS / FAIL / BLOCKED / NOT RUN)
| WO | Deliverable | Verdict |
|---|---|---|
| WO1 | Pinned, no-fallback bare-metal build pipeline, ELF validation, ISO stage, manifests, orchestrator integration | **Code+tests PASS (32 + 42). Real bare-metal build: BLOCKED** (pinned rustc 1.91.1, target `x86_64-unknown-none`, zig 0.15.2 unavailable; `static.rust-lang.org` returns 403 here). ISO + QEMU boot: **NOT RUN** |
| WO2 | Subsystem status matrix, unsafe-boundary register, sync primitives, boot-memory/CPU/handoff code | **Docs + hosted tests PASS. No isolation/scheduling/protection claim made** (no executed evidence) |
| WO4 | HAL: memmap, ACPI, PCI, LAPIC, CPU, framebuffer, device decoders, registry + capability matrix | **Parsers/decoders PASS vs mocks. No NVMe/AHCI/xHCI/network driver exists** |
| WO6 | Boot sequence docs, BootInfo v1, A/B slots + rollback, GPT disk builder/validator | **Foundations PASS (hosted). UEFI loader, filesystem formatting, installer: NOT IMPLEMENTED. End-to-end virtual install: NOT RUN** |

## Evidence (all re-run at submission; logs in `logs/`)
* `python3 -I -B Meta/Exile/build/tests/test_exile_build.py` → 32 OK
* `python3 -I -B Meta/Exile/validation/tests/test_exile_validate.py` → 42 OK
* `python3 -I -B Meta/Exile/installer/tests/test_exile_disk.py` → 8 OK, 1 skipped (`sfdisk` absent: independent GPT cross-check NOT RUN)
* `cargo test --offline` in `Meta/Exile/platform` → 67 OK; `cargo clippy --all-targets -D warnings` clean; `cargo fmt --check` clean
* Mutation check on the platform crate: 12 injected bugs, 12 detected (`logs/mutation_platform.log`)
* `exile_build.py --repo . preflight --with-zig` → exit 3 BLOCKED (`logs/preflight.log`)

## Known limitations (read these)
1. **Nothing was booted.** No QEMU/GRUB/xorriso/UEFI firmware here. No boot, ring-3, fault-variant or install result exists.
2. **Platform crate verified only with rustc 1.97**, not the pinned 1.91.1; never compiled for `x86_64-unknown-none` (std target absent). It is `#![no_std]` and compiled that way on the host, which is not the same thing.
3. Rlib provenance markers are calibrated on rustc 1.97 only.
4. `build.sh --target x86_64-unknown-none` has never completed anywhere in this environment.
5. HAL parsers saw fixtures/mocks only (one exception: CPUID decode ran on the host's virtual CPU).
6. TicketLock was tested on 2 host threads; a 4-thread test took >100 s on a 2-vCPU container (spinning-waiter convoy). Not interrupt-safe; kernel use needs IRQ/preemption disabling.
7. GPT images are partition-table skeletons only; not cross-checked by an independent tool.
8. **CI finding:** the repository CI never builds `--target x86_64-unknown-none`; left for maintainer review, not changed.
9. Existing GRUB/Multiboot1 BIOS path is the only boot architecture in the tree; the UEFI path is design-only.

## Reproduction
```
cd <worktree>
python3 -I -B Meta/Exile/build/tests/test_exile_build.py
python3 -I -B Meta/Exile/validation/tests/test_exile_validate.py
python3 -I -B Meta/Exile/installer/tests/test_exile_disk.py
CARGO_TARGET_DIR=/tmp/plat cargo test --offline --manifest-path Meta/Exile/platform/Cargo.toml
python3 Meta/Exile/build/exile_build.py --repo . preflight --with-zig     # expect exit 3 here; 0 only with the pinned toolchain
python3 Meta/Exile/validation/exile_validate.py --help                    # default build is the strict bare-metal path
```
Docs: `Documentation/Architecture/Platform/{SUBSYSTEM_STATUS,HAL,BOOT_AND_INSTALL,CROSS_WORKSTREAM}.md`, `Meta/Exile/build/README.md`.
Changed-file manifest with SHA-256: `SHA256SUMS`.
