# OMNI-Kernel ABI Contracts and Security Model

**Document status:** Batch 1 specification. All interfaces here are **proposed contracts**. No header, library, kernel, or service implements them. Nothing in this document has been compiled, executed, tested, or verified. Pseudocode and structures are **conceptual**, written in a Zig-flavoured notation for readability only; they are not valid, compiled Zig and do not define final binary layouts.

ABI version label for this document: **OMNI-ABI 0.1 (draft)**. Terminology is shared with `ARCHITECTURE.md` in this directory.

---

## Part A — Common ABI conventions

### A.1 Versioning and compatibility rules

- The whole ABI carries a version `OMNI-ABI <major>.<minor>`; each interface also carries its own `<major>.<minor>`. For Batch 1 every interface is **0.1 (draft)**: `omni_fs 0.1`, `omni_net 0.1`, `omni_process 0.1`, `omni_ipc 0.1`, `omni_clock 0.1`, `omni_random 0.1`, `omni_gui 0.1`.
- **While the major version is 0, no compatibility is promised.** Any change may break callers. Implementers must not treat 0.x as stable.
- **From 1.0 onward (planned):** a minor increment may only *add* operations, error codes, optional fields at the end of structures, and flag bits. A major increment may break compatibility. Removal or semantic change of an existing operation requires a major increment.
- **Version negotiation.** Opening an interface (see A.5) carries a requested `[min_version, max_version]` range; the service replies with the selected version or `BAD_VERSION`. A client must not assume a version it did not negotiate.
- **Extensible structures.** Every request and response structure begins with a header (A.3) containing its size and a flags word. Unknown flags must be rejected with `INVALID_ARGUMENT`. Trailing bytes beyond the known size must be zero or rejected; the rule is chosen per interface minor version and stated in that version.
- **Fixed layout.** Final ABI types will use explicit sizes, little-endian byte order for multi-byte fields on all initial targets, explicit padding, and natural alignment. Final layout is not defined in Batch 1.

### A.2 Common types (conceptual)

```text
// CONCEPTUAL PSEUDOCODE — not compiled, not final.
Handle      : u32   // index+generation into the caller's kernel-held table; 0 is always invalid
UserPtr     : u64   // address in the caller's address space; never dereferenced without validation
Len         : u64
Status      : i32   // 0 = OK, negative = error code (A.4)
Nanos       : u64   // nanoseconds
```

### A.3 Common message header (conceptual)

```text
// CONCEPTUAL PSEUDOCODE
struct MsgHeader {
    size:      u32,  // total size of this structure in bytes, including header
    abi_ver:   u16,  // negotiated interface version (major<<8 | minor)
    op:        u16,  // operation number within the interface
    flags:     u32,  // operation-specific flags; unknown bits rejected
    request_id:u64,  // chosen by caller; echoed in the reply; not trusted for authorization
}
struct ReplyHeader {
    size:      u32,
    status:    Status,
    request_id:u64,
}
```

### A.4 Error codes

The following codes are used across interfaces. Code names are written without any prefix in prose; a numeric assignment is not made in Batch 1.

| Code | Meaning |
|---|---|
| `OK` | Success |
| `INVALID_ARGUMENT` | Malformed value, unknown flag, length out of range |
| `INVALID_HANDLE` | Handle not present, stale generation, or wrong object type |
| `ACCESS_DENIED` | Handle valid but lacks the required right |
| `NOT_FOUND` / `ALREADY_EXISTS` | Name or object lookup results |
| `QUOTA_EXCEEDED` | Caller's resource account would exceed a limit |
| `WOULD_BLOCK` | Non-blocking call could not proceed |
| `TIMED_OUT` | Deadline elapsed |
| `CANCELLED` | Operation cancelled (A.7) |
| `PEER_DEAD` | The other endpoint or serving component terminated |
| `NOT_SUPPORTED` | Operation or flag not implemented by this service |
| `BAD_VERSION` | No mutually supported version |
| `OVERFLOW` | Arithmetic or size overflow detected in arguments |
| `BUSY` | Resource in use and the call cannot wait |
| `IO_ERROR` | Underlying device or service failure |
| `INTERNAL` | Unexpected internal failure; logged |

**Error-handling rules.** (1) Errors never partially apply unless an operation documents partial completion (for example short reads). (2) Error replies must not leak information the caller was not entitled to; in particular `ACCESS_DENIED` versus `NOT_FOUND` must not reveal existence of objects the caller cannot access, where that matters (see per-interface notes). (3) A failed call must not leak handles or leave resources charged.

### A.5 Opening a service interface

A client does not name a global service. It receives a **service capability** (a handle to an IPC endpoint) at creation time or over IPC from a naming/supervisor service. Conceptual operation, common to all service-backed interfaces:

```text
// CONCEPTUAL PSEUDOCODE
op OPEN_INTERFACE(request: { min_version, max_version, flags })
   -> reply: { selected_version, session_handle }
```

