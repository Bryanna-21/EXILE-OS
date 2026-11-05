# OMNI-Kernel Core Interfaces (Batch 2)

Status: describes the code in `src/core/` and `src/arch/interface.zig` as written in Batch 2. These
are **hosted, architecture-neutral abstractions** for testing validation and bookkeeping rules.
They are not a kernel, not a finalized ABI, and not a security boundary. Terminology follows Batch 1
(`OMNI-Kernel-Batch-1`, **OMNI-ABI 0.1 (draft)**): *capability*, *handle*, *rights*, *resource
account*, *channel*, *memory object*.

Every decision marked **PROVISIONAL** is new in Batch 2 and has been tested only for internal
consistency, not validated against a running kernel.

## 1. Conventions common to all modules

* **Errors.** Fallible operations return `OmniError!T`. No operation panics on invalid input.
* **All-or-nothing mutation.** A mutating operation either completes or changes nothing. Where an
  operation touches several structures (for example a table and an account), it first performs every
  check that can fail, then commits steps that cannot fail.
* **Checked arithmetic.** Sizes, offsets, quotas and counters use `std.math.add/sub/mul` or explicit
  comparisons; overflow is `Overflow`, underflow of a counter is `InvalidState`.
* **No allocation.** No module allocates. Tables have a compile-time capacity.
* **Single-threaded.** No module is safe for concurrent use (see section 7).
* **Untrusted input.** Values that would arrive from user space (`u32` handles, rights bits, lengths,
  enum tags, message bytes) are modelled as raw integers or byte slices and validated before use.

## 2. `core/error.zig`

| Item | Description |
|---|---|
| `OmniError` | Error set, one entry per Batch 1 A.4 code plus `InvalidState` |
| `Result(T)` | Alias for `OmniError!T` |
| `Status` | Provisional wire value: `0` ok, negative error, non-exhaustive enum so unknown values are representable |
| `toStatus(e)` | Exhaustive mapping (compile error if an error lacks a mapping) |
| `fromStatus(s)` | `.ok` is success; known negative values map back; any unknown value is `InvalidArgument` and never success |

Mapping to the eventual ABI: syscalls would return `Status` as a signed 32-bit value (Batch 1
`Status : i32`, 0 = OK, negative = error). The numeric values in Batch 2 are **PROVISIONAL**; Batch 1
assigned none. `InvalidState` is new (Batch 1 had no code for "valid request, wrong object state").

Error selection rules used throughout:

