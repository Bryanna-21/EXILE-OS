# Batch 8 report

## Verdict

Batch 8 is **built and unit-tested, but not boot-tested**. The definition-of-done items that need a real boot (QEMU serial
evidence, ring-3 execution, a syscall from ring 3, timer interrupts) are **not met**. They are implemented, compiled and
statically checked, and they are written as tests that report NOT RUN here.

## Status by level

| Feature | Implemented | Compiled | Unit-tested (host) | QEMU-tested | Hardware-tested |
|---|---|---|---|---|---|
| Multiboot v1 bootstrap, serial, long-mode entry | yes | yes | no (assembly) | **NOT RUN** | no |
| Linker script, image layout | yes | yes | image checker 31 checks | **NOT RUN** | no |
| GDT / TSS | yes | yes | yes | **NOT RUN** | no |
| IDT, ISR stubs, fatal reports | yes | yes | yes (layout, encoding, format) | **NOT RUN** | no |
| Multiboot map ingestion | yes | yes | yes | **NOT RUN** | no |
| Frame allocator | yes | yes | yes | **NOT RUN** | no |
| Page tables, W^X, window discipline | yes | yes | yes (simulated memory) | **NOT RUN** | no |
| Page-fault self-tests | yes | yes | matcher only | **NOT RUN** | no |
| Scheduler scenario on Batch 7 | yes | yes | **yes, on real Batch 7 context-switch asm (host)** | **NOT RUN** | no |
| PIC/PIT timer test (no preemption) | yes | yes | sequences only | **NOT RUN** | no |
| Ring-3 entry, SYSCALL from ring 3 | yes (variant) | yes | `iret_frame` only | **NOT RUN** | no |
| Boot image generation (GRUB ISO) | script | n/a | no | **NOT RUN** | no |
| Tick-driven preemption at safe points (`preempt` variant: ISR does lock-free slice accounting, threads yield at safe points) | yes | yes | 11 hosted tests incl. a scenario on the real Batch 7 scheduler; image builds and passes static checks | **NOT RUN** (needs the CI boot job) | no |
| Asynchronous preemption from inside the interrupt, SMP, ARM64, ExileKernel integration | **not implemented** | | | | |

## Results actually obtained

* `cargo test --offline`, debug and release: **65 passed, 0 failed** (desc 9, idt 11, paging 13, frames 9, multiboot 10,
  timer/usermode 10, selftest 3).
* `cargo fmt --check`, `cargo clippy --all-targets -D warnings` (hosted crate): clean. `kernel/main.rs` is formatted but
  not linted.
* Six kernel variants and the debug profile build and link. `check_image.py`: 31 passed. `check_asm.py`: 25 passed.
  Two independent base builds are byte-identical.
* `qemu_test.py --selftest`: 9 passed (matcher only).
* **QEMU boot tests: NOT RUN** (no QEMU, GRUB, xorriso). Exit status of the harness without them: 3.
* Regression: Batches 2 to 6 re-run by `verify.sh` (see `verify8.txt` delivered with the bundle).

Toolchain: rustc/cargo 1.97.0, gcc 13.3, GNU ld 2.42, Zig 0.15.2, Python 3.13. Commands are in `docs/BOOT.md`
and `docs/TEST-PLAN.md`; the exact compiler invocations are recorded in `build-info.txt` by `build.sh`.

## Deviations and limitations

1. **Toolchain.** Pinned rustc 1.91.1 / `x86_64-unknown-none` unavailable offline; the kernel is built for the host target
   with freestanding flags and GNU ld. Passing the image checks does not prove it runs.
2. **No boot evidence.** Every privileged instruction sequence (lgdt, ltr, lidt, CR3 write, MSR writes, iretq, sysretq, sti)
   is unexecuted. A first boot may well triple-fault; the fault variants and transcripts are ready for that run.
3. Identity-mapped, 1 GiB ceiling, single CPU, no preemption, no FPU isolation, a single syscall stack (one ring-3 thread).
4. The scheduler static (about 256 KiB of thread stacks) lands in `.data` rather than `.bss` because it has non-zero initial fields, so the ELF is about 1 MB. Harmless; noted.
5. The `-kernel` ELF32 path and ISO generation are untested.
6. Syscall numbers and register convention are provisional; `Kernel/` uses a different convention.

## Files

Created: everything in this directory. Modified outside it: none (`verify.sh` checks protected paths).

## Security implications

See `docs/SECURITY.md`. The main one is that unexecuted privileged code is unverified code.

## ARM64 implications

See `docs/SECURITY.md`: policy logic and the self-test scenario port; boot, descriptors, interrupts, syscall entry and
context switch do not.

## Recommended Batch 9

1. Obtain the pinned toolchain and QEMU/GRUB (or run in CI), then boot every variant and fix whatever the first boot finds;
   record real transcripts. This must precede new features.
2. Per-thread kernel stacks for the syscall path, then more than one user thread.
3. Timer-driven tick into the scheduler (still cooperative), then preemption design with FPU save.
4. Decide the syscall register convention against `Kernel/` and ExileKernel; begin integration there.
5. Higher-half kernel, guard pages, SMEP/SMAP.