A `session` is service-side state for one client, charged to the client's resource account where the service supports it. Closing the session handle releases it.

### A.6 Capability rights model (summary)

Each handle carries a rights bitmask. Common rights: `READ`, `WRITE`, `EXECUTE`, `DUPLICATE` (derive another handle with equal or fewer rights), `TRANSFER` (send over IPC), `WAIT`, `SIGNAL`, `ADMIN` (interface-specific management), `INSPECT`. Interface sections list the specific rights they require. Rights can only be reduced by derivation. Part B defines the model fully.

### A.7 Blocking and cancellation conventions

- Every potentially blocking operation accepts a `deadline` (absolute monotonic time, or "none") and optionally a `cancel_handle` referencing a cancellation object.
- **Desired semantics:** signalling the cancellation object *requests* cancellation. The blocked operation completes exactly once, with either `CANCELLED` (no effect applied) or its true result if it had already committed. The caller must treat both outcomes as possible. Resources associated with the operation are released exactly once.
- **Not claimed:** synchronous cancellation (cancel returns only after the operation has stopped) is **not** guaranteed and not implemented. Operations committed to hardware or to a remote peer may not be abortable. The design challenges are in B.8.
- A non-blocking mode (`WOULD_BLOCK`) is available for operations where it makes sense.

---

## Part B — Security model

### B.1 Default-deny

A process begins with only the handles placed in its table at creation. No ambient authority, global namespace, or privileged user identity exists. Any operation lacking a presented handle with the necessary right fails with `INVALID_HANDLE` or `ACCESS_DENIED`. Services follow the same principle toward their clients: a service grants nothing a client did not ask for and was not entitled to.

### B.2 Capabilities and handles

- **Kernel-maintained authority.** The authoritative record of a capability (object reference, rights, generation, derivation link) lives in kernel memory. User space holds only a `Handle`, an index plus generation into a per-process table. A handle value alone conveys nothing outside its table.
- **Unforgeability.** A process cannot manufacture a handle that the kernel will accept, because every use is checked against the process's own table: index in range, slot occupied, generation matches, object type correct, and rights sufficient. Handle values are not secrets and are not relied upon as such; security comes from table custody, not from unpredictability. (Mixing in randomness against stale-handle reuse is an optional hardening, not a security boundary.)
- **Scope.** A capability designates a specific object (a file session, a memory object, a channel endpoint) and a rights set. Service-side capabilities (such as a file) designate an endpoint plus a service-assigned object identifier that the service validates itself.
- **Delegation and transfer.** A holder with `DUPLICATE` may derive a handle with a subset of rights. A holder with `TRANSFER` may send a handle over IPC, either *moving* it (sender loses it) or *copying* it if it also has `DUPLICATE`. The receiver gets a new entry in its own table.
- **Lifetime.** A capability exists until all handles referring to it are released, the object is destroyed, or (if supported) it is revoked. Process termination releases all its handles.
- **Revocation (not implemented).** Revoking authority that was already delegated is the hardest part of the model.

| Option | Mechanism | Advantages | Drawbacks |
|---|---|---|---|
| R1 Indirection | All handles go through a revocable intermediate object; revoking it invalidates every handle derived through it | Simple, constant-time check | Must be chosen at delegation time; extra indirection |
| R2 Derivation tree | Kernel tracks parent/child; revoke deletes the subtree | Revoke anything derived, precise | Memory and traversal cost; deep trees; races with transfer |
| R3 Epochs/generations | Object has a generation; revoking bumps it, invalidating older capabilities | Cheap, bulk revoke | Coarse, invalidates unrelated holders of the same generation |

**Provisional recommendation:** R2 for kernel-resident objects with a bounded derivation depth, plus R1 for service-defined authority (services can revoke their own sessions). The recommendation is provisional because it needs prototyping to measure cost and race behaviour. Revocation racing against in-flight operations that already passed their check is an **unresolved challenge** (see B.8).

### B.3 Validation of user-controlled input

Every syscall and IPC message argument is untrusted. Requirements:

1. **Pointers.** A `UserPtr` is checked to lie wholly within the caller's user address range and to be mapped with suitable permission for the access. Validation is on `[ptr, ptr+len)` using overflow-checked arithmetic.
2. **Lengths and counts.** Checked against per-operation maximums *before* allocation or copying; multiplication and addition overflow-checked (`OVERFLOW`).
3. **Identifiers.** Object IDs, indices, and offsets are range-checked; identifiers supplied by clients to services are validated by the service against its own state, never trusted as indices.
4. **Handles.** Checked per B.2 on every use. Handle type confusion (passing a channel where a memory object is expected) yields `INVALID_HANDLE`.
5. **Message contents.** Structure validated by the kernel (size, handle count); semantics by the receiving service. Strings and paths have explicit length and encoding rules and are never assumed NUL-terminated.
6. **Time-of-check/time-of-use (TOCTOU).** User memory can change concurrently (other threads, shared memory). Rule: **copy-in then validate the copy**, and operate on the copy. Never validate user memory in place and then re-read it. For shared memory objects, services must treat contents as volatile and read each value once into private storage before using it. Handle validity can also change between check and use; kernel operations that use a handle must hold a reference to the object for the operation's duration.
7. **Integer overflow.** All size computations use checked arithmetic; targets with 32-bit-sized fields widen before arithmetic. Zig's checked arithmetic helps, but safety-checked builds may be disabled in release builds, so explicit checks are required in security-critical code rather than relying on build-mode behaviour.
8. **Malformed input.** The correct response to malformed input is a defined error, never a panic, hang, or unbounded loop. Parsers (file formats, network protocols, Wasm modules) are fuzzing targets (B.10).

