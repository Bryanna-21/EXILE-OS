# OMNI-Kernel Batch 2 Security Review

Status: a **self-review by the author** of the Batch 2 code and design. It is not an independent
review, not an audit, and not a proof. It separates three kinds of statement and labels each:

* **Tested property** — checked by an automated test in `src/tests.zig` (results in `REPORT.md`).
* **By construction / reviewed** — follows from how the code is written; not covered by a test.
* **Design intention** — a goal for a future kernel; not implemented in Batch 2.

The code is a hosted model. It runs inside an ordinary process, so none of it provides hardware
privilege separation or memory isolation. The threat model is section B.12 of `docs/ABI-AND-SECURITY.md` in the separate Batch 1 package (not included here); this
review covers only what Batch 2 touches.

## 1. Scope

In scope: the validation and bookkeeping logic in `src/core/` and the architecture contract in
`src/arch/interface.zig`.

Out of scope: everything that does not exist yet (boot, MMU, syscall entry, scheduler, drivers,
real IPC transport), side channels, physical attacks, supply-chain risk (including the toolchain; see
section 5), and concurrency.

## 2. Threats and mitigations

| # | Threat | Mitigation present in Batch 2 | Status | Gap |
|---|---|---|---|---|
| T1 | Forging a handle (guessing values, zero, out-of-range index) | Every use is checked against the owning table: index range, live state, generation | Tested (CAP-02, CAP-15) | Hosted table has no isolation: any code in the process could edit it. Real protection needs kernel/user separation (not implemented) |
| T2 | Using a stale handle after release or slot reuse (use-after-release, ABA) | Generation counter per slot; slot retired on wrap | Tested (CAP-03, CAP-12, CAP-15) | Only the table is covered; object lifetime and reference counts are not implemented, so a handle can name an object that no longer exists in a real kernel |
| T3 | Type confusion between objects | `lookup` requires an expected `ObjectType` | Tested (CAP-05, CAP-15) | Callers must pass the correct expected type; nothing forces a call site to be correct |
| T4 | Rights escalation through derivation | `derive` requires `duplicate` and a subset of rights | Tested (CAP-07, CAP-15) | Revocation absent (see T5) |
| T5 | Retaining authority after it should be withdrawn | **None.** Derived handles survive release of their source | Tested as a limitation (CAP-08) | Revocation is **not implemented**; Batch 1 options R1-R3 remain open |
| T6 | Unknown or reserved rights bits slipping through as permissions | Rights parsed with `fromRaw`; reserved bits rejected; `isValid` guard on forged values | Tested (CAP-06) | A `@bitCast` can still create an invalid `Rights` value in code; the table rejects it at `insert`/`lookup`/`derive`, but other future consumers must do the same |
| T7 | Resource exhaustion (handles, pages, threads, channels, queued bytes) | Per-account limits, hierarchical charging, fixed table capacity | Tested for the accounting arithmetic (RES-*, CAP-11, CAP-13, CAP-14) | Not wired to real allocation or queues; a real kernel must charge at every allocation site. No CPU accounting |
| T8 | Counter overflow or underflow corrupting accounting | Checked arithmetic; two-pass reserve/release | Tested (RES-03, RES-05, RES-06) | Callers can still release the wrong class or amount if they pass wrong arguments (the underflow check catches only some mistakes) |
| T9 | Partial state change on failure | All checks before commits | Tested (CAP-10, CAP-11, CAP-13, RES-04, RES-06, ID-05) | Only for the operations tested; future operations must follow the same rule |
| T10 | Integer overflow in sizes and offsets | `std.math` checked operations throughout | Tested (MEM-04, MEM-05, MEM-07, IPC-09) | Zig safety checks are disabled in `ReleaseFast`/`ReleaseSmall`; the code uses explicit checks rather than relying on them. A manual run of the suite in those modes passed (`REPORT.md`), but with a compiler that has no LLVM optimizer (section 5), so optimized code generation was not exercised |
| T11 | Out-of-range or misaligned buffers and addresses | `validateWithin`, `UserWindow.validateSpan`, `checkAligned` | Tested (MEM-01 to MEM-09) | Validation is arithmetic only: it does **not** make a user pointer safe to dereference. Mapping/permission checks, fault handling and copy-in are not implemented |
| T12 | Time-of-check/time-of-use on user memory | `ipc.decode` works on a caller-supplied byte slice and returns slices of it; no hidden re-read | By construction | In a kernel the slice must be a private copy made before validation (Batch 1 B.3 rule 6). No copy-in exists in Batch 2; shared-memory hazards are unaddressed |
| T13 | Malformed IPC messages (bad size, lengths, kinds, versions, counts) | Fixed-order validation; exact-size rule; limits | Tested (IPC-01 to IPC-10, IPC-12 fuzz) | Fuzz volume is modest and seeded; validation covers the header only, not payload semantics (service responsibility) |
| T14 | Transferring handles the sender may not transfer, stale handles, or duplicates | `validateTransferList` | Tested (IPC-11, X-01) | Does not perform the transfer; atomic move between tables, in-flight ownership and cleanup on peer death are not implemented |
| T15 | IPC queue flooding / denial of service | Message size and handle limits; `queued_bytes` account class | Partly tested (limits: IPC-06, IPC-08; accounting: RES-*, X-01) | There are no queues. Flooding resistance is a design intention |
| T16 | Wrong or hostile architecture facts (page size, user window) | `validateFacts` | Tested (ARCH-03) | Facts come from trusted backend code in a real kernel; this only catches configuration mistakes |
| T17 | Information leakage through error distinctions | Not addressed | Design intention only | `InvalidHandle` versus `AccessDenied` distinguishes "no such handle" from "no right"; this is within the caller's own table, but service-level existence leaks (Batch 1 C.1) are not modelled |
| T18 | Predictable or reused handle values | Not relied upon: handle values are not secrets | By design | Batch 1 B.2 states security comes from table custody, not unpredictability |
| T19 | Side channels (timing, cache, speculation) | None | Out of scope | Lookup is not constant-time; no claim is made |
| T20 | Untrusted hardware, firmware, DMA | None | Out of scope | No driver or boot code exists |

