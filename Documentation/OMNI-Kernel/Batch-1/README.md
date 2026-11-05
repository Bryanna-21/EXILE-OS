# OMNI-Kernel

**Status: Batch 1 — specification-only milestone.**
Nothing in this package is source code, a bootloader, or a build artifact. OMNI-Kernel does not boot, does not run on any architecture, enforces no security controls, and has no tests. This package contains design documents only.

## Contents of this package

| File | Purpose |
|---|---|
| `README.md` | Project identity, principles, roadmap, status table (this file) |
| `docs/ARCHITECTURE.md` | System layers, boundaries, trust model, design areas, targets and non-goals |
| `docs/ABI-AND-SECURITY.md` | Proposed ABI contracts for seven service interfaces and the security model |
| `REPORT.md` | Delivery and validation report: what was checked, what was not |
| `manifest.json` | SHA-256 checksums and byte sizes of the four files above |

`manifest.json` lists the other four files and deliberately excludes itself, since a file cannot contain a stable checksum of itself.

## 1. What OMNI-Kernel is

OMNI-Kernel is a proposed, Zig-first, capability-based microkernel and execution platform intended to become the native kernel foundation of Exile OS. "Universal" in this project means *one architecture with well-defined backends and compatibility layers*. It does **not** mean a universal binary format, and it does not promise seamless compatibility with every operating system or CPU.

The motivating goals are:

- A small privileged core whose mechanisms are few enough to review carefully.
- Authority expressed through unforgeable, kernel-maintained capability handles rather than ambient identity.
- Device drivers, filesystems, networking, and graphics as isolated user-space services, so that a failure in one does not compromise the kernel.
- A clean separation between kernel mechanisms, user-space services, execution environments, hardware backends, and compatibility layers.
- A development workflow in which most logic can be exercised in a hosted, deterministic environment before touching virtualized or bare-metal targets.

## 2. Relationship to Exile OS

Exile OS exists today and must keep working. OMNI-Kernel is therefore developed **alongside** it, never in place of it, under these principles:

1. **No destructive changes.** Existing Exile OS files, including `Kernel/` and `ExileKernel/`, are not modified by this project's documentation milestones.
2. **Incremental migration.** Replacement happens one subsystem at a time behind stable interfaces, with the existing implementation retained until a replacement is demonstrated, not merely specified.
3. **Reversibility.** Each integration step must have a documented way back to the previous behaviour.
4. **Honest status.** A component is not described as replacing an Exile OS component until it has been built, tested, and reviewed.

The specifics of how Exile OS components map onto OMNI-Kernel services are an **unresolved design question** (see `docs/ARCHITECTURE.md`, section 15). Batch 1 does not inspect or assume the internals of the existing Exile OS code.

## 3. Zig-first engineering principles

Zig is the intended primary implementation language for native kernel code. Assembly is permitted where the hardware requires it (early boot, context switch, trap entry, certain atomic and barrier sequences). The following are principles, not claims of current capability:

- **Zig is a tool, not a guarantee.** Zig does not by itself provide memory safety, portability, security, or binary compatibility. Those properties must come from design, review, and testing.
- **Explicit allocation.** Kernel code should take allocators explicitly and avoid hidden allocation, so resource accounting stays tractable.
- **Narrow unsafe surface.** Code that handles raw pointers, user memory, and hardware registers is confined to small, reviewed modules.
- **Fixed-layout ABI types.** Types crossing the user/kernel boundary use explicit sizes, alignment, and endianness rules, independent of compiler defaults.
- **Supported implementation boundary.** Zig and assembly for kernel and platform code; user-space services may use Zig or other languages that can target the ABI. Rust is not the primary kernel implementation language.
- **Toolchain pinning.** A specific Zig version will need to be pinned at the toolchain stage because the language and standard library are still evolving. Which version is **not yet decided**.

## 4. Target architecture strategy

| Architecture | Role | Status |
|---|---|---|
| x86-64 | Initial target | Planned |
| ARM64 (AArch64) | Initial target | Planned |
| RISC-V (RV64) | Considered in architecture; later target | Planned, lower priority |

Architecture-specific code is isolated behind a platform interface (see `docs/ARCHITECTURE.md`). No architecture is supported today.

## 5. Execution modes

OMNI-Kernel is intended to run under three distinct backends. They are **separate execution environments** with different privilege and trust assumptions. They are not interchangeable and a hosted process is not equivalent to a privileged kernel.

| Mode | Where it runs | Privilege | Hardware access | Primary use |
|---|---|---|---|---|
| Hosted | As an ordinary process on an existing OS | Unprivileged | Emulated through host services | Development, unit and deterministic testing |
| Virtualized | Inside a virtual machine | Privileged within the guest | Virtual devices | Integration testing, early boot validation |
| Bare-metal | Directly on hardware | Fully privileged | Real devices and firmware | Eventual production target |