### B.4 Memory isolation

Processes are separated by distinct address spaces. Cross-process memory access occurs only through shared memory objects explicitly sent as handles with chosen rights; there is no capability for reading another process's arbitrary memory by default. Kernel memory is inaccessible to user mode. This is a **design goal**. Complete memory isolation is **not claimed and not implemented**: it depends on correct page-table code, hardware behaviour (including speculative-execution side channels, which are out of scope as a guarantee, see B.11), DMA protection (IOMMU), and the absence of kernel bugs.

### B.5 IPC validation and denial-of-service resistance

- Message size and handle count have hard maximums; queues are bounded and charged.
- A receiver that never reads cannot exhaust kernel memory: the sender is charged for queued bytes and receives `QUOTA_EXCEEDED` or `WOULD_BLOCK` when limits are reached.
- A sender cannot force a service to allocate unbounded state: services must cap sessions and per-session objects.
- Peer death produces `PEER_DEAD` on all waiters and releases queued messages and in-flight handles exactly once.
- Handle transfer in flight is accounted; a transferred handle sitting in an unread message is owned by the channel until received or the channel is destroyed.
- Deadlock and starvation: no kernel-level IPC operation may wait indefinitely without a cancellable, deadline-capable path.

### B.6 Resource limits and accounting

Each process has a resource account with limits on pages, handles, threads, channels, queued IPC bytes, and CPU share, with a parent account hierarchy so a service can limit a client subtree. Charging rules: objects are charged to their creator; shared objects keep their creator's charge unless charge transfer is explicitly performed. Limits are enforced at allocation and fail with `QUOTA_EXCEEDED`. Services may keep their own per-client limits in addition. Accounting correctness under concurrency, shared-object lifetimes, and reclaim after crash are implementation challenges not yet solved.

### B.7 Driver and service isolation

Drivers and services run as unprivileged processes with minimum capabilities. A driver gets only its device's MMIO, interrupt, and DMA capabilities. Without IOMMU protection, a DMA-capable device can bypass memory isolation; on such platforms the driver and device are effectively inside the TCB. This must be documented per platform rather than hidden. Services must not accept capabilities they did not request and must validate every client-supplied identifier.

### B.8 Cancellation and safe cleanup — implementation challenges

Cancellation interacts with: operations already committed to a device, blocked threads holding kernel references, handle revocation in flight, partially transferred handles, and services holding client state. Desired invariant: each blocked operation reaches a single completion, all acquired references are released exactly once, and no handle or memory is leaked or double-freed. Options for implementation include cooperative cancellation points with kernel-side wait queue removal, and operation objects with explicit state machines (*pending → committed → completed/cancelled*). **Provisional recommendation:** operation objects with explicit states, because the committed state makes the "already happened" case explicit. Proving the invariant is future work; it is not implemented or verified.

### B.9 Audit and logging

Security-relevant events (access denials, validation failures, quota exhaustion, capability delegation to unexpected parties, service restarts) are recorded as structured events, rate-limited and sampled to prevent log flooding. Logs do not contain secrets, raw key material, or full user data. Reading logs requires an `INSPECT`-class log capability. Audit completeness and tamper-resistance are goals and are not specified in detail.

### B.10 Security testing (planned)

Planned: unit tests for validators; property-based tests for capability table invariants; fuzzing of syscall arguments, IPC messages, filesystem and network parsers, and Wasm module loading; deterministic simulation of race conditions in hosted mode; VM-level tests for isolation; and external review. None of these exist yet. Hosted-mode tests cannot establish isolation properties, because a hosted process is not a privileged kernel.

### B.11 Randomness requirements and limitations

- **Requirement.** `omni_random` must provide bytes suitable for cryptographic use when its entropy pool has been seeded from adequate sources, and must not return cryptographic-class output before that point.
- **Provisional design.** A kernel pool seeded from hardware sources (CPU instructions such as RDRAND/RNDR where available, firmware-provided seeds, virtual-device entropy, jitter and device timings) mixed through a conventional cryptographic construction, with a CSPRNG for output and periodic reseeding. The specific construction is **not chosen** in Batch 1.
- **Limitations.** Early-boot entropy can be poor, especially in VMs and on embedded devices; hardware generators cannot be fully trusted; snapshot/restore of VMs can duplicate state; hosted mode just uses the host's generator. The interface therefore exposes a seeding-state indicator, and blocking until seeded is the default for cryptographic requests. **No claim is made that cryptographically secure randomness is implemented.**

