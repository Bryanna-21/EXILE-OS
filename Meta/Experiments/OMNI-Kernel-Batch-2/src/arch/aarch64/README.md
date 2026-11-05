# ARM64 (AArch64) backend — NOT IMPLEMENTED

**Status in Batch 2: no code exists here. ARM64 does not boot and is not operational.**

This directory is intentionally limited to a requirements note. No assembly,
vector table, or privileged code is included, because none could be tested in this
batch (no emulator or hardware run was performed). A manual probe compiling the core
modules for `aarch64-freestanding` with the Zig compiler used in Batch 2 failed with an
internal code-generation error in that compiler (see REPORT.md), so nothing here has
been compiled for this target.

When a backend is added it must satisfy the contract in `../interface.zig` and the
requirements below. The list is a planning aid drawn from the general ARM64
architecture, not a design that has been validated.

| Area | Requirement to specify and test before implementation |
|---|---|
| Boot entry | Define entry state (exception level, MMU/cache state, device tree or UEFI handoff). Validate all firmware/device-tree data as untrusted. Platform choice (virtual machine first) deferred. |
| Early init | Stack, console, exception-level handling, feature registers. Assembly permitted, minimal. |
| Exception entry | Vector table installation, separate handling for synchronous exceptions, IRQ/FIQ/SError, system-call entry that never trusts user registers, state save/restore. |
| Interrupt controllers | Generic Interrupt Controller (version to be determined per target); architected timer for the scheduler tick. |
| Memory management | Translation table format and granule choice, separate kernel/user translation roots, ASID management, TLB maintenance including broadcast, user/kernel window facts for `UserWindow`. |
| Synchronization | ARM64 has a weaker memory model than x86-64. The backend must supply correct barriers (`fullBarrier`) and the portable core must use explicit atomics; cache maintenance is needed for non-coherent DMA. |
| DMA and devices | System MMU where present; otherwise document the trust consequence. |
| SMP | CPU bring-up through the platform's firmware interface (for example PSCI). Deferred. |
| Verification | First run in a VM, with the result recorded; never claimed from reading code alone. |

Unresolved: target platform and interrupt-controller version, boot protocol, and the
user-space ABI register conventions.
