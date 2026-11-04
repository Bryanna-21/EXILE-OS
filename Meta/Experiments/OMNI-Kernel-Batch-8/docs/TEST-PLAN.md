# Test plan and results

Legend: **PASSED** = executed here and passed. **NOT RUN** = needs something this environment lacks. A skipped or unrun
test is never counted as a pass.

## Environment

rustc/cargo/clippy/rustfmt 1.97.0 (host `x86_64-unknown-linux-gnu`; the pinned 1.91.1 and `x86_64-unknown-none` are not
installed), gcc 13.3, GNU ld/objdump/objcopy 2.42, Python 3.13, Zig 0.15.2 (regression only). No QEMU, no GRUB, no
xorriso, no `/dev/kvm`, no network.

## Hosted unit tests (`cargo test --offline`, debug and release: 65 passed, 0 failed)

| file | tests | covers |
|---|---|---|
| `desc_tests` | 9 | GDT layout and encoding, selector arithmetic, STAR compatibility, TSS size/offsets, IST validation, negative cases |
| `idt_tests` | 11 | gate encoding, error-code table, IST map, trap-frame layout, page-fault decoding, fatal report format, bad gate rejection |
| `paging_tests` | 13 | 4 KiB/2 MiB mapping, permissions arithmetic, W^X refusal, window discipline, misalignment, overflow, double map, huge conflict, out-of-frames |
| `frames_tests` | 9 | pool/reserve, alloc order, double free, foreign frames, ceiling, exhaustion |
| `multiboot_tests` | 10 | magic, truncated/oversized/overlapping maps, usable-range arithmetic, reserved ranges (modules, strings, overflow, capacity) |
| `timer_usermode_tests` | 10 | divisor limits, PIC/PIT sequences, tick counter, `iret_frame` validation, Batch 7 constant compatibility |
| `selftest_tests` | 3 | probe expectation matching and the scheduler order scenario on the real Batch 7 context-switch assembly, repeatable and negative |

## Static checks on the built image

| check | result |
|---|---|
| `scripts/check_image.py` (ELF64 and ELF32 copy: segments, W^X, Multiboot header and checksum, entry, symbols, alignment, 2 MiB bound) | 31 passed, 0 failed |
| `scripts/check_asm.py` (isr.S matches the generator, isr_common structure, 48 stubs, no AVX/x87, no red zone, SYSCALL stub, probes, boot sequence order) | 25 passed, 0 failed |
| two independent builds are byte-identical | PASSED |
| all six variants build; release and debug profiles build | PASSED |
| `scripts/qemu_test.py --selftest` (transcript matcher on synthetic input; boots nothing) | 9 passed, 0 failed |

## QEMU boot tests: **NOT RUN** (QEMU and GRUB unavailable)

`scripts/qemu_test.py` builds, wraps in a GRUB ISO, boots and compares. Without the tools it prints `NOT RUN` for every
case and exits 3; `verify.sh` reports `SKIPPED`.

| case | expected exit | expected serial | status |
|---|---|---|---|
| base | 33 | `tests/expected/base.txt` (exact, in order) | NOT RUN |
| ring3 | 33 | `tests/expected/ring3.txt` | NOT RUN |
| ring3-fault | 35 | fatal #PF, `cs=0x0023 ring3`, no PASS line | NOT RUN |
| trap-fault | 35 | fatal #UD report, no PASS line, paging never reached | NOT RUN |
| magic-fault | 35 | `OMNI8 FAIL: boot magic` | NOT RUN |
| cpu-fault | 35 | `OMNI8 FAIL: CPU features` | NOT RUN |

QEMU command line for every case:
`qemu-system-x86_64 -machine pc -accel tcg -cpu qemu64 -m 128M -boot d -display none -serial stdio -monitor none -no-reboot -device isa-debug-exit,iobase=0xf4,iosize=0x04 -cdrom omni8.iso`

## Expected `base` transcript

```
OMNI8 BOOT: start
OMNI8 VARIANT: base
OMNI8 BOOT: multiboot ok
OMNI8 MEM: map parsed regions=<n> usable_ranges=<n>
OMNI8 INFO: free_frames=<n> ram_top=<hex>           (ignored by the matcher)
OMNI8 GDT: loaded
OMNI8 TSS: loaded
OMNI8 IDT: loaded
OMNI8 PAGING: wx tables active
OMNI8 FRAMES: alloc/free ok
OMNI8 TRAP: breakpoint ok
OMNI8 TRAP: undefined-opcode ok
OMNI8 TRAP: divide-error ok
OMNI8 TRAP: write-to-text pagefault ok
OMNI8 TRAP: exec-from-nx pagefault ok
OMNI8 TRAP: unmapped-read pagefault ok
OMNI8 TRAP: null-read pagefault ok
OMNI8 TRAP: positive controls ok
OMNI8 SCHED: order ok switches=12
OMNI8 TIMER: ticks ok
OMNI8 TEST PASS
```
`ring3` adds `OMNI8 RING3: user address space built` and `OMNI8 RING3: cpl3 int81 seen, syscall exit code=1` before the
PASS line. These are **expectations**, not observations.

## Regression

Batches 2 to 6 (Zig, includes the Batch 2 to 5 suites) are re-run by `scripts/verify.sh`; Batch 7's 56 tests are
unchanged and Batch 8 builds against it.

## Not covered by any test

Real hardware; KVM; SMP; the pinned toolchain; the `-kernel` (ELF32) boot path; a GRUB ISO build; interrupt delivery; MSR
writes; any ring transition; `Kernel/` and `ExileKernel/` integration.