### B.12 Threat model

**Assets.** Confidentiality and integrity of process memory and data; integrity of kernel state and the capability table; availability of the system and services; confidentiality of input and display content; integrity of persisted data.

**Adversaries considered.**

- A malicious or compromised unprivileged application attempting to forge authority, escalate privilege, read others' data, or exhaust resources.
- A malicious or buggy user-space service or driver attempting to harm its clients or the kernel.
- A hostile Wasm module or compatibility-layer guest attempting to escape its runtime.
- A remote network attacker sending malformed traffic to the network service.
- Malformed filesystem images or removable media.
- A malicious peer on IPC attempting denial of service.

**Assumptions.** The kernel's TCB is correct enough to be trustworthy (an assumption to be tested, not verified); the CPU's privilege and MMU hardware behave to specification; the build toolchain is not malicious; the boot chain delivers an unmodified kernel (secure boot is not specified here).

**Out of scope in Batch 1.** Physical attacks (cold boot, bus probing, evil-maid attacks); hardware and firmware backdoors; microarchitectural side channels (speculative execution, cache timing) as guaranteed defences; malicious hypervisors against virtualized guests; supply-chain compromise of the toolchain; formal verification; protection of data against root-equivalent holders of broad capabilities, who by design hold the authority they hold.

**Limitation statement.** This model identifies what the design aims to resist and what it does not. It makes **no universal security guarantee**; the design has not been reviewed, tested, or verified.

---

## Part C — Interface contracts

Each section follows the same template. All operations are proposed. "Blocking" lists operations that can wait.

### C.1 `omni_fs` — filesystem operations (0.1 draft)

**Purpose and boundary.** Access to named files and directories provided by a user-space filesystem service. The kernel is not involved beyond IPC and memory objects.

**Versioning.** Per A.1. Path encoding is UTF-8 without NUL; normalization policy is an **open question** (provisional: no normalization, byte-exact comparison, to avoid ambiguity attacks).

**Operations.**

| Operation | Semantics |
|---|---|
| `fs_open_root` | Obtain a directory handle for a granted root (via service capability) |
| `fs_open` | Open a name relative to a directory handle; returns a file handle with requested rights ⊆ directory rights |
| `fs_read` / `fs_write` | Positioned read/write against a file handle; buffers by shared memory object or inline payload |
| `fs_stat` | Metadata for a handle |
| `fs_readdir` | Enumerate directory entries with a cookie for continuation |
| `fs_create` / `fs_unlink` / `fs_rename` | Namespace mutation relative to directory handles |
| `fs_sync` | Request durability for a handle |
| `fs_close` | Release handle |

There is no operation that opens an absolute path from nothing; all paths are relative to a held directory handle (capability-style).

**Request/response (conceptual).**

```text
// CONCEPTUAL PSEUDOCODE
FsOpenReq  { hdr, dir: Handle, name_len: u32, name: [name_len]u8, want_rights: u32, flags: u32 }
FsOpenRep  { rhdr, file: Handle (transferred) , granted_rights: u32 }
FsReadReq  { hdr, file: Handle, offset: u64, length: u64, buf: Handle(memobj) | inline }
FsReadRep  { rhdr, bytes_read: u64 }   // short reads permitted
```

**Errors.** `NOT_FOUND`, `ALREADY_EXISTS`, `ACCESS_DENIED`, `IO_ERROR`, `QUOTA_EXCEEDED` (disk or handle quota), `INVALID_ARGUMENT` (bad name), plus A.4 generals. Short reads and writes are partial completions and are documented as such.

**Capabilities and least privilege.** Rights: `READ`, `WRITE`, `EXECUTE` (open for execution), `ADMIN` (namespace mutation), `DUPLICATE`, `TRANSFER`. A file handle's rights never exceed its directory's. Path traversal (`..`) must not escape the granted directory capability; the service resolves names against the held directory and rejects escapes, including through symbolic links that point outside.

**Ownership and lifetime.** File handles are session-owned and released on close or session death. Open files after unlink remain valid until closed (provisional). The service is responsible for freeing on client death.

**Blocking and cancellation.** `fs_read`, `fs_write`, `fs_sync`, `fs_readdir` may block on I/O and accept deadline and cancel handle. A cancelled write may have partially completed; the reply reports the true result if committed.

**Validation.** Name length caps, component count caps, offset+length overflow checks, buffer handle rights and bounds, rename cycle checks, read-modify-write races on shared buffers handled by copying.

