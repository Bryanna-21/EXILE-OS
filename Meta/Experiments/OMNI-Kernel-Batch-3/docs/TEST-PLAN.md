# OMNI-Kernel Batch 3 test plan

Status: maps each Batch 3 requirement to the tests in `src/tests/`. Actual outcomes are recorded only
in `REPORT.md`; this plan asserts nothing about results.

All tests are hosted. Most are deterministic (fixed seeds, no clock). The concurrency tests use real
threads; their interleavings are partly forced with events and partly left to the scheduler, so
those tests are not deterministic in timing, only in what they assert. Test names begin with their ID.
`zig build test` runs the Batch 3 suite and the unmodified Batch 2 suite (59 tests) as separate test
executables. There is no test-filter option in `build.zig`.

## A. Capability lifecycle

| Requirement                                                                 | Tests         |
| --------------------------------------------------------------------------- | ------------- |
| Create, lookup, type and rights checks, bad inputs                          | LC-01         |
| Derive with attenuation, escalation rejected, failed derive changes nothing | LC-02         |
| Release; double release; object lives while any capability lives            | LC-03         |
| Release re-parents children                                                 | LC-04, REV-04 |
| Transfer moves, rights kept, tree position kept                             | LC-05         |
| Transfer errors change nothing                                              | LC-06         |
| Close space; stale space id                                                 | LC-07         |
| Zombie space and deferred refund                                            | LC-08         |
| Destructor outside the lock, re-entrant, exactly once                       | LC-09         |
| Teardown destroys remaining objects once                                    | LC-10         |
| Object ids never reused or zero                                             | LC-11         |

## B. Generation safety and exhaustion

| Requirement                                                                         | Tests            |
| ----------------------------------------------------------------------------------- | ---------------- |
| Stale handles rejected after slot reuse                                             | GEN-01, REV-05   |
| Handle-table generation exhaustion retires the slot                                 | GEN-02, ARN-03   |
| Node generation exhaustion; stale operation tokens                                  | GEN-03           |
| Stale space id after slot reuse                                                     | GEN-04           |
| Handle never 0                                                                      | GEN-05, ARN-04   |
| Arena behaviour: limit, retirement, free list, allocation failure, randomized model | ARN-01 to ARN-07 |

## C. Revocation

| Requirement                                                 | Tests                  |
| ----------------------------------------------------------- | ---------------------- |
| Descendants removed, node kept, siblings and ancestors kept | REV-01                 |
| `admin` required; failure changes nothing                   | REV-02                 |
| Propagation across spaces                                   | REV-03                 |
| Revocation after an intermediate release                    | REV-04                 |
| Stale authority after slot reuse                            | REV-05                 |
| In-flight operation, object lifetime, double `endOp`        | REV-06                 |
| `beginOp` checks; authority across transfer                 | REV-07                 |
| Closing the holder's space                                  | REV-08                 |
| Deep chain, no recursion                                    | REV-09                 |
| Derive before/after revoke                                  | REV-10                 |
| Atomic revoke; derive/revoke race; revoke/transfer race     | CON-02, CON-03, CON-06 |

## D. Resource accounting

| Requirement                                            | Tests            |
| ------------------------------------------------------ | ---------------- |
| Charge/refund through create, derive, release, destroy | ACC-01, ACC-07   |
| Charge held during the destructor                      | ACC-02           |
| Rollback on failure within one operation               | ACC-03           |
| Transfer charges destination first                     | ACC-04           |
| Root budget bounds spaces                              | ACC-05           |
| Table size limit                                       | ACC-06           |
| Conservation under allocation failure                  | OOM-01 to OOM-05 |
| Conservation under concurrency                         | CON-05, CON-07   |

## E. Allocation failure

OOM-01 to OOM-05 fail the allocator at every point inside `createSpace`, `createObject`, `derive`,
`transfer` and `beginOp` (the setup is arranged so that each operation actually grows a table) and
compare kernel statistics and root usage before and after, and run the invariant checker. OOM-06
shows `release`, `revoke`, `endOp` and `closeSpace` never allocate.

## F. Concurrency

CON-01 to CON-07 (see `CONCURRENCY.md` section 5). Single-threaded support: LC-09, LC-07, OOM-06.

## G. Randomized reference model

RND-01 to RND-03: 2,500 random steps each, three fixed seeds, against a separately written model.
Each step compares the result (including the exact error), every live handle's rights and object,
a sample of dead handles (all of them every 50th step), per-space accounting, object, node and pin
counts, destructor counts and `checkInvariants`.

## H. Regression

The 59 Batch 2 tests are compiled from the unmodified `../OMNI-Kernel-Batch-2/src/tests.zig` by
`build.zig` and run on every `zig build test`.

## I. Mutation checks performed during authoring

Selected deliberate defects were injected into a scratch copy of the sources to confirm the suite
detects them. The list and results are in `REPORT.md`. The mutants are not part of this package and
are not rerun by `scripts/verify.sh`.

## J. Known gaps

- No test under a race detector, in a release mode, on a second architecture, or on a second machine.
- No test with more than four threads or for longer than a few seconds.
- The randomized model uses three fixed seeds and 2,500 steps; it is a regression check, not an
  exhaustive search.
- No test exercises the 65,535-slot table limit with real objects (ARN-03/ARN-04 and ACC-06 cover
  the mechanism with smaller arenas and forced generations).
- Interleavings of `revoke` with `beginOp` on another thread are covered only by the sequential tests
  (REV-06) and by CON-07's random mix, not by a forced interleaving.
- Destructors that block forever are not tested.
