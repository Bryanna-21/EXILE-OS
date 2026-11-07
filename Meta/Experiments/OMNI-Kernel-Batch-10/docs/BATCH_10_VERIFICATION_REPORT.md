# Batch 10 verification report

Run on 2026-11-07 in a Linux x86-64 container. Results are from `scripts/verify.sh` (phase `pre`, before the manifest existed). A check
that could not run is marked SKIPPED, WARN or BLOCKED and is never counted as passed.

## Toolchain versions and pin status

| Tool | Installed | Pinned / required | Status |
|---|---|---|---|
| rustc, cargo, clippy | 1.97.0 | 1.91.1 | **MISMATCH.** Not installable here (no network). Results are not evidence for the pin. The pin was not changed |
| Rust target `x86_64-unknown-none` | absent | required for the bare-metal build | **BLOCKED** |
| Zig | 0.15.2 | 0.15.2 | OK |
| QEMU x86-64 and aarch64, GRUB `grub-mkrescue`, `xorriso` | absent | required for runtime validation | **BLOCKED** |
| gcc 13.3, GNU ld 2.42, nasm, clang 18, lld | present | not pinned | informational |
| C++ cross toolchain for `Kernel/` | absent | needed to build `Kernel/` | **BLOCKED**: `Kernel/` was not built |

`python3 -I scripts/check_toolchain.py` prints this table and exits 3 here (7 problems). `OMNI10_REQUIRE_PINNED=1` makes it fatal in `verify.sh`.

## Commands and results

From `Meta/Experiments/OMNI-Kernel-Batch-10`: `OMNI10_PHASE=pre OMNI10_SKIP_B8_VERIFY=1 scripts/verify.sh`

| Check | Result |
|---|---|
| `cargo fmt --check` | PASS |
| `cargo clippy --all-targets -- -D warnings` (Batch 10 crate; also run once with `--features x86-ring0`) | PASS |
| Batch 10 tests, debug: `cargo test --offline --locked` | PASS, 24 passed, 0 ignored (txmap 10, irqlock 7, abi 7) |
| Batch 10 tests, release | PASS, 24 passed |
| Batch 9 regression | PASS, 34 passed |
| Batch 9 ABI conformance and self-test | PASS, 23 checks; 7 of 7 mutations detected |
| Batch 7 regression | PASS, 56 passed |
| Batch 8 regression | PASS, 65 passed |
| Zig Batches 2, 3, 4, 5, 6 | PASS: 59, 116, 171, 228, 282 (cumulative counts) |
| `git diff --check`, trailing whitespace, hygiene scan | PASS |
| Protected paths unmodified; tracked files equal the base outside Batches 8, 9, 10 | PASS |
| Required-file check | **FAIL in this run**: `scripts/gen_manifest.py` was not yet in the package. It was added afterwards and is checked again in the post-packaging run (see the final response) |
| Warnings | rustc is not the pin; toolchain does not match `toolchain.pin` |
| Skipped | manifest and checksums (phase `pre`); Batch 8's own verify script (it was run in the Batch 9 session) |

The Zig results reuse compiled caches; each suite was built from an empty cache earlier in the programme with the same counts.

## Mutation checks (tests catch what they claim to)

Each mutation was applied to a copy, the tests were run, and the original was restored (24/24 pass afterwards):

| Mutation | Tests that failed |
|---|---|
| `txmap`: do not return frames on rollback | `tx_02`, `tx_08` |
| `txmap`: do not undo journaled writes | `tx_02`, `tx_03`, `tx_04`, `tx_07`, `tx_08` |
| `txmap`: `destroy_tables` does not free the root | `tx_09`, `tx_10` |
| `irqlock`: unlock does not restore interrupt state | `irq_01`, `irq_02`, `irq_03`, `irq_05`, `irq_07` |
| `abi`: Exile mode does not move arg1 | `abi_02`, `abi_07` |

## Not run

| Item | Reason |
|---|---|
| Anything under rustc 1.91.1 or `x86_64-unknown-none` | unavailable offline |
| QEMU boot, interrupt delivery, timer, user-mode execution, syscall entry, multiple processes/threads, IPC, process termination on `pc` and `q35` | no QEMU; these were not attempted |
| ARM64 build or boot | not implemented, no QEMU |
| Hardware | none available |
| Build or tests of `Kernel/`, `Tests/Kernel`, `Tests/LibC` | no toolchain; source reading only |
| `X86Irq` execution | needs ring 0; it only compiles |
| Fuzzing, sanitizers, Miri, loom | not set up |
| Phases 1, 4 to 14 of the brief | not implemented here; see `PROGRAMME_STATUS.md` |

## Reproduce

```
cd Meta/Experiments/OMNI-Kernel-Batch-10
CARGO_TARGET_DIR=/path/outside/repo/c10 OMNI10_ZIG_CACHE=/path/outside/repo/zig scripts/verify.sh
```
