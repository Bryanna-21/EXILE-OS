# OMNI-Kernel Batch 4 report

## Identification

* Branch: `experiment/omni-kernel-batch-4`
* Base: `origin/main` = `3922a59aeb1beb484a266013f963d12c2b444a71` (Batch 3 merge)
* Commit SHA: reported in the hand-off message (a file cannot contain the hash of the commit that adds it);
  `git log -1 --format=%H -- Meta/Experiments/OMNI-Kernel-Batch-4` gives it.
* Compiler actually used: a locally built Zig 0.15.2 (stage3, no LLVM, Debug). An official release was not
  available in the build environment; verification with an official 0.15.2 remains pending.

## Commands run

```
export ZIG_LOCAL_CACHE_DIR=... ZIG_GLOBAL_CACHE_DIR=...   # outside the repo
cd Meta/Experiments/OMNI-Kernel-Batch-4
zig build test --summary all
bash scripts/verify.sh
```

## Results

See the verification section appended at hand-off (final numbers from `scripts/verify.sh`, run in a fresh
clone with empty caches). Test totals at the last full run before packaging:

| suite | tests |
|---|---|
| Batch 4 (`src/tests.zig`) | 55 |
| Batch 3 regression (unmodified sources) | 57 |
| Batch 2 regression (unmodified sources) | 59 |
| total | 171 |

The Batch 4 binary was additionally run 12 times in a row without a failure (thread tests).

## Pre-existing findings (not fixed, outside Batch 4)

* Batch 2 `scripts/verify.sh` is mode 644, so running it directly gives exit 126; run via `bash`.
  Via `bash`, its manifest check fails because `EXILE-OS-STATUS.md` is not listed in Batch 2's manifest.

## Design decisions worth reviewing

* In-flight capabilities live in a per-end hold space; queued bytes are charged via one accounting object per
  message. This avoids touching Batch 3 but costs one object and one handle slot per message.
* `move` cannot attenuate and the receiver always receives TRANSFER (Batch 3 limitation).
* A rolled-back `move` returns the capability under a new handle value (`SendReport.rebound`).
* Allocation failure is reported as `QuotaExceeded`, matching Batch 3.

## Unresolved issues and limitations

1. Revocation does not wake blocked waiters; they notice at the next wake-up (use timeouts/cancel).
2. `recv` partial-delivery path and `lost_on_rollback` are defensive and not exercised by any test.
3. Thread-safety is evidenced only by the stress tests listed in docs/CONCURRENCY.md; no ThreadSanitizer,
   no formal model, x86-64 Linux Debug only.
4. `WAIT-05` (no busy-waiting) is a CPU-time heuristic, and timing tests use generous bounds; they could be
   flaky on a very loaded machine.
5. A single global lock serialises everything; not a scalability design.
6. Batch 3 proposals (attenuating transfer, transactional multi-transfer, reservation without object,
   revocation wake-up) are in docs/PROPOSED-ABI-CHANGES.md and are not applied.
7. Mutation testing of Batch 4 was not performed.
