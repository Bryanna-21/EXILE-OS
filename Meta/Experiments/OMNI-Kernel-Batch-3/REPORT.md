# OMNI-Kernel Batch 3 report

Reviewer-facing record of what was built, what was run and what is not established. Every result
below was observed in the authoring session on the machine and compiler described in section 2.
Anything not listed there was not run.

## 1. Scope delivered

- `src/gen_arena.zig`: generation-checked arena used for every table.
- `src/kernel.zig`: `Kernel` with space, object and capability lifecycle, derive, release, revoke
  (derivation tree, option R2), transfer, in-flight operation pins, accounting, invariant checker,
  teardown.
- `src/tests/`: 56 named Batch 3 tests plus one anchor block that imports them (57 entries in the
  runner's count).
- `build.zig`: builds the Batch 3 suite and the **unmodified** Batch 2 suite (59 tests) as separate
  executables. Batch 2 sources are copied at build time into a generated module; nothing in Batch 2
  is edited.
- Documents: `README.md`, this report, `docs/CAPABILITY-LIFECYCLE.md` (includes the comparison with
  Batch 1), `docs/REVOCATION-MODEL.md`, `docs/CONCURRENCY.md`, `docs/TEST-PLAN.md`,
  `docs/PROPOSED-ABI-CHANGES.md`, `EXILE-OS-STATUS.md`, `manifest.json`, `scripts/verify.sh`.

Not delivered: see "What Batch 3 does not deliver" in `README.md`.

## 2. Environment

| Item        | Value                                                                                                                                                                                                   |
| ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Base        | Repository `Bryanna-21/EXILE-OS`, branch `main` at commit `45fbe98c` ("Add experimental OMNI Kernel Batch 2")                                                                                           |
| Work branch | `experiment/omni-kernel-batch-3`                                                                                                                                                                        |
| Host        | Linux 6.18.44 x86_64, Debug and release runs on this one machine                                                                                                                                        |
| Compiler    | Zig 0.15.2 built locally from tag `0.15.2` **without LLVM** (`zig version` prints `0.15.2`; binary SHA-256 `448d395e06f42ee6d33e85f02fc4a9095ccbe4febe8f494730062d7fc092242e`). Not an official release |
| Cache       | `ZIG_LOCAL_CACHE_DIR` and `ZIG_GLOBAL_CACHE_DIR` pointed outside the repository                                                                                                                         |

Because the compiler is a no-LLVM build, optimized-code quality, other targets and some linker features
are unavailable. The aarch64 backend of this compiler failed on Batch 2 and was not tried again.

## 3. Results actually observed

All commands were run from `Meta/Experiments/OMNI-Kernel-Batch-3` with `zig` 0.15.2.

| Command                                               | Result                                                                                                                        |
| ----------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `zig build test --summary all` (Debug)                | `6/6 steps succeeded; 116/116 tests passed`: Batch 3 executable 57 passed, Batch 2 executable 59 passed                       |
| `zig build test -Doptimize=ReleaseSafe --summary all` | `116/116 tests passed`                                                                                                        |
| `zig build test -Doptimize=ReleaseFast --summary all` | `116/116 tests passed`                                                                                                        |
| `zig fmt --check build.zig build.zig.zon src`         | exit 0 (the compiler prints allocator-leak diagnostics about itself on stderr; the exit status is what counts, as in Batch 2) |

Final-state results from the committed tree (fresh clone, `scripts/verify.sh`, `git diff --check`,
manifest validation) are reported in the hand-off message, because a file cannot contain the hash of
the commit that contains it. `scripts/verify.sh` reproduces them.

The tests do not print per-test names on success. The failure messages shown during authoring
(sections 5 and 7) are from real runs.

## 4. Mutation checks

Twelve deliberate defects were injected one at a time into a scratch copy and the whole suite was
run. "Detected" means the run failed (exit status 1). No mutant survived.

| Mutant                                                    | Detected by                                                                                                                 |
| --------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| M1 release does not re-parent children                    | Crash in `unlinkFromParent` ("attempt to use null value") during a test, so the runner aborted before reporting a test name |
| M2 derive skips the rights-subset check                   | LC-02, RND-01, CON-06, CON-07                                                                                               |
| M3 transfer does not refund the source                    | LC-05, LC-06, LC-08, REV-03 and others                                                                                      |
| M4 object destroyed while pinned                          | REV-06 and others                                                                                                           |
| M5 arena free keeps the generation                        | ARN-01 and others                                                                                                           |
| M6 destructor called with the lock held                   | LC-03 and others                                                                                                            |
| M7 no handle-charge rollback when the object charge fails | ACC-03 and others                                                                                                           |
| M8 revoke needs no `admin`                                | REV-02 and others                                                                                                           |
| M9 revoke also removes the named node                     | LC-04 and others                                                                                                            |
| M10 arena never retires exhausted slots                   | Integer-overflow panic in ARN-03                                                                                            |
| M11 `closeSpace` leaves nodes in place                    | Assertion failure in `freeSpace`                                                                                            |
| M12 object charge not refunded at destruction             | LC-08 and others                                                                                                            |

Three of the mutants (M1, M10, M11) were caught by a crash or assertion, not by a test comparing a
value. Detection by a crash is still detection, but it means those tests do not isolate the
mutant to one named assertion. The mutants and the script that ran them are not part of the package.
The classification printed by the first run of the script labelled M3 and M5 "COMPILE-ERROR" because
of a crude text match; their output listed failing test names, so they compiled and ran.

## 5. Defects found and fixed during authoring

| Where                | What                                                                                                    | Fix                                                                                                                 |
| -------------------- | ------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| Test LC-06           | Expected `PeerDead` for a freed space; a freed space is correctly `InvalidHandle`                       | Test now builds a real zombie for the `PeerDead` case                                                               |
| Tests CON-04, CON-07 | The `tryLock` probe reported "lock held in destructor" because other threads legitimately held the lock | Probe removed from multi-threaded tests; kept in single-threaded tests and replaced by CON-01 for the threaded case |
| Test OOM-03          | Derive never allocated because arrays had spare capacity, so no failure was injected                    | Setup now fills the arrays so the derive must grow them; the sweep asserts a failure occurred                       |
| Model test setup     | Per-space budget equal to the root budget made later `createSpace` fail                                 | Smaller per-space budget                                                                                            |

No defect in `kernel.zig` or `gen_arena.zig` was found by the tests after the first compile; the
mutation results in section 4 are the evidence that the tests are able to find such defects.

## 6. Comparison with Batch 1

See `docs/CAPABILITY-LIFECYCLE.md` section 8. No Batch 1 decision is contradicted. Partial
implementations: transfer is move-only; derivation depth is not bounded. Not addressed: R1.
Proposed changes are in `docs/PROPOSED-ABI-CHANGES.md`; Batch 1 is not modified.

## 7. Preflight findings about existing files (not changed)

- `Meta/Experiments/OMNI-Kernel-Batch-2/scripts/verify.sh` fails its manifest check on the committed
  tree because `EXILE-OS-STATUS.md` is in the directory but not in `manifest.json`. Batch 2 was not
  modified; the Batch 3 manifest lists its own status file.
- Some existing Markdown files in the repository do not pass the repository's Prettier version
  here (version mismatch). The Batch 3 Markdown was written to the same conventions as Batch 2; see
  the hand-off message for what linting was run.
- `shellcheck` was not available, so `scripts/verify.sh` was checked only with `bash -n`.

## 8. Unresolved risks

1. **Single global lock.** `revoke` and `closeSpace` hold the lock for time proportional to the
   subtree or space size. A hostile or merely large tree blocks every other operation.
2. **No derivation depth bound.** Batch 1 recommended one. Depth is limited only by the node quota.
   Adding a bound requires re-computing depths when `release` re-parents (proposal P7).
3. **Allocation under the lock.** Arena growth and space creation call the allocator inside the
   critical section; a slow or re-entrant allocator would hurt or deadlock.
4. **Allocation failure is reported as `QuotaExceeded`.** The cause is hidden (proposal P1).
5. **Destructor failure modes.** A destructor that never returns pins its object, its charge and (for
   a closed space) the zombie record forever. A destructor that panics aborts the process.
6. **Authority check at one point.** Operations that use `lookup` without `beginOp` have a
   time-of-check/time-of-use gap by design. Nothing enforces that callers use `beginOp`.
7. **Retirement shrinks capacity.** A handle slot is retired after 65,535 reuses; a space that
   churns heavily eventually loses slots until it hits `QuotaExceeded`. There is no compaction.
8. **Zombie spaces.** A space with surviving billed objects stays in memory and its `max_spaces`
   slot stays used. Billing stays with the creating space even when a different space holds the only
   capability (proposal P10).
9. **Teardown contract.** `Kernel.destroy` is not safe against concurrent users, and runs
   destructors of pinned objects.
10. **Hosted model only.** Nothing protects the kernel structures from the code that calls them.

## 9. Untested or unproven claims

- Thread safety rests on the single-lock argument in `docs/CONCURRENCY.md` plus stress and
  forced-interleaving tests. No thread sanitizer, model checker or weak-memory machine was used.
- Behaviour under more than four threads, long durations, or on a second machine was not measured.
- The tests ran in Debug, ReleaseSafe and ReleaseFast with a no-LLVM compiler on one machine; no
  other compiler or target was used.
- The 65,535-slot handle table limit was not exercised with real objects; the retirement mechanism
  was exercised with smaller arenas and forced generations (ARN-03, ARN-04, GEN-02, GEN-03).
- Interleavings of `revoke` with `beginOp`/`endOp` on different threads are covered only by the
  random mix in CON-07 and by sequential tests, not by a forced interleaving.
- The documents describe design intent for later kernel work (for example R1, synchronous
  revocation); none of that is implemented or tested.
- `scripts/verify.sh` was not run with an official Zig release, and was not linted with ShellCheck.
- No claim of isolation, memory safety of a real kernel, or security is made.