**DoS and leakage risks.** Huge directories (cookie-based paging with caps), deeply nested paths, many open handles (quota), slow-media starvation, symlink loops (depth cap). Leakage: distinguishing `NOT_FOUND` from `ACCESS_DENIED` can reveal existence; services should return `NOT_FOUND` for entries the caller cannot see. Timestamps and sizes can reveal activity; metadata exposure is capability-gated.

### C.2 `omni_net` — networking (0.1 draft)

**Purpose and boundary.** Sockets and network interface access provided by a user-space network service on top of a network driver.

**Versioning.** Per A.1. Protocol families (IPv4, IPv6, datagram, stream) are feature flags negotiated at open.

**Operations.**

| Operation | Semantics |
|---|---|
| `net_socket` | Create a socket object of a family/type, constrained by the session's allowed set |
| `net_bind` / `net_listen` / `net_accept` | Local endpoint and listening |
| `net_connect` | Outbound connection to an address permitted by the capability |
| `net_send` / `net_recv` | Data transfer (stream or datagram) |
| `net_shutdown` / `net_close` | Orderly and final close |
| `net_getopt` / `net_setopt` | Restricted option set |
| `net_poll` | Wait for readiness on multiple sockets |
| `net_raw_open` | Raw frame access; requires a distinct restricted capability |

**Request/response (conceptual).**

```text
// CONCEPTUAL PSEUDOCODE
NetConnectReq { hdr, sock: Handle, addr_family: u16, addr: [..]u8, flags: u32 }
NetRecvReq    { hdr, sock: Handle, buf: Handle(memobj), buf_len: u64, flags: u32 }
NetRecvRep    { rhdr, bytes: u64, peer_addr_len: u32, peer_addr: [..]u8 }
```

**Errors.** `ACCESS_DENIED` (address or port not permitted), `NOT_FOUND` (no route), `TIMED_OUT`, `PEER_DEAD` (service death) distinct from connection reset (reported as `IO_ERROR` with a reason code, proposed), `BUSY` (port in use), `QUOTA_EXCEEDED`.

**Capabilities and least privilege.** The session capability carries a **policy**: permitted address ranges, port ranges, directions (inbound/outbound), protocols. Binding privileged ports or raw access need separate rights. Default session policy: deny all.

**Ownership and lifetime.** Sockets are session-owned; closed on session death. Kernel memory objects used for buffers are charged to the client.

**Blocking and cancellation.** `net_accept`, `net_connect`, `net_recv`, `net_send`, `net_poll` may block; all accept deadline and cancel handle. Cancelling `net_send` after bytes were handed to the stack reports the partial count; bytes already transmitted cannot be recalled.

**Validation.** Address length and family consistency, port range, buffer bounds, flag whitelist, option whitelist; protocol parsers in the service are untrusted-input code.

**DoS and leakage risks.** SYN-flood-style resource exhaustion (backlog caps, per-session socket quotas), amplification (rate limits on datagram sends), buffer bloat (bounded queues), port-scan oracles via distinguishable errors, traffic metadata leakage (timing, sizes), ephemeral-port or sequence-number predictability (depends on `omni_random` quality), cross-session information via shared caches.

### C.3 `omni_process` — process and thread management (0.1 draft)

**Purpose and boundary.** Creation, supervision, and control of processes and threads. Kernel mechanisms (thread and address-space objects) are exposed through handles; policy (loading executables, supervision) is a user-space service.

**Versioning.** Per A.1. Process creation parameters grow by optional trailing fields in minor versions.

**Operations.**

| Operation | Semantics |
|---|---|
| `proc_create` | Create an empty process with a new address space; returns a process handle |
| `proc_map` | Map a memory object into a target process's address space (requires rights on both) |
| `proc_grant` | Place a specific handle into the new process's table before start |
| `proc_start` | Start the first thread at an entry address |
| `thread_create` | Create a thread within a process with stack and entry parameters |
| `thread_exit` / `thread_kill` | Terminate current or target thread |
| `proc_kill` | Terminate a process; releases its handles |
| `proc_wait` | Wait for process or thread termination; returns exit status |
| `thread_set_priority` | Set priority up to the granted ceiling |
| `proc_set_limits` | Set resource-account limits on a child (cannot exceed own) |

**Request/response (conceptual).**

```text
// CONCEPTUAL PSEUDOCODE
ProcCreateReq { hdr, account: Handle, flags: u32 }
ProcCreateRep { rhdr, proc: Handle }
ThreadCreateReq { hdr, proc: Handle, entry: UserPtr, stack: UserPtr, stack_len: u64, arg: u64, priority: u8 }
ProcWaitRep   { rhdr, exit_status: i64, exit_reason: u32 }
```

**Errors.** `INVALID_HANDLE`, `ACCESS_DENIED`, `QUOTA_EXCEEDED` (threads/processes), `INVALID_ARGUMENT` (entry or stack outside the target address space), `BUSY` (process already started), `PEER_DEAD` for waits on already-reaped objects as defined by the lifetime rule.

