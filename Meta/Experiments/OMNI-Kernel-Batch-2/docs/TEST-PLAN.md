# OMNI-Kernel Batch 2 Test Plan

Status: maps each Batch 2 requirement to concrete tests in `src/tests.zig`. Actual counts and
outcomes of a run are recorded only in `REPORT.md`; this plan does not assert that anything passed.

All tests are hosted (an ordinary process on the build machine), deterministic (fixed PRNG seeds, no
clock, no I/O) and single-threaded. They exercise validation logic and bookkeeping. They do **not**
test hardware isolation, privilege separation, concurrency, timing, or any architecture backend.

Requirement letters follow the Batch 2 task statement, section 5 (A to H). A test name begins with its
ID. `build.zig` defines no test-filter option, so `zig build test` always runs the whole suite.

## A. Error conventions

| Requirement | Test |
|---|---|
| A1 Small, consistent error set covering invalid input, missing authority, exhausted quotas, invalid state, unsupported operations | ERR-01 (every error maps to a unique negative status; fails to compile if an error is unmapped) |
| A2 Mapping to the provisional ABI status, round trip | ERR-02, ERR-03 |
| A3 Unknown status values are never treated as success | ERR-04 |

## B. Identifiers and handles

| Requirement | Test |
|---|---|
| B1 Distinct object, process and handle types | ID-01 |
| B2 Zero is invalid for every type | ID-02 |
| B3 Handle packs index and generation; generation 0 is not constructible; a live handle is never 0 | ID-03 |
| B4 A handle alone confers no authority: only table validation does | CAP-02 (missing, out of range, never issued, garbage), CAP-03 (stale), CAP-05 (type confusion), CAP-04 (rights) |
| B5 Object id allocation never yields the invalid id and reports overflow without state change | ID-04, ID-05 |

## C. Capability checks (the eight mandatory cases)

| Requirement | Test |
|---|---|
| C1 Valid handle with required right succeeds | CAP-01 |
| C2 Missing handle rejected | CAP-02 |
| C3 Stale or invalid handle rejected (including after slot reuse) | CAP-03, CAP-12 (generation wraparound) |
| C4 Handle lacking the required right rejected | CAP-04 |
| C5 Unknown rights bits rejected (`fromRaw`, `lookupRaw`, `deriveRaw`, forged `@bitCast` values) | CAP-06 |
| C6 Rights cannot be increased through derivation | CAP-07 (superset, missing `duplicate`, re-derivation), CAP-15 (randomized) |
| C7 Release invalidates the handle; second release fails | CAP-09 |
| C8 Invalid operations leave state unchanged | CAP-10 (table and account compared by value), CAP-11 (full table), CAP-13/CAP-14 (quota) |

Additional capability tests: CAP-05 (object-type mismatch), CAP-08 (derived handle keeps object and
type; it survives release of its source, documenting that revocation is **not** implemented), CAP-15
(6000 seeded random operations against a reference model, also checking that released handles stay
invalid and that live handles are unique).

## D. Resource accounting

| Requirement | Test |
|---|---|
| D1 Allocation within limits succeeds (including exactly at the limit) | RES-01 |
| D2 Allocation exceeding limits fails | RES-02 |
| D3 Arithmetic overflow rejected | RES-03 |
| D4 Failed reservations do not modify counters (single account and across a parent chain) | RES-04 |
| D5 Release cannot underflow | RES-05 |
| D6 Invalid release does not corrupt accounting at any level | RES-06 |
| D7 Concurrency assumptions stated | Documented in `core/resources.zig` header, `CORE-INTERFACES.md` section 10; **not testable** by single-threaded tests |

Additional: RES-07 (hierarchy), RES-08 (child limit bounded by parent's remaining budget), RES-09
(zero amounts), RES-10 (independent classes), CAP-13 and CAP-14 (table charges `handles`).

## E. Memory and range validation

| Requirement | Test |
|---|---|
| E1 Zero-length ranges | MEM-01 |
| E2 Exact-end boundaries | MEM-02 |
| E3 Out-of-range offsets | MEM-03 |
| E4 Addition overflow | MEM-04 (also MEM-05 multiplication, MEM-07 alignUp) |
| E5 Invalid alignment where required | MEM-06, MEM-07 |
| E6 User-address window validation without dereferencing | MEM-08 |
| E7 Slice helper returns exactly the validated bytes | MEM-09 |
| E8 No arbitrary address is dereferenced | By construction: the helpers take integers or caller-owned slices; MEM-08 validates integers only. Verified by review, not by a test |

## F. IPC message validation

| Requirement | Test |
|---|---|
| F1 Round trip and empty payload | IPC-01 |
| F2 Truncated input at every length | IPC-02 |
| F3 Version | IPC-03 |
| F4 Flags, reserved bytes, op 0 | IPC-04 |
| F5 Message type | IPC-05 |
| F6 Message size (versus input and limit) | IPC-06 |
| F7 Declared payload length | IPC-07 |
| F8 Handle-transfer count | IPC-08 |
| F9 Size computation overflow, encode bounds | IPC-09, IPC-10 |
| F10 Transfer list: count, liveness, `transfer` right, duplicates; table unchanged | IPC-11 |
| F11 Deterministic rejection of malformed input; accepted messages satisfy invariants | IPC-12 (40,000 seeded cases) |
| F12 Composition of table, account and IPC checks | X-01 |

## G. Architecture boundary

| Requirement | Test |
|---|---|
| G1 A conforming backend satisfies the contract | ARCH-01 (mock) |
| G2 Missing or mistyped declarations are detected | ARCH-02 |
| G3 Backend numeric facts are validated | ARCH-03 |
| G4 No architecture is claimed operational | Documentation only (`src/arch/*/README.md`, `README.md`, `REPORT.md`); not a test |

## H. Verification script (`scripts/verify.sh`)

The script is not covered by unit tests. Its behaviour is checked by running it, and by running it on
deliberately broken copies, as recorded in `REPORT.md`:

| Check | How |
|---|---|
| Passes on the intact tree and exits 0 | Run it |
| Missing required file is reported and the exit code is 1 | Run it on a copy with a file deleted |
| Missing Zig is reported as a missing tool with exit code 2 (not as a test failure) | Run it with `ZIG` pointing at a non-existent path |
| A failing test makes it exit 1 and the failing output is shown | Run it on a copy with a test changed to fail |
| A formatting violation makes it exit 1 | Run it on a copy with a file deformatted |
| A manifest mismatch makes it exit 1 | Run it on a copy with a file byte changed |
| Optional tool missing is reported as SKIPPED, distinct from FAIL | Run it with `python3` absent from `PATH` |

## I. Known gaps in coverage

* No concurrency tests (the code is single-threaded by design).
* No automated tests on a second architecture, a freestanding target, or release modes. Manual
  one-off runs are recorded in `REPORT.md` (suite in three release modes: passed; x86-64 freestanding
  object probe: compiled; aarch64 probe: compiler error). See the toolchain constraint in
  `IMPLEMENTATION-PLAN.md`.
* No mutation testing or coverage measurement was run.
* The randomized tests use one fixed seed each; they are regression checks, not an exhaustive search.
* The test mock for `arch/interface.zig` proves the contract is checkable, not that any real backend
  conforms.
