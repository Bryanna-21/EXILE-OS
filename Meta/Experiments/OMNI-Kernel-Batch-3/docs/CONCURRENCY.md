# Concurrency model (Batch 3)

Status: states the ownership model and synchronization invariants that `src/kernel.zig` is written
to and that the tests exercise. It is a **hosted** model using operating-system threads and
`std.Thread.Mutex`. It says nothing about interrupts, preemption of kernel code, multiple CPUs in a
real kernel, memory-ordering behaviour on weakly ordered hardware, or lock-free structures.

## 1. What is claimed

- `Kernel` is **internally synchronized**: every public method may be called from any number of
  threads. All kernel state is protected by one mutex, `Kernel.mutex`.
- Each public method is **linearizable** at its commit critical section (see `REVOCATION-MODEL.md`
  section 4).
- Object destructors run **outside** the lock, exactly once per object.
- The batch 2 core modules (`ids`, `capabilities`, `resources`) are single-threaded and are used only
  while the kernel mutex is held. `resources.Account` keeps no lock of its own, which is correct only
  because the kernel is the single user of each account.

The claim is limited to what the tests below exercise. No thread sanitizer was available in the
authoring environment (the compiler is a Debug-only build without LLVM), so data-race freedom is argued
from the lock discipline in this file, not detected by a tool.

## 2. Ownership

| State                                                                   | Owner                      | Guarded by                                                  |
| ----------------------------------------------------------------------- | -------------------------- | ----------------------------------------------------------- |
| Arenas (spaces, objects, nodes, pins) and every handle table            | Kernel                     | `Kernel.mutex`                                              |
| `Space` structs (heap allocated) including account                      | Kernel                     | `Kernel.mutex`                                              |
| Object id allocator                                                     | Kernel                     | `Kernel.mutex`                                              |
| Per-operation destroy list                                              | The calling thread's stack | Nobody else can see it                                      |
| Values returned to callers (`SpaceId`, `Handle`, `OpToken`, `Resolved`) | Caller                     | They are copies; they confer nothing without a kernel check |

No pointer to kernel state escapes. Inside the kernel, links between records are table **indices**
that are updated only under the lock; pointers into arenas are never held across an allocation
(`reserveOne` can reallocate), which is enforced by structure: each function reserves first, then
re-reads what it needs, then commits with non-allocating code.

## 3. Lock rules

1. **One lock, so no lock order exists.** There is no second lock to order against. The test helper
   locks (`Tracker.lock`, mailbox locks) are test-side and never held while calling the kernel.
2. **Lock only at the public boundary.** Public methods take the lock; private helpers assume it is
   held. No public method calls another public method while holding the lock.
3. **Fallible work before mutation.** Allocation and account reservation happen first; the commit
   after them cannot fail. On error nothing has changed.
4. **No callbacks under the lock.** The only callbacks are object destructors. They are queued as
   `dying` objects during the critical section and run after the lock is released
   (`finishDestroy`). Justification: a destructor may take arbitrary time or re-enter the kernel
   (LC-09), and a callback under a non-recursive mutex would deadlock.
5. **The allocator is called under the lock** (arena growth, space creation and teardown). The
   allocator must not call into the kernel. This is a deliberate trade for atomic all-or-nothing
   operations; its cost is that a slow allocator lengthens the critical section.
6. **Dying objects stay occupied until finalized.** Between the commit and the end of the destructor
   an object's registry slot and its charge remain, so a concurrent operation can never see a freed
   slot; it simply cannot reach the object. `finalizeObject` takes the lock again to refund and free.
7. **Teardown** (`Kernel.destroy`) requires that no other thread uses the kernel. It is not
   synchronized against concurrent callers.

## 4. Invariants (checked by `checkInvariants`)

- Each arena's free list and counters are consistent; generations are non-zero.
- Every live node: its object is live, types agree, its holder space is open, its handle slot points
  back to it exactly (index and generation), its rights are valid, its rights are a subset of its
  parent's, it shares its parent's object, sibling and child links are symmetric, and the parent chain
  has no cycle.
- Every live object: `cap_refs` equals the number of nodes referring to it; `pins` equals the number of
  pins; at least one is non-zero. A `dying` object has neither.
- Every space: its table size equals its node count; its billed-object count matches the registry;
  a closed space has no capabilities and at least one billed object (otherwise it would have been freed).
- Accounting: each account's usage equals the recomputed usage (node count for `handles`, plus object
  charges for the billed resource), and the root account's usage equals the sum over spaces.

The checker takes the lock and allocates nothing. It is exercised after every step of the randomized
tests and at the end of the stress test.

## 5. Concurrency tests and what each proves

| Test                                 | Scenario                                                                                                  | Evidence it gives                                                                                                                                                |
| ------------------------------------ | --------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| CON-01                               | One thread releases the last capability; its destructor blocks on an event while the main thread inspects | The removal is visible (lookup, derive, revoke fail), the kernel is usable, the lock is free, the object is `dying` and still charged, while the destructor runs |
| CON-02                               | Observer reads two sibling descendants while the main thread derives two and revokes them, 3,000 times    | No partial revoke is observed                                                                                                                                    |
| CON-03                               | One thread derives while another revokes                                                                  | Everything that resolves afterwards is accounted for, invariants hold, a final revoke leaves only the root                                                       |
| CON-04                               | Two threads release the same 3,000 handles in opposite orders                                             | Exactly one release per handle succeeds; each destructor ran exactly once                                                                                        |
| CON-05                               | Two threads transfer the same 2,000 handles to different spaces                                           | Each handle moves at most once; destination accounts match the winners                                                                                           |
| CON-06                               | One thread derives and transfers while another revokes                                                    | No leak across spaces; a final revoke leaves one node                                                                                                            |
| CON-07                               | Four threads, 4,000 random operations each, with transfers via mailboxes                                  | Invariants hold; after closing all spaces every created object was destroyed exactly once and every account is zero                                              |
| LC-09, LC-07, OOM-06 (single thread) | Destructor probes `tryLock`, re-enters the kernel; release/revoke/close allocate nothing                  | The lock is not held during a destructor; teardown paths cannot fail for lack of memory                                                                          |

The `tryLock` probe is only meaningful single-threaded: with several threads, a failed `tryLock` may
just mean another thread holds the lock. The multi-threaded tests therefore do not use it; CON-01
establishes the same fact with only one other thread able to hold the lock.

These are **stress and forced-interleaving tests, not a proof**. They ran in Debug mode on one
machine. They can miss rare interleavings, and none was run under a race detector or on a weakly
ordered CPU.

## 6. Known limits

- A single global lock serializes everything. A long `revoke` or `closeSpace` blocks all other
  operations. Finer-grained locking is future work and would need a new lock-order document.
- The model does not cover interrupt context. A kernel lock taken in an interrupt handler would need a
  different primitive.
- Destructors are not given any ordering relative to other threads' operations except "after the
  object's last reference and pin are gone".
- If a destructor never returns, its object and charge stay occupied forever (no timeout).
- `Kernel.destroy` is single-threaded by contract.