**Capabilities and least privilege.** A process handle conveys control over that process; `proc_map` into a process needs `WRITE`-class rights on the process handle and `READ`/`WRITE`/`EXECUTE` on the memory object matching the mapping. A parent can grant only handles it holds, with rights ⊆ its own and requiring `TRANSFER`. A child's account limits are bounded by the parent's remaining budget. `thread_kill` of a thread in another process requires a rights bit specific to that.

**Ownership and lifetime.** The process object lives while any handle refers to it or while it is running. Termination releases the handle table, threads, mappings, channels (generating `PEER_DEAD` on peers), and charges. Exit status is retained until waited on or the last handle is released (bounded retention).

**Blocking and cancellation.** `proc_wait` blocks with deadline and cancel handle. Killing a process cancels all its threads' blocked operations according to A.7; cleanup completes asynchronously and the process transitions through *terminating* before *terminated*. **Synchronous kill-completion is not guaranteed**; `proc_wait` returns when the kernel considers cleanup complete.

**Validation.** Entry and stack ranges inside the target address space, alignment, overflow-checked sizes, priority within ceiling, flags whitelist, circular parent/child account hierarchies rejected.

**DoS and leakage risks.** Fork-bomb-style process/thread creation (account limits), zombie accumulation (bounded retention), starvation through priority abuse (ceilings), information leakage via process IDs or exit codes (handle-based identity instead of global PIDs; no global process list without an `INSPECT` capability).

### C.4 `omni_ipc` — inter-process communication and handle transfer (0.1 draft)

**Purpose and boundary.** Kernel-mediated channels, notification objects, and shared memory objects. Only this interface and the clock/random primitives are expected to be directly kernel-backed.

**Versioning.** Per A.1. Maximum message size and maximum handles per message are exposed as queryable limits, not hard-coded in the ABI.

**Operations.**

| Operation | Semantics |
|---|---|
| `ipc_channel_create` | Create a channel; returns two endpoint handles |
| `ipc_send` | Asynchronous send of payload plus up to N handles |
| `ipc_recv` | Receive next message; blocks unless non-blocking |
| `ipc_call` | Send and wait for the matching reply |
| `ipc_reply` | Reply to a call |
| `ipc_notify_create` / `ipc_notify_signal` / `ipc_notify_wait` | Notification objects (bitmask semaphores) |
| `ipc_memobj_create` | Create a shared memory object of a given size |
| `ipc_memobj_map` / `ipc_memobj_unmap` | Map/unmap into the caller's address space |
| `ipc_handle_derive` | Derive a handle with a subset of rights |
| `ipc_handle_close` | Release a handle |
| `ipc_cancel_create` / `ipc_cancel_signal` | Cancellation objects (A.7) |

**Request/response (conceptual).**

```text
// CONCEPTUAL PSEUDOCODE
IpcSendReq { hdr, ch: Handle, data: UserPtr, data_len: u32, handles: UserPtr, handle_count: u32 }
IpcRecvRep { rhdr, data_len: u32, handle_count: u32, sender_token: u64 }  // data copied to caller buffer
```

**Errors.** `PEER_DEAD`, `WOULD_BLOCK`, `QUOTA_EXCEEDED`, `ACCESS_DENIED` (missing `TRANSFER` on an enclosed handle), `INVALID_ARGUMENT` (payload or handle count above limit, duplicate handles in one message when disallowed), `CANCELLED`, `TIMED_OUT`.

**Capabilities and least privilege.** Rights on channel endpoints: `READ` (receive), `WRITE` (send), `DUPLICATE`, `TRANSFER`. Sending a handle requires `TRANSFER`; copying (rather than moving) requires `DUPLICATE`. Memory object rights bound the permissions of any mapping.

**Ownership and lifetime.** The channel exists until both endpoints are closed or both holders die. An enclosed handle in transit belongs to the channel until received; if the channel is destroyed first, it is released. Memory objects live until the last handle and last mapping are gone; unmapping does not free while handles remain.

**Blocking and cancellation.** `ipc_recv`, `ipc_call`, `ipc_notify_wait` block; each accepts deadline and cancel handle. `ipc_call` has the hardest cancellation case: after the request was delivered, cancellation can complete the caller with `CANCELLED` but the callee may still process it. The protocol must be idempotent or carry request IDs; the kernel does not guarantee the callee never sees a cancelled request.

**Validation.** Payload/handle counts, `UserPtr` ranges, handle rights per B.3, no self-send of an endpoint into its own queue when that would form an unreclaimable cycle (cycle handling is an **open question**: provisional approach is to charge in-flight handles so cycles cannot exceed the quota).

**DoS and leakage risks.** Queue flooding (bounded queues, sender charge), handle-count exhaustion (account handles), wait-for-ever (mandatory deadline capability), covert channels through queue depth or timing (**not defended against** in 0.1), leaking address-space layout via mapped addresses (mapping addresses chosen by the kernel and not shared across processes), `sender_token` that must not reveal global identity (provisional: a per-channel opaque value).

