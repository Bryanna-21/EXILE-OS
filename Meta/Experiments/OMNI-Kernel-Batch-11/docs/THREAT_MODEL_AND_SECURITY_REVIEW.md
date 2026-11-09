# Kernel threat model and security review (Batch 11)

A review by the author of the batch, **not an independent audit**. Hosted tests are not proof of hardware isolation. `Kernel/` was not built or
run; its rows come from reading source.

## Threat model

- **Assets**: kernel memory and control flow; other spaces' memory; capabilities held by other spaces; scheduler integrity.
- **Attacker**: unprivileged user code in one space that controls every syscall argument and its own memory, can race its own threads, can exhaust
  quotas, and can exit or be killed at any point. Out of scope: DMA, devices, firmware, speculative side channels, physical access, a malicious
  kernel developer.
- **Trust boundary**: the syscall entry. Everything crossing it is copied in, decoded field by field, bounded, and validated before use.

## Controls and negative tests (hosted)

| Threat | Control | Test |
|---|---|---|
| Forged or guessed handle | Handles carry generation and index; slot lookup validates both | `cap_01`, `cap_03`, `ipc_07` (fuzzed random handles) |
| Use of a handle from another space | Tables are per space | `cap_02` |
| Stale handle after release or close | Generation bump; slot retirement on wrap | `cap_03`, `cap_04`, `ipc_04` |
| Type confusion | Type checked before rights | `cap_01` |
| Rights escalation by copy | Requested rights must be a subset of the held rights | `cap_07` |
| Attenuation bypass by move | A move must carry the exact rights | `cap_10` |
| Capability smuggled into its own queue | Rejected (`obj == dst`) | `cap_07` |
| Duplicate handle in one message | Rejected | `cap_07` |
| Quota exhaustion (handles, objects, queue, waiters) | Fixed bounds with defined errors | `cap_05`, `cap_06`, `ipc_04` |
| Reference cycles leaking objects | Reachability GC; dead capabilities purged | `cap_08`, `cap_10` |
| Resource leak on space exit or kill | `cleanup_space`, `terminate_space`; waiters scrubbed | `cap_09`, `ipc_05` |
| Malformed message or arguments | Fixed header validation order; nonzero reserved fields rejected | `ipc_03`, `ipc_07` (400 fuzz cases, status always in the defined set) |
| Bad user pointers | Probe before consuming; copy through the Batch 7/9 checked path | `ipc_03`, `ipc_06` |
| Message consumed but undeliverable | Output buffers probed first; on late failure installed handles are released | `ipc_06` |
| Kernel stack overflow from large frames | Messages never copied by value; measured by test on 16 KiB stacks | `ipc_01`..`ipc_08` in debug builds |

Mutation checks run during development (a deliberately broken rule must make a test fail): escalation check removed, move-attenuation check removed,
cycle check removed, dead-capability purge disabled: each was detected by at least one test (`cap_07`, `cap_10`).

## Findings status (SEC ids from the Batch 9 review)

| Id | Was | Now |
|---|---|---|
| SEC-01 forged/stale capabilities | not applicable to Rust | **Mitigated in Rust (hosted)** |
| SEC-02 unauthorized access | not applicable | **Mitigated in Rust (hosted)** |
| SEC-03 incorrect transfer | model only | **Mitigated in Rust (hosted)** |
| SEC-09 IPC exhaustion | model only | **Mitigated in Rust (hosted)** |
| SEC-05 `SpinLock` interrupt masking | unresolved | Batch 10 `irqlock` (hosted; real `cli`/`sti` never executed) |
| SEC-12 hardware boundary | unresolved | **Unresolved**: no boot evidence |
| SEC-13 single syscall stack | unresolved | **Unresolved**; Batch 11 runs IPC on thread stacks in tests only |
| SEC-17 SMEP/SMAP/KPTI absent | unresolved for OMNI | `Kernel/` enables SMEP/SMAP/UMIP/NX (read from source); KPTI not found; **unresolved** |
| SEC-18 ABI version query | unresolved | Fixed in Batch 10 (`SYS_ABI_QUERY`), hosted |
| SEC-19 pinned toolchain | unresolved | **Unresolved** (unavailable here) |
| SEC-20 register convention | owner decision | Adaptor in Batch 10; decision still the owner's |

## `Kernel/` observations (limited reading, nothing executed, no change made)

The author read the FIXME list of `Kernel/Syscalls` (36 entries) and a few files; this is not an audit of `Kernel/`. Items worth an owner decision:

1. `Kernel/Syscalls/thread.cpp`: thread creation has a FIXME for returning `EAGAIN` above a per-system thread limit. Without a limit a process can create threads until kernel memory is exhausted (denial of service). Not changed: the right limit and its interaction with userland is the owner's call.
2. `Kernel/Syscalls/fallocate.cpp`: overflow of offset plus length is already rejected (`EFBIG`); the FIXME concerns the filesystem maximum size only.
3. `Kernel/Syscalls/sigaction.cpp`: signal handling uses `InterruptDisabler` with a FIXME for a narrower lock (latency, and a poor fit for SMP).
4. KPTI was not found in the x86-64 entry code.

## New residual risks introduced by Batch 11

1. Unbounded priority inversion or lock contention is not analysed; only cooperative scheduling was tested.
2. The GC walks all objects under the IPC lock: its cost is bounded (32 objects) but not measured.
3. `CapTable::force_generation` is a public hidden test hook; it must not be reachable from any syscall (it is not).
4. Timeouts count scheduler ticks; the tick source on hardware is untested.
5. No audit logging of capability denials.

## Not reviewed

DMA, devices, firmware, timing channels, hardware errata, anything needing a booted system, `Kernel/` C++ memory safety.