| Situation | Error |
|---|---|
| Malformed value, unknown flag, unknown enum tag, unknown rights bit, length or alignment out of range | `InvalidArgument` |
| Handle zero, out of range, free/retired slot, stale generation, wrong object type | `InvalidHandle` |
| Live handle of the right type that lacks a required right (including derivation beyond the source's rights) | `AccessDenied` |
| Limit or fixed capacity reached | `QuotaExceeded` |
| Arithmetic overflow | `Overflow` |
| Counter would underflow (release of more than reserved) | `InvalidState` |
| Unsupported ABI version | `BadVersion` |
| Violated internal invariant, such as a misconfigured user window | `Internal` |

## 3. `core/ids.zig`

| Type | Representation | Purpose |
|---|---|---|
| `ObjectId` | `enum(u64)`, `invalid = 0`, non-exhaustive | Kernel-internal object identity. Not an authority |
| `ProcessId` | `enum(u64)`, `invalid = 0`, non-exhaustive | Kernel-internal diagnostic identity. Batch 1 C.3 exposes no global PIDs to user space; this type exists for kernel bookkeeping only |
| `Handle` | `enum(u32)`, `invalid = 0`, non-exhaustive | User-visible token: index (bits 0-15) and generation (bits 16-31) |
| `ObjectIdAllocator` | counter | Never returns `invalid`; reports `Overflow` without changing state |

Distinct enum types mean `ObjectId`, `ProcessId` and `Handle` cannot be passed for one another
without explicit `raw`/`fromRaw` conversions at a boundary (compile-time property; test ID-01).

**A handle never confers authority by itself.** It is only a lookup key into one process's
kernel-owned `CapabilityTable`. See section 4 for the checks the table applies. Handle values are not
secrets and are not relied on to be unguessable.

**PROVISIONAL (DEC-01) handle layout.** Batch 1 A.2 says "index+generation into the caller's
kernel-held table; 0 is always invalid" without a split. Batch 2: 16-bit index, 16-bit generation,
generation of a live handle is never 0, so a live handle is never 0. Capacity is therefore at most
65,536 slots per table.

## 4. `core/capabilities.zig`

### Types

* `Rights`: `packed struct(u32)` with the nine Batch 1 A.6 rights (`read`, `write`, `execute`,
  `duplicate`, `transfer`, `wait`, `signal`, `admin`, `inspect`) and 23 reserved bits that must be
  zero. `fromRaw` rejects any reserved bit; `isValid` guards values forged with `@bitCast`;
  `contains` is the subset test.
* `ObjectType`: `channel`, `memory_object`, `notification`, `cancellation`, `process`, `thread`,
  `timer`, `service_endpoint` (the object kinds named in Batch 1 C.3-C.7).
* `CapabilityTable(capacity)`: fixed-capacity table; each entry has `state` (`free`, `live`,
  `retired`), `generation`, `object`, `otype`, `rights`. Optionally charges a `resources.Account` one
  `handles` unit per live entry.

### Operations

| Operation | Behaviour |
|---|---|
| `insert(object, otype, rights)` | Creates an entry in the lowest free slot. Fails `InvalidArgument` (invalid object id or invalid rights), `QuotaExceeded` (table full or account limit). Atomic |
| `lookup(h, expected, required)` | Applies the check sequence below. Returns object, type and rights |
| `lookupRaw(h, expected, bits)` | As `lookup`, with untrusted rights bits parsed first (`InvalidArgument` on unknown bits) |
| `peek(h)` | Kernel-internal: resolves a handle without type or rights requirements. Used by IPC transfer validation; must never be exposed to user space |
| `derive(src, requested)` | New handle to the same object with `requested` rights. Needs `duplicate` on the source; `requested` must be a subset of the source's rights (`AccessDenied` otherwise). Atomic |
| `deriveRaw(src, bits)` | As `derive` with untrusted bits |
| `release(h)` | Invalidates `h`, frees the slot, returns the account charge. Second release is `InvalidHandle` |

### Check sequence for `lookup`

1. Required rights are valid (else `InvalidArgument`).
2. Handle is not 0, its index is below capacity, the slot is `live` and its generation equals the
   handle's generation (else `InvalidHandle`).
3. Object type equals the expected type (else `InvalidHandle`; PROVISIONAL DEC-10, matches Batch 1
   B.3 "handle type confusion yields INVALID_HANDLE").
4. The entry's rights contain every required right (else `AccessDenied`).

### Invariants (each is exercised by a test)

* Live handles are never 0 and are unique among live entries.
* A released handle is never valid again: release bumps the slot generation; when a generation would
  wrap past 65,535 the slot is **retired permanently** (PROVISIONAL DEC-02), so a stale handle cannot
  become valid again by wraparound. The cost is that a retired slot is lost for the life of the table.
* Rights of a derived handle are a subset of its source's rights at the time of derivation.
* `live_count` equals the number of `live` entries; with an account attached, account `handles` usage
  equals `live_count`.
* A failed operation leaves the table and the account unchanged.

### Ownership and lifetime

The table owns its entries. A `Handle` is a copyable value with no ownership. The table does **not**
own or destroy the referenced object: `ObjectId` is a reference only. Releasing the last handle does
not destroy anything in Batch 2 (object lifetime is deferred; see section 9).

### Not implemented

Revocation (including revoking handles derived from a released source; test CAP-08 documents that
they survive), transfer between tables, derivation parent links, reference counting, concurrency
control. Rights checks here do not provide hardware-enforced privilege or memory isolation.

## 5. `core/resources.zig`

* `Resource`: `pages`, `handles`, `threads`, `channels`, `queued_bytes` (the classes named in Batch 1
  B.6).
* `Counters`: one `u64` per class, used for limits and usage.
* `Account`: `limits`, `usage`, optional `parent`.

| Operation | Behaviour |
|---|---|
| `Account.create(limits, parent)` | With a parent, each child limit must not exceed the parent's remaining budget (limit minus usage) at creation (`QuotaExceeded`). PROVISIONAL DEC-09; Batch 1 C.3 says "bounded by the parent's remaining budget" |
| `reserve(r, n)` | Checks the account and every ancestor first (`Overflow` or `QuotaExceeded`), then charges all levels. A failed reserve changes nothing |
| `release(r, n)` | Checks every level for underflow first (`InvalidState`), then returns all levels. A failed release changes nothing |
| `used(r)`, `limit(r)` | Accessors |

Invariants: `usage <= limit` at every level after any successful `reserve`; counters never underflow
or overflow; a child's usage is included in every ancestor's usage when charged through the child.

Lifetime: a child stores a pointer to its parent. The parent must outlive the child and must not be
moved or copied while children exist; nothing in the module enforces this.

Charging policy (who is charged for a shared object, Batch 1 D5) is not in this module.

## 6. `core/memory.zig`

All helpers are arithmetic only and **never dereference** an address. Validation success does not
make a user pointer safe to access (see the module header and `SECURITY-REVIEW.md`).

| Function | Behaviour |
|---|---|
| `checkedRange(offset, len)` | `Overflow` if `offset + len` overflows `u64` |
| `validateWithin(total, offset, len)` | Range lies within `[0, total)`; `InvalidArgument` if not, `Overflow` on overflow |
| `byteLen(count, elem_size)` | Checked multiply |
| `isPowerOfTwo`, `checkAligned`, `alignUp` | Alignment must be a power of two (`InvalidArgument` otherwise); `alignUp` reports `Overflow` |
| `UserWindow{lo, hi}.validateSpan(addr, len)` | Span must lie in `[lo, hi)`; zero-length at `hi` is accepted; misconfigured window is `Internal` |
| `subslice(bytes, offset, len)` | Validated sub-slice of a slice the caller already owns; rejects values not representable as `usize` |

PROVISIONAL DEC-07: ranges are half-open; a zero-length range is valid iff `offset <= bound`
(including the exact end).

## 7. `core/ipc.zig`

Encoding, decoding and validation of message bytes, and validation of a handle-transfer list. No
queues, delivery, blocking or transport.

### Wire layout (PROVISIONAL DEC-05; little-endian)

| Offset | Size | Field | Rule |
|---|---|---|---|
| 0 | 4 | `size` | Equals the number of bytes supplied; at most `max_message_bytes` |
| 4 | 2 | `abi_ver` | Must equal `0x0001` (major 0, minor 1), else `BadVersion` |
| 6 | 2 | `op` | Non-zero |
| 8 | 4 | `flags` | Must be 0 (no flags defined in 0.1) |
| 12 | 2 | `kind` | `send=1`, `call=2`, `reply=3`; anything else is rejected |
| 14 | 2 | `reserved` | Must be 0 |
| 16 | 8 | `request_id` | Opaque; echoed in replies; never used for authorization (Batch 1 A.3) |
| 24 | 4 | `payload_len` | Must equal `size - 32` |
| 28 | 4 | `handle_count` | At most `max_handles`; handle values travel out of band (Batch 1 C.4) |
| 32 | n | payload | |

Relationship to Batch 1: A.3 sketched `size, abi_ver, op, flags, request_id` and noted explicit
padding; C.4 requires `send`/`call`/`reply` and a handle count. Batch 2 fills the padding with `kind`
and `reserved` and adds `payload_len` and `handle_count`. **Migration note:** Batch 1's header is
conceptual and implemented nowhere, so no code migrates; a later ABI draft should adopt or replace
this layout.

Batch 1 A.1 left open whether trailing bytes are zero-or-rejected; Batch 2 chooses **rejected**:
`size` must equal the supplied length exactly (PROVISIONAL DEC-05).

### Validation order

Fixed, so rejection is deterministic: length at least 32; version; flags, reserved and op; kind; size
against the input and the limit; payload length against size; handle count against the limit. The
decoded payload is a slice into the input; nothing is copied or allocated.

### Limits

`Limits{ max_message_bytes = 4096, max_handles = 8 }`. These defaults are **PROVISIONAL (DEC-06)** and
unvalidated; Batch 1 C.4 says limits are queryable rather than ABI constants, so they are parameters.

### Transfer-list validation

`validateTransferList(table, handles, declared_count)` requires the list length to equal the declared
count, every handle to be live with the `transfer` right, and no handle to appear twice (PROVISIONAL
DEC-08, from Batch 1 C.4 "duplicate handles in one message when disallowed"). It only reads the table.
It does not move or copy handles.

## 8. `arch/interface.zig`

A compile-time contract for a backend type: `name`, `page_size`, `user_va_lo`, `user_va_hi`,
`irqSaveDisable`, `irqRestore`, `fullBarrier`, `cpuRelax`, each with an exact type. `implements(T)`
returns a boolean (used by tests); `assertImplements(T)` is a compile error on failure;
`validateFacts` checks that the page size is a power of two and the user window is non-empty, ordered
and page-aligned; `userWindow(T)` combines them into a `memory.UserWindow`.

It contains no assembly and no privileged operation. The only implementer in Batch 2 is a mock in
`src/tests.zig`. Interrupt-controller, MMU, boot-entry and exception interfaces are deliberately not
defined yet; the arch READMEs list their requirements.

## 9. Provisional decision register

| ID | Decision | Section |
|---|---|---|
| DEC-01 | Handle = 16-bit index + 16-bit generation, generation of a live handle is never 0 | 3 |
| DEC-02 | A slot whose generation would wrap is retired permanently | 4 |
| DEC-03 | `InvalidState` error added | 2 |
| DEC-04 | Numeric `Status` values assigned (negative, as listed in `error.zig`) | 2 |
| DEC-05 | Concrete IPC wire layout; `kind` and `reserved` fill the header padding; trailing bytes rejected | 7 |
| DEC-06 | Default limits 4096 bytes and 8 handles | 7 |
| DEC-07 | Half-open ranges; zero-length valid iff `offset <= bound` | 6 |
| DEC-08 | Duplicate handles in one transfer list are rejected | 7 |
| DEC-09 | Child account limits bounded by the parent's remaining budget at creation | 5 |
| DEC-10 | Object-type mismatch reports `InvalidHandle`; missing right reports `AccessDenied` | 4 |
| DEC-11 | Fixed compile-time table capacity, no allocator | 4 |
| DEC-12 | Derivation requires `duplicate` on the source | 4 |

## 10. Concurrency assumptions

Nothing in Batch 2 is thread-safe. Tables, accounts and the id allocator use plain integers. A kernel
needs: per-table and per-account locks (or atomics), a defined lock order along the account parent
chain, and care so that handle lookup keeps the referenced object alive for the duration of the
operation (Batch 1 B.3 rule 6). These are deferred (plan milestone M3). Tests are single-threaded and
therefore say nothing about races.

## 11. Contradictions and gaps found in Batch 1, and how Batch 2 handles them

| Finding | Handling |
|---|---|
| A.2 gave no index/generation split | DEC-01 |
| A.4 had no code for invalid object state | DEC-03 |
| A.3 header has an unstated 4-byte gap and no message-kind field though C.4 uses send/call/reply | DEC-05 |
| A.1 trailing-byte rule undecided | Rejected (DEC-05) |
| B.6 hierarchy had no limit rule | DEC-09 |
| ARCHITECTURE.md section 10 mentions a "derivation parent" per capability; revocation is undecided | Batch 2 stores no parent link; deferred to plan milestone M2 |
| Zig version undecided (ARCHITECTURE.md Q5) | Zig 0.15.2 recorded in `README.md` and `REPORT.md`, with the compiler caveat in the plan |

No Batch 1 statement is contradicted by Batch 2; all of the above fill gaps and are labelled
provisional.