### C.5 `omni_clock` — monotonic and wall-clock time (0.1 draft)

**Purpose and boundary.** Time sources and timers. The monotonic clock is kernel-backed; wall-clock time and adjustments are provided by a user-space time service that consumes platform sources.

**Versioning.** Per A.1. Resolution is queryable and not fixed by the ABI.

**Operations.**

| Operation | Semantics |
|---|---|
| `clock_get_monotonic` | Nanoseconds since an unspecified origin; never decreases within a boot |
| `clock_get_wall` | Wall-clock time (nanoseconds since a defined epoch); may jump; **not** guaranteed monotonic |
| `clock_get_resolution` | Reported resolution of a clock |
| `clock_set_wall` | Set wall-clock time; privileged |
| `clock_sleep_until` | Block until an absolute monotonic deadline |
| `timer_create` / `timer_arm` / `timer_cancel` | Timer objects that signal a notification object |

**Request/response (conceptual).**

```text
// CONCEPTUAL PSEUDOCODE
ClockGetRep   { rhdr, nanos: Nanos, uncertainty_ns: u64 }   // uncertainty may be unknown (= max)
TimerArmReq   { hdr, timer: Handle, deadline: Nanos, period: Nanos, notify: Handle, bits: u64 }
```

**Errors.** `ACCESS_DENIED` (set without right), `INVALID_ARGUMENT` (deadline in an unrepresentable range), `OVERFLOW`, `QUOTA_EXCEEDED` (timers), `NOT_SUPPORTED` (clock not present).

**Capabilities and least privilege.** Reading the monotonic clock may be allowed by default for processes that were granted a clock handle; **it is not ambient**. `clock_set_wall` requires an `ADMIN` right. Timer objects need `WAIT`/`SIGNAL` on their notification target. Reduced-precision clock handles may be issued (rights cannot express precision directly; a *derived clock capability with coarsened resolution* is proposed).

**Ownership and lifetime.** Timers are owned by their creator, charged to its account, destroyed on release or process death; armed timers are cancelled on destruction.

**Blocking and cancellation.** `clock_sleep_until` blocks and accepts a cancel handle; it completes with `CANCELLED` or `OK`. Timer cancellation is asynchronous: a timer that has already fired may have signalled the notification, so callers must tolerate one late signal.

**Validation.** Deadline range, period lower bound (minimum period, to avoid interrupt storms), arithmetic overflow when adding durations to times, wall-clock steps bounded by policy if the service chooses.

**DoS and leakage risks.** Timer floods (account limit, minimum period), interrupt storms, high-resolution timing enabling side-channel attacks (coarsened clock capabilities), wall-clock changes affecting certificate validity and logs (the service logs changes), wall time leaking the machine's time zone or boot patterns (wall clock stored as UTC; local offset is an application concern).

### C.6 `omni_random` — random bytes and cryptographic randomness (0.1 draft)

**Purpose and boundary.** Provide random bytes from the kernel entropy pool and CSPRNG; distinguish cryptographic-class output from non-cryptographic.

**Versioning.** Per A.1. Quality classes are fixed names in 0.1; new classes require a minor increment.

**Operations.**

| Operation | Semantics |
|---|---|
| `random_get_bytes` | Fill a buffer with random bytes of a requested class |
| `random_get_status` | Report seeding state and available quality classes |
| `random_add_entropy` | Contribute entropy (privileged; contribution never reduces pool quality) |

Quality classes: `CRYPTO` (suitable for key generation once seeded) and `FAST` (non-cryptographic; for hashing salts, simulation, never for secrets).

**Request/response (conceptual).**

```text
// CONCEPTUAL PSEUDOCODE
RandomGetReq { hdr, class: u32, buf: UserPtr, len: u32 }   // len capped per call
RandomStatusRep { rhdr, seeded: u32, reseed_count: u64 }
```

**Errors.** `WOULD_BLOCK` (not yet seeded, non-blocking `CRYPTO` request), `TIMED_OUT`, `INVALID_ARGUMENT` (len cap, bad class), `ACCESS_DENIED`, `NOT_SUPPORTED` (class unavailable).

**Capabilities and least privilege.** A `random` capability with `READ` for `get_bytes`; `ADMIN` for `add_entropy`. Whether unprivileged processes receive `CRYPTO` capabilities by default is a policy decision for init (provisional: yes, since denying it only drives applications toward weaker sources).

**Ownership and lifetime.** No per-request state. Output buffers belong to the caller. The kernel must not retain returned bytes.

**Blocking and cancellation.** A `CRYPTO` request before seeding blocks by default; it accepts deadline and cancel handle. After seeding it must not block. **Cancellation is trivially safe because no state is committed.**

