# Capability lifecycle (Batch 3)

Status: describes what `src/kernel.zig` and `src/gen_arena.zig` implement and what the tests in
`src/tests/` check. It is a hosted model. It enforces nothing against code in the same process.
Outcomes of actual test runs are recorded only in `REPORT.md`.

## 1. Objects and records

| Record          | Owner                   | Identified by                                                    | Lifetime                                                                |
| --------------- | ----------------------- | ---------------------------------------------------------------- | ----------------------------------------------------------------------- |
| Space           | Kernel                  | `SpaceId` (index + generation)                                   | From `createSpace` until it is closed and nothing is billed to it       |
| Capability node | The space that holds it | `Handle` (16-bit slot + 16-bit generation) in that space's table | From create/derive/transfer until release, revoke, or the space closing |
| Object          | Kernel registry         | `ObjectId` (never reused, never 0)                               | From `createObject` until the last node **and** the last pin are gone   |
| Pin             | Kernel                  | `OpToken` (index + generation)                                   | From `beginOp` to `endOp` (or kernel teardown)                          |

User code holds only values (`SpaceId`, `Handle`, `OpToken`). No pointer into a kernel table is ever
returned. A handle value alone confers nothing: every use is checked against the named space's table
(slot in range, slot live, generation equal, type equal, rights sufficient).

## 2. Operations

| Operation                                 | Needs                           | Effect                                                                                      | Errors                                                                                                                                           |
| ----------------------------------------- | ------------------------------- | ------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `createSpace(limits)`                     | Root budget                     | New space with a child account                                                              | `QuotaExceeded`                                                                                                                                  |
| `createObject(space, type, rights, opts)` | Open space                      | New object plus its root capability; charges 1 `handles` and the optional object charge     | `InvalidHandle` (stale space), `PeerDead` (closed), `InvalidArgument` (reserved rights bits), `Overflow` (object ids exhausted), `QuotaExceeded` |
| `lookup(space, h, type, rights)`          |                                 | Resolves; no state change                                                                   | `InvalidHandle`, `AccessDenied`, `InvalidArgument`, `PeerDead`                                                                                   |
| `derive(space, h, rights)`                | `duplicate` on `h`              | Child node in the same space; rights must be a subset                                       | `AccessDenied`, `InvalidHandle`, `InvalidArgument`, `QuotaExceeded`                                                                              |
| `release(space, h)`                       |                                 | Removes the node; children are re-parented; refunds 1 `handles`; drops the object reference | `InvalidHandle`, `PeerDead`                                                                                                                      |
| `revoke(space, h)`                        | `admin` on `h`                  | Removes every descendant of `h` in every space; `h` survives; returns the count             | `AccessDenied`, `InvalidHandle`, `PeerDead`                                                                                                      |
| `transfer(src, h, dst)`                   | `transfer` on `h`               | Moves the node to `dst` with a new handle; the old handle dies                              | `AccessDenied`, `InvalidHandle`, `InvalidArgument` (same space), `PeerDead`, `QuotaExceeded`                                                     |
| `beginOp / endOp / opStillAuthorized`     | Type and rights as for `lookup` | Pins the object for an operation                                                            | as `lookup`, plus `QuotaExceeded`                                                                                                                |
| `closeSpace(space)`                       |                                 | Releases every node in the space; destroys objects whose last reference this was            | `InvalidHandle`, `PeerDead`                                                                                                                      |
| `destroy()`                               | Single-threaded                 | Closes all spaces, runs remaining destructors, frees memory                                 | none                                                                                                                                             |

Allocation failure is reported as `QuotaExceeded`: Batch 1 and Batch 2 have no out-of-memory code
(see `PROPOSED-ABI-CHANGES.md`, item P1). Every fallible operation is **all-or-nothing**: it first
reserves capacity in every table it will write and charges accounts, and only then commits with code
that cannot fail. The allocation-failure tests (OOM-01 to OOM-05) sweep the failure point over every
allocation each operation makes.

## 3. Rights attenuation

- A new node's rights are a subset of its source's rights (`derive`), checked with
  `Rights.contains`. Escalation is `AccessDenied`. Reserved rights bits are `InvalidArgument`.
- `derive` requires `duplicate` on the source, as in Batch 2.
- `transfer` moves a node unchanged: rights are not altered. To hand over fewer rights, derive first,
  then transfer the derived node. This keeps one invariant: **every node's rights are a subset of its
  parent's rights**, which `checkInvariants` verifies for every node.
- Nothing in the kernel ever raises a node's rights after creation.

## 4. Ownership and lifetime rules

1. An object has `cap_refs` (live nodes referring to it) and `pins` (in-flight operations).
2. The object is destroyed when `cap_refs == 0 and pins == 0`. That condition is evaluated at every
   decrement. It can become true only once: a reference can only be created from an existing
   reference (a live node or a pin), so an object whose counts are both zero cannot be revived.
3. Destruction has two phases. Inside the critical section the object moves from `live` to `dying`
   and is queued on an operation-local list (no allocation). After the lock is released, the
   destructor callback runs, then a second short critical section refunds the object's charge and frees
   its registry slot. While `dying`, no lookup can reach the object, but its charge stays held.
4. The destructor runs exactly once per object. It may call back into the kernel (LC-09).
5. `revoke` never destroys an object, because the revoked node keeps its own reference.
6. Forced destruction of an object with live capabilities is **not implemented** (no `destroy_object`
   operation). Objects only die by losing all references.
7. At `Kernel.destroy()`, objects still pinned are destructed once, then freed; pins do not outlive
   teardown (LC-10).

