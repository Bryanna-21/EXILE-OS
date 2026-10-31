# OMNI-Kernel Batch 5 report

**Scope statement:** this package is hosted and kernel-facing. It is **not integrated into a bootable
kernel**. No syscall is wired to hardware, no scheduler exists, and user memory is simulated.

## Identification

* Branch: `experiment/omni-kernel-batch-5`
* Base: Batch 4 commit `faa9273ee6c51f3150abaca3ab71b24e80bc325a` (branch `experiment/omni-kernel-batch-4`).
  `origin/main` was `3922a59aeb1beb484a266013f963d12c2b444a71` (Batch 3) when this was written and does not
  contain Batch 4, so Batch 4 must be merged first.
* Commit SHA: reported in the hand-off message (a file cannot contain the hash of the commit that adds it).
* Compiler actually used: a locally built Zig 0.15.2 (stage3, no LLVM, Debug). An official release was not
  available in the build environment; verification with an official 0.15.2 remains pending.

## Commands run

```
export ZIG_LOCAL_CACHE_DIR=... ZIG_GLOBAL_CACHE_DIR=...   # outside the repo
cd Meta/Experiments/OMNI-Kernel-Batch-5
zig fmt --check build.zig build.zig.zon src
zig build test --summary all
bash scripts/verify.sh
```

## Results (last full run before packaging)

| suite | tests |
|---|---|
| Batch 5 (`src/tests.zig`) | 57 (units 11, syscall 12, wait 17, resource 12, stress/fuzz 4) |
| Batch 4 regression (unmodified sources) | 55 |
| Batch 3 regression (unmodified sources) | 57 |
| Batch 2 regression (unmodified sources) | 59 |
| total | 228 |

The Batch 5 test binary was additionally run 12 times in a row without a failure (thread tests). The fuzz
test (STRESS-04) uses fixed seeds 1..24; failing seeds are printed. The final fresh-clone `verify.sh`
output is given in the hand-off message.

## 1. Implemented and tested

* Kernel-facing interface `KernelIpc` over Batch 4 (endpoints, cancellation objects, send, recv, handle
  release, revoke, space close, stats, audit) with rights checked on every operation.
* Syscall boundary dispatcher: closed number table, register and handle width checks, reserved-field checks,
  bounded sizes, checked address arithmetic, copy-in/decode and probe-before-consume, status mapping.
* Bounded eventcount wait model: no lost wake-up (20000-change test), deadlines, cancellation, wake on
  close/revoke/space close/release through the wrappers, bounded waiters per space and in total.
* Resource accounting and audit: queue bounds, quotas, capability transfer and rollback, exactly-once
  destruction, no leaks across 400-cycle create/destroy and teardown, allocation-failure sweeps on every
  point of syscall send and recv.
* Concurrency: 4 producers x 3 consumers exactly-once and ordered; revoke racing capability sends; close
  under load (60 rounds); timeout-vs-arrival and cancel-vs-arrival races; seeded syscall fuzz.

## 2. Implemented but not fully verified

* Thread-safety: evidence is stress testing only, on x86-64 Linux, Debug build, one compiler. No formal
  argument, no other architecture, no sanitizer or model checker run.
* Batch 4's partial-delivery path and "lost on rollback" path are defensive and have no test that reaches them.
* Per-space queued-message and in-flight-capability limits are enforced through the Batch 3/4 bounds; no shadow
  counters exist in this layer, so they are tested through behaviour (RES-01..05) only.
* Wake-ups after changes made directly on Batch 3/4 (not through the wrappers) arrive only at the deadline
  (documented limit, BLK-11).
* Timing tests (BLK-03, BLK-15) use wall-clock thresholds; they passed in every run here but are
  environment-dependent.

## 3. Specified but not implemented

* x86-64 and ARM64 syscall entry adapters (register mappings in `docs/SYSCALL-CONTRACT.md`).
* Real user/kernel memory copying, page-fault handling and address-space isolation.
* Scheduler integration (what a scheduler must provide is in `docs/SCHEDULER-CONTRACT.md`).

## 4. Deferred

* Fairness and priority policy for waiters; per-waiter wake-up instead of broadcast.
* Network or cross-machine IPC, persistence, shared memory.
* A final syscall ABI: numbers and layouts here are PROVISIONAL.
* Official Zig 0.15.2 run and other architectures.

## Known behaviours to review

* A fault while copying a received message out, after it was consumed, drops the message (capabilities are
  released, event counted). Probing first makes this rare but not impossible.
* The Batch 3 TRANSFER right cannot be stripped on a move; a rolled-back move returns under a new handle
  value, which the syscall layer reports to user memory.
* Blocking `send`/`recv` always make one last attempt after a timeout, so arrival beats timeout.

## Pre-existing findings (not fixed, outside Batch 5)

* Batch 2 `scripts/verify.sh` is mode 644 (exit 126 when run directly; run via `bash`), and Batch 2's
  manifest does not list `EXILE-OS-STATUS.md`.

## Layout deviation

An extra source file, `src/hosted_user_memory.zig`, holds the simulated user memory; it is used by the
tests and is not part of the kernel-facing interface.
