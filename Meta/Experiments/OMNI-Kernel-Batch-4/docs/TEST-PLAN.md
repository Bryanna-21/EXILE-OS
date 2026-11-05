# Test plan (Batch 4)

Run `zig build test`. Totals and commands are in REPORT.md. IDs are the test-name prefixes.

| # | Required area | Tests |
|---|---|---|
| 1 | Endpoint creation, rights, ownership | IPC-01..04, IPC-11 |
| 2 | Bounded queue limits, no overflow, deterministic full behaviour | IPC-05, IPC-06, MODEL-01/02 |
| 3 | Message validation, limits | IPC-07, IPC-08 |
| 4 | Accounting, refund on receive/close | IPC-09, IPC-10, CLOSE-06/07 |
| 5 | Move and copy semantics | CAP-01, CAP-02, CAP-07, CAP-08 |
| 6 | Attenuation and escalation rejection | CAP-02, CAP-03 |
| 7 | Stale handles, exhausted slots | IPC-04, CAP-04, CAP-05 |
| 8 | Receiver quota exhaustion, retry | CAP-06 |
| 9 | Revocation races | REV-01..04, STRESS-02 |
| 10 | Allocation-failure injection (kernel and IPC allocators, every point, shifted growth boundaries), no leak | ALLOC-01..03 |
| 11 | Rollback, atomic publication | ALLOC-01, CLOSE-ROLLBACK, RECV-ZOMBIE, RECV-DEAD-SPACE |
| 12 | Timeout, cancel, cancel-vs-arrival | WAIT-01/02/03/04, CANCEL-01..06 |
| 13 | Close wakes waiters, closure semantics | CLOSE-01..05, CLOSE-09 |
| 14 | Exactly-once cleanup, teardown leak check | CLOSE-06/07/08, all `rig.deinit()` checks |
| 15 | Concurrency stress | STRESS-01..03, CANCEL-05, WAIT-05 |
| 16 | Batch 2 and Batch 3 regressions, unmodified | `zig build test` runs both suites |

Recorded seeds: MODEL-01 uses seeds 1..48 and MODEL-02 a fixed list; a failing seed is printed
(`MODEL FAILURE seed=N`) and replays deterministically. Allocation sweeps are deterministic.

Not covered: partial-delivery path of `recv` (defensive, unreachable through the public API in tests);
lost-on-rollback under concurrent sender-space exhaustion; memory-model and non-x86 behaviour; ThreadSanitizer.
