# EXILE bare-metal build pipeline (`exile_build.py`)

Builds the seven Batch 8 kernel variants **for the pinned bare-metal target only** and refuses everything else.
Standard library only. It never installs or changes a toolchain, never edits tracked files, and has **no host-fallback option**.
It wraps the existing `OMNI-Kernel-Batch-8/scripts/build.sh` (unchanged) by always passing `--target <pin>`, then re-checks the
result itself instead of trusting the wrapper.

## Pins (single source of truth: `Meta/Experiments/OMNI-Kernel-Batch-10/toolchain.pin`)

`rustc=1.91.1`, `rust_target=x86_64-unknown-none`, `zig=0.15.2`. Zig is used only by the Batch 2-6 regression, not by the kernel
build, so it is reported always and blocking only with `--with-zig`.

## Commands (run from the repository root)

```
python3 Meta/Exile/build/exile_build.py preflight                         # exit 0 PASS, 3 BLOCKED
python3 Meta/Exile/build/exile_build.py build --out OUT [--variants base,ring3] [--profile release|debug] [--repro]
python3 Meta/Exile/build/exile_build.py iso   --out OUT                   # needs grub-mkrescue + xorriso, an accepted build
python3 Meta/Exile/build/exile_build.py all   --out OUT --repro
python3 Meta/Exile/build/exile_build.py inspect OUT/variants/base         # or any kernel.elf
python3 Meta/Exile/build/exile_build.py verify-manifest OUT/build-manifest.json
python3 -I -B Meta/Exile/build/tests/test_exile_build.py                  # 32 tests
```

To point it at a pinned toolchain that is not the default: `EXILE_RUST_SYSROOT=/path/to/rust-1.91.1` (its `bin/rustc` is used), or have
`rustup which --toolchain 1.91.1 rustc` resolve. Installing it is your decision, for example
`rustup toolchain install 1.91.1 --profile minimal -t x86_64-unknown-none` (this is what CI does; the tool never runs it).

## What it enforces

1. **Preflight:** `rustc -vV` release equals the pin exactly; the target's `core` library exists in that rustc's sysroot; gcc, GNU ld and
   objcopy are present; source files exist. Anything missing is **BLOCKED** with the precise reason and *nothing is built*.
2. **Build:** `build.sh --variant V --test-exit --profile P --target x86_64-unknown-none --out OUT/variants/V` with a clean environment:
   `RUSTFLAGS`, `RUSTC_WRAPPER`, `RUSTC_BOOTSTRAP` removed; `SOURCE_DATE_EPOCH` = the HEAD commit time; `TZ=UTC`, `LC_ALL=C`.
3. **Independent provenance check** (the kernel ELF discards `.comment`, so the evidence is read from the produced rlibs/static library):
   the rustc version string inside the rlibs must be exactly the pin; the rlibs must carry the pinned target triple; the kernel static
   library must contain no host target triple; `build-info.txt` must agree. A compiler shim that *lies* about its version
   is therefore still REJECTED (tested).
4. **ELF validation:** ELF64 little-endian, EXEC, x86-64; entry inside an executable PT_LOAD **and** inside `.text`; `.text` AX/not-W,
   `.rodata` A/not-WX, `.data`/`.bss` WA/not-X; no W+X segment; no dynamic/TLS/interp; Multiboot 1 header (magic, checksum, 4-aligned, in
   the first 8 KiB, memory-map flag); plus the existing `check_image.py` and `check_asm.py`.
5. **Manifest** `OUT/build-manifest.json` + `OUT/SHA256SUMS` (see below). `verify-manifest` re-hashes every artifact and re-applies the rules,
   so an edited "PASS" does not verify (tested).
6. **ISO** (`iso`): the existing intended architecture, a GRUB Multiboot ISO: `boot/omni8.elf` + `boot/grub/grub.cfg` (the Batch 8 file),
   made with `grub-mkrescue`; read back with `xorriso` to check the file list, GRUB i386-pc files, an El Torito BIOS entry, and that the
   kernel inside is byte-identical to the built one. ISO reproducibility is not verified and booting is not part of this tool.

Verdicts: `PASS`, `BLOCKED` (prerequisite missing), `FAIL` (build or check failed), `REJECTED` (structurally fine but not provably
built by the pinned compiler for the pinned target). Only `PASS` sets `accepted: true`.

## Manifest fields

`schema_version, tool, tool_version, created, status, accepted, source{git_head, git_branch, commit_time, dirty_tracked_files,
build_input_files, build_input_digest}, config{profile, test_exit, target, build_script, env, host_fallback_allowed:false},
toolchain{pin, rustc{path, release, host, commit_hash, llvm, verbose}, sysroot, target, target_installed, gcc, ld, objcopy, zig},
preflight{status, checks, blockers}, variants[{variant, status, reasons, command, rc, seconds, log, build_info, elf{sha256, bytes, entry,
sections}, elf_checks, provenance, check_image.py, check_asm.py, artifacts[{path, sha256, bytes}]}], reproducibility, iso[], notes`.

## Orchestrator integration

`Meta/Exile/validation/exile_validate.py` now runs this pipeline as stage `build.bare-metal`; `build.<variant>` and
`build.reproducible` are read from its manifest, and the manifest is copied to `reports/build-manifest.json`. Without the pinned toolchain
those stages (and everything after them) are BLOCKED. `--diagnostic-host-build` keeps the old host-target path for diagnostics: it is
labelled in every stage, disables the runtime claim, and caps the overall verdict below PASS. No existing check was removed or relaxed.

## Findings about the existing CI (not changed)

`.github/workflows/exile-kernel-boot.yml` installs rustc 1.91.1 with the bare-metal target, but `verify.sh` and `qemu_test.py` call
`build.sh` **without `--target`**, so CI builds and boots *host-target* images (with a 1.91.1 compiler) and never exercises
`x86_64-unknown-none`. Whether the Batch 7/8 crates compile and boot for the real target is therefore **unproven anywhere**.
This pipeline is the first thing that asks for it. Fixing CI is a one-line change per call (`--target x86_64-unknown-none`) left for
maintainer review because it changes a protected batch's scripts.

## Blockers in the authoring sandbox (all verified by running the commands)

| Blocker | Evidence |
|---|---|
| rustc 1.91.1 absent (only 1.97.0) | `preflight` output; `static.rust-lang.org` returns HTTP 403 via the proxy, so it cannot be fetched |
| `x86_64-unknown-none` target absent | same; stable cannot build `core` without it |
| QEMU, grub-mkrescue, xorriso, zig absent | `which`; proxy refuses their sources |
| ISO + boot never executed | consequence of the above |

Therefore **no accepted bare-metal build, ISO or boot exists from this work**. What was run for real: the refusal paths, the rejection of a
real host-target build (6 independent reasons), ELF validation of real linked images, manifest tamper detection, and the full test
suites.

## Untested assumptions

- `build.sh --target x86_64-unknown-none` has never been run to completion (needs the pinned toolchain).
- The rlib provenance markers (`rustc version X.Y.Z`, the triple string) were calibrated on rustc 1.97.0 output; they are expected to be
  present in 1.91.1 output as well but that is unverified. If they are absent the pipeline REJECTS (fails closed), it does not accept.
- Reproducibility under the pinned toolchain is untested (the host-target build is reproducible across directories, which is the only evidence).