## 5. Resource accounting

Each space has an `Account` (Batch 2 `resources.Account`) that is a child of the kernel's root
account, so root limits bound the sum over all spaces.

| Event                             | Charge                                                                                                                  |
| --------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `createObject`                    | `handles` +1 on the space; `opts.charge_resource` +`charge_amount` on the **creating** space (the "billed" space)       |
| `derive`                          | `handles` +1 on the space                                                                                               |
| `transfer`                        | `handles` +1 on the destination (first, may fail), then -1 on the source. The object charge stays with the billed space |
| `release`, `revoke`, `closeSpace` | `handles` -1 on the holder of each removed node                                                                         |
| Object destruction                | Object charge refunded to the billed space, after the destructor returns                                                |

**Zombie spaces.** If a space is closed while objects billed to it still exist (for example, it
created an object and transferred the only capability away), the space is kept as a closed record
until those objects are destroyed, so the refund has somewhere to go. A zombie rejects every
operation with `PeerDead`; once freed, its `SpaceId` is stale (`InvalidHandle`). Tests LC-08, REV-08.

`checkInvariants` recomputes every account's usage from the records and compares: usage equals the
node count (plus object charges), and root usage equals the sum over spaces.

## 6. Generation-safe handle reuse and exhaustion

All tables use one generic arena (`GenArena`).

- A slot's generation starts at 1 and is never 0. Freeing a slot increments it, so every earlier
  reference stops matching. A handle is therefore never 0 (GEN-05).
- **Exhaustion.** When a slot's generation is at its maximum and the slot is freed, the slot is
  **retired**: it is never allocated again, so no stale reference can ever match it. This costs one slot
  of capacity. Handle tables: a slot can be used 65,535 times (generations 1 to 65,535). When every
  slot is retired or live and the table is at its limit, creation fails with `QuotaExceeded` (ARN-03,
  ARN-04, GEN-02). Node, object, pin and space arenas use 32-bit generations and follow the same rule
  (GEN-03, GEN-04).
- A handle table has at most 65,535 slots (index 0xFFFF is reserved), so a space cannot hold more
  than 65,535 capabilities at once regardless of its quota.
- Stale handles and stale tokens are rejected after slot reuse: `Handle` by `InvalidHandle`,
  `OpToken` by `opStillAuthorized == false` and `endOp` returning `InvalidHandle` (GEN-01, GEN-03,
  REV-05).

## 7. Double release and invalid handles

- Releasing a handle twice: the second call is `InvalidHandle`; accounting is unchanged (LC-03).
- Zero, out-of-range, never-issued, stale, wrong-type handles: `InvalidHandle`. Missing right:
  `AccessDenied`. Reserved rights bits in a request: `InvalidArgument` (LC-01).
- A failed call changes no state (asserted by comparing statistics, usage and invariants in the
  allocation sweeps and the randomized model).

## 8. Comparison with Batch 1 decisions

Batch 1 (`Documentation/OMNI-Kernel/Batch-1/docs/ABI-AND-SECURITY.md`) is frozen and unchanged.

| Topic                                | Batch 1                                                                                                                    | Batch 3                                                                                                                                                                        | Consistent?                                      |
| ------------------------------------ | -------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------ |
| Handle structure                     | Index plus generation into a per-process table (B.2)                                                                       | 16-bit index + 16-bit generation per space (from Batch 2)                                                                                                                      | Yes                                              |
| Kernel custody                       | Authority lives in kernel memory (B.2)                                                                                     | Nodes live in the kernel arena; callers hold values                                                                                                                            | Yes                                              |
| Checks on every use                  | Index, occupied, generation, type, rights (B.2)                                                                            | Same, plus space open                                                                                                                                                          | Yes                                              |
| Derivation                           | Needs `DUPLICATE`, subset of rights (B.2)                                                                                  | Same                                                                                                                                                                           | Yes                                              |
| Transfer                             | Needs `TRANSFER`; move or copy over IPC; receiver gets a new entry (B.2)                                                   | Move only, between spaces, in-kernel. **Copy-transfer is not implemented** (derive then transfer gives the same effect with an explicit step). IPC integration is out of scope | Partial: subset of Batch 1                       |
| Lifetime                             | Exists until all handles are released, the object is destroyed, or revoked; process termination releases all handles (B.2) | Same, with `closeSpace` as process termination                                                                                                                                 | Yes                                              |
| Revocation                           | R2 derivation tree recommended, "bounded derivation depth"; races with in-flight operations unresolved (B.2, B.8)          | R2 implemented; **depth is not bounded** (iterative traversal; bounded only by node quota); in-flight rule defined in `REVOCATION-MODEL.md`                                    | Partial: no depth bound                          |
| Errors                               | A.4 codes; failed calls must not leak handles or charge (A.4 rule 3)                                                       | Same codes; closed space is `PEER_DEAD`; allocation failure maps to `QUOTA_EXCEEDED`                                                                                           | Yes, with the notes in `PROPOSED-ABI-CHANGES.md` |
| TOCTOU on handles                    | Operations using a handle must hold a reference to the object for their duration (B.3 item 6)                              | `beginOp` pins the object                                                                                                                                                      | Yes                                              |
| Resource charging                    | Failed call must not leave resources charged (A.4)                                                                         | All-or-nothing, tested                                                                                                                                                         | Yes                                              |
| R1 indirection for service authority | Recommended with R2                                                                                                        | Not implemented                                                                                                                                                                | Not addressed                                    |

No Batch 1 decision is contradicted. Gaps and proposed changes are listed in `PROPOSED-ABI-CHANGES.md`.