**Validation.** Length cap per call; buffer range and writability; class value whitelist. Output written only on success of the full request, or with an explicit partial-length reply, never uninitialized memory.

**DoS and leakage risks.** Entropy-exhaustion myths (after seeding, a CSPRNG output is not depleted; the design does not block after seeding), starving other users (per-process rate limits), state-compromise recovery (reseeding policy), predictability from poor early seeding (status indicator; warn consumers), VM snapshot duplication (reseed on resume), leaking pool state through `random_get_status` (only coarse values), timing side channels in the generator.

**Honest limitations.** See B.11. No cryptographic-quality guarantee is claimed or implemented.

### C.7 `omni_gui` — graphical surfaces and input access (0.1 draft)

**Purpose and boundary.** Client access to a compositor service for surfaces, display information, and input, with isolation between clients.

**Versioning.** Per A.1. Pixel formats and input event types are enumerations extended by minor versions.

**Operations.**

| Operation | Semantics |
|---|---|
| `gui_connect` | Open a session with the compositor |
| `gui_surface_create` / `gui_surface_destroy` | Create/destroy a surface backed by a memory object |
| `gui_surface_attach` | Attach a new buffer (shared memory object) with damage region |
| `gui_surface_commit` | Present pending state atomically |
| `gui_set_title` / `gui_set_geometry` | Window attributes (requests; compositor decides) |
| `gui_input_subscribe` | Receive input events directed to the client's focused surface |
| `gui_clipboard_*` | Clipboard access through an explicit, gated capability |
| `gui_capture_request` | Request screen or other-surface capture; requires a distinct restricted capability |

**Request/response (conceptual).**

```text
// CONCEPTUAL PSEUDOCODE
GuiSurfaceCreateReq { hdr, width: u32, height: u32, format: u32, flags: u32 }
GuiSurfaceCreateRep { rhdr, surface: Handle, buffer_memobj: Handle }
GuiInputEvent { kind: u16, device_id: u16, time: Nanos, payload: [..]u8 }
```

**Errors.** `ACCESS_DENIED` (capture or global input without right), `QUOTA_EXCEEDED` (surface memory), `INVALID_ARGUMENT` (dimensions or stride), `NOT_SUPPORTED`, `PEER_DEAD` (compositor restart).

**Capabilities and least privilege.** Default session: create surfaces, receive input *only while focused*, no global hotkeys, no capture of others, no input injection. Each of: global input observation, input synthesis, screen/other-surface capture, clipboard read of other clients' data, and display configuration is a separate capability that is not issued by default.

**Ownership and lifetime.** Surfaces and their buffers are charged to the client and released on session death. The compositor must not hold references that let a client's memory persist unaccounted after exit.

**Blocking and cancellation.** `gui_input_subscribe` event waits and presentation waits (frame callbacks) block, with deadline and cancel handle. `gui_surface_commit` is non-blocking by default. Presentation timing is best-effort.

**Validation.** Dimension limits, stride and format consistency with the buffer size (overflow-checked), damage rectangles clipped to the surface, buffer reuse hazards (a client may write while the compositor reads, so the compositor copies or tolerates tearing but never reads out of bounds), event payload bounds.

**DoS and leakage risks.** Surface or buffer flooding (quotas), commit floods (rate limit), focus stealing (compositor policy), keystroke snooping and screen scraping (capability-gated, off by default), clipboard leakage, input timing side channels, overlay and spoofing attacks (trusted-UI indicators are an open design question), display-driver crashes (compositor restart behaviour).

---

## Part D — Cross-interface consistency notes

- All interfaces share: `MsgHeader`/`ReplyHeader` (A.3), the error codes (A.4), and handle semantics (B.2).
- Every blocking operation accepts a deadline and a cancel handle (A.7).
- Service-backed interfaces (`omni_fs`, `omni_net`, `omni_gui`, policy parts of `omni_process` and `omni_clock`) are reached over `omni_ipc` channels.
- `omni_ipc`, the monotonic part of `omni_clock`, and the pool part of `omni_random` are the kernel-backed pieces; this matches the placement in `ARCHITECTURE.md` section 5.
- All interfaces are 0.1 (draft) and make no stability promise.

## Part E — Open design decisions

| # | Decision | Provisional choice | Rationale |
|---|---|---|---|
| D1 | Revocation | R2 derivation tree for kernel objects + R1 for service-defined authority | Precision and service control; requires prototyping |
| D2 | Cancellation implementation | Operation objects with explicit states | Makes committed-state explicit |
| D3 | Path normalization | None; byte-exact | Avoids normalization attacks |
| D4 | `CRYPTO` randomness default for unprivileged processes | Granted | Avoids weaker ad hoc sources |
| D5 | Shared-object accounting | Charge creator; explicit charge transfer | Predictable and simple |
| D6 | IPC cycle handling | Charge in-flight handles | Bounds leaks without cycle detection |
| D7 | Side-channel mitigation | Out of scope for 0.1 | Needs separate study |
