# OMNI-Kernel Batch 7: scheduler, context switch and syscall path (experimental)

A `no_std` Rust crate with x86-64 assembly: a cooperative uniprocessor scheduler on a real context switch, a SYSCALL
entry stub and dispatch logic, and user-memory validation. Written for `ExileKernel/` but **not integrated into it and
not booted**: this environment has no way to build `ExileKernel/` for its target or to run QEMU. Read `REPORT.md`
first; it states exactly what was and was not achieved.

## What is real

* The context-switch assembly, trampoline, scheduler and syscall dispatch are production-shaped code that the tests
  **execute** on a Linux x86-64 host: real stack switches, real register save/restore, real blocking and waking, a real
  second thread racing wake-ups against the scheduler.
* The SYSCALL entry stub is assembled and disassembled but has **never been executed**.

## What is not

No ring 0, no QEMU, no `x86_64-unknown-none` build, no hardware paging, no preemption, no ARM64, no IPC integration.

## Layout

```
kernel/            lib.rs context.rs sched.rs syscall.rs uaccess.rs msr.rs sync.rs status.rs tests.rs
kernel/arch/x86_64/ context.S syscall_entry.S
docs/              ARCHITECTURE SYSCALL-INTEGRATION SCHEDULER-INTEGRATION CONTEXT-SWITCHING SECURITY-ANALYSIS TEST-PLAN
scripts/verify.sh  full verification (exit 0 / 1 / 2)
Cargo.toml  Cargo.lock  manifest.json
```

## Run

```
scripts/verify.sh                       # everything below, plus manifest and protected-path checks
cargo test --offline                    # 56 tests; needs Linux on x86-64, Rust >= 1.88
cargo fmt --check && cargo clippy --all-targets -- -D warnings
ZIG=/path/to/zig OMNI_BASE_REF=experiment/omni-kernel-batch-6 scripts/verify.sh
```
Set `CARGO_TARGET_DIR` outside the repository to keep build output out of the work tree (the script does by default).

## Depends on

Nothing at build time. It does not import Batches 2-6; `verify.sh` re-runs the Batch 6 suite (which includes 2-5) as a
regression only. Merge order for the branches is 4, 5, 6, 7.