Results obtained in one mode do not automatically transfer to another. Hosted testing cannot validate interrupt behaviour, page-table hardware semantics, or real-device timing. Each claim must be tested in the mode it concerns.

## 6. Microkernel and user-space service model

The kernel is intended to provide only mechanisms that must be privileged: address-space and page-table management, thread scheduling primitives, IPC, capability management, interrupt delivery, and minimal time and entropy primitives. Policy and most functionality live in user space: filesystems, networking, graphics, device drivers, naming, and process supervision.

Seven logical service interfaces are specified in Batch 1: `omni_fs`, `omni_net`, `omni_process`, `omni_ipc`, `omni_clock`, `omni_random`, and `omni_gui`. All are **proposed contracts**, not implemented APIs. The current ABI version label is **OMNI-ABI 0.1 (draft)**.

## 7. Role and limitations of WebAssembly

WebAssembly (Wasm) is an **optional** user-space execution environment for sandboxed application code. It runs in a user-space runtime service and reaches system functionality only through capabilities granted to it. Limitations stated up front:

- Wasm is not a substitute for native drivers; drivers need device and DMA access that a sandbox must not give.
- A Wasm runtime is a large attack surface and must itself be treated as untrusted-input-handling code.
- Wasm sandboxing does not remove the need for kernel-level isolation of the runtime process.
- Performance and system-interface coverage are constrained by the host interface chosen, which is not yet decided.

## 8. Security and privacy principles

- **Default deny.** No authority exists unless explicitly granted.
- **Least privilege.** Capabilities carry narrow rights and scopes.
- **Kernel-maintained authority.** Handles are indices into kernel-held state, never self-describing tokens trusted by themselves.
- **Validate everything crossing a boundary**: pointers, lengths, identifiers, handles, messages.
- **Isolation of failure.** Drivers and services run outside the kernel so faults are contained.
- **Bounded resources.** Every resource consumer is accounted and limited.
- **Auditability.** Security-relevant decisions are loggable without leaking secrets.
- **Privacy by design.** Services expose the minimum identifying information; entropy and clock interfaces are designed with leakage in mind.
- **No overclaiming.** The design is not formally verified and is not production-ready. Security guarantees are goals to be tested, not properties asserted.

Details and the threat model are in `docs/ABI-AND-SECURITY.md`.

## 9. Staged roadmap

Each stage lists its intended outcome. Stages are **planned**, ordering may change, and none is complete beyond stage 0.

| Stage | Area | Intended outcome |
|---|---|---|
| 0 | Architecture specification (this batch) | Reviewed architecture, ABI draft, security model |
| 1 | Toolchain and build foundation | Pinned Zig toolchain, reproducible build, hosted test harness skeleton |
| 2 | Boot path | Boot to a minimal kernel entry in a VM on x86-64, then ARM64 |
| 3 | Memory management | Physical allocator, page tables, address spaces |
| 4 | Scheduling | Threads, context switch, timer-driven preemption |
| 5 | IPC | Message passing, handle transfer |
| 6 | Capabilities | Handle table, rights, derivation, revocation design chosen and implemented |
| 7 | Drivers | User-space driver framework, interrupt delivery, first virtual devices |
| 8 | Filesystem | Storage service and `omni_fs` first implementation |
| 9 | Networking | Network service and `omni_net` first implementation |
| 10 | Compatibility | Isolated compatibility layers and optional Wasm runtime |
| 11 | Testing and hardening | Fuzzing, deterministic simulation, security review |
| 12 | Exile OS integration | Staged, reversible integration |

Bare-metal bring-up is expected to follow virtualized validation, not precede it.

## 10. Status table

### Specified in Batch 1 (documented; not implemented)

- Layered architecture and trust boundaries.
- Boot and initialization phases (conceptual).
- Seven service interface contracts (OMNI-ABI 0.1 (draft)).
- Capability model, validation rules, resource-accounting model.
- Threat model and stated limitations.
- Document package and integrity manifest.

### Planned for future batches

- Toolchain, build system, hosted test harness.
- Boot code, memory manager, scheduler, IPC, capability implementation.
- User-space driver framework and services.
- Compatibility layers and optional Wasm runtime.
- Exile OS integration plan and migration tooling.

### Not yet implemented or verified

- Any kernel source code.
- Booting on any target.
- Any architecture support.
- Any security control, including capability revocation, cancellation, and randomness.
- Any test, fuzzing, or formal verification.
- Any ABI header, compilation, or conformance suite.

## 11. Reading order

1. This file.
2. `docs/ARCHITECTURE.md` for structure and rationale.
3. `docs/ABI-AND-SECURITY.md` for interface contracts and the security model.
4. `REPORT.md` for exactly which checks were and were not performed.