## 3. Missing mitigations (not implemented in Batch 2)

* Hardware-enforced separation of user and kernel memory and of processes.
* Capability revocation; handle transfer between tables; object reference counts and destruction.
* Safe copy-in/copy-out with fault handling; mapping and permission checks on user pointers.
* Cancellation objects and safe cleanup of blocked operations.
* Locking or atomics; any claim about concurrent correctness.
* Audit logging and rate-limited security events.
* Entropy and random-number services (no cryptography is implemented, by instruction).
* Secure boot chain, IOMMU/DMA protection, side-channel mitigations.
* Real IPC with queues, scheduling, and peer-death handling.

## 4. Unresolved issues and open risks

1. **Hosted model versus kernel reality.** Rules proved in a hosted table can still be violated in a
   kernel through unchecked callers, concurrency, or memory corruption. Nothing here shows otherwise.
2. **Retired slots shrink capacity.** A slot retires after 65,535 releases. A long-lived process with
   heavy handle churn would eventually exhaust a fixed table. Whether to widen the generation, reset
   a table, or grow it is undecided.
3. **`peek` bypasses rights and type checks by design.** It exists for kernel-internal validation and
   is a hazard if it is ever reachable from user input. Callers must be reviewed when a syscall layer
   is added.
4. **`@constCast` in the table.** `resolveMut` derives a mutable entry pointer from a const one inside
   a method that already holds `*Self`. It is sound here but is a pattern that should be reviewed if
   the table internals change.
5. **Zero-length ranges at the upper bound are valid.** Callers that then compute with `end` or treat
   such a span as "a mapping" must handle the empty case; this is a classic source of off-by-one
   mistakes at the call site.
6. **Account parent pointers.** A moved or freed parent leaves children with a dangling pointer; the
   module cannot detect this. A real kernel should store accounts in a stable arena or use indices.
7. **Provisional ABI choices.** DEC-01 to DEC-12 in `CORE-INTERFACES.md` have not been validated
   against an implementation of the ABI. Early adoption by other components could entrench mistakes.
8. **Test confidence.** Randomized tests use one seed each and were run on one machine in one mode.
9. **Self-review limits.** The author wrote both the code and this review. Independent review is needed
   before any security claim is made.

## 5. Toolchain provenance and limits

The Zig compiler used for Batch 2 was built from the official source tag with the project's own
no-LLVM bootstrap procedure because no pre-built compiler could be downloaded in the authoring
environment. Details, including the source commit, are in `REPORT.md`. This is a supply-chain and
fidelity risk: the compiler was built by the author's environment, not downloaded as an official
signed release, and it lacks LLVM optimization. The scripted results are for Debug-mode builds on that
compiler; manual release-mode runs are noted in `REPORT.md`. Re-running `scripts/verify.sh` with an official Zig 0.15.2 release is the intended
way to remove this uncertainty (plan milestone M1).

## 6. What this review does not claim

It does not claim the capability model is secure, that memory is isolated, that the IPC layer is safe
against a hostile peer, that the design is verified, or that any component is production-ready.
