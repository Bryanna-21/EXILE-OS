# x86-64 backend — NOT IMPLEMENTED

**Status in Batch 2: no code exists here. x86-64 does not boot and is not operational.**

This directory is intentionally limited to a requirements note. No assembly,
bootloader, linker script, or privileged code is included, because none could be
tested in this batch (no emulator or hardware run was performed).

When a backend is added it must satisfy the contract in `../interface.zig` and the
requirements below. The list is a planning aid drawn from the general x86-64
architecture, not a design that has been validated.

| Area | Requirement to specify and test before implementation |
|---|---|
| Boot entry | Choose a boot protocol (for example UEFI, or a protocol-based loader). Define the handoff structure and validate every firmware-supplied field (memory map, framebuffer, ACPI pointers) as untrusted input. Decision deferred. |
| Early init | Stack, serial console, CPU feature detection, long-mode state checks. Assembly permitted, minimal. |
| Exception and syscall entry | Descriptor tables, per-CPU stacks for faults, a system-call entry path that never trusts user registers, correct state save/restore. |
| Interrupt controllers | Local APIC and I/O APIC (and legacy PIC masking); timer source selection (local APIC timer, TSC calibration). |
| Memory management | Four-level paging, NX and write-protect enforcement, per-address-space root switching, TLB invalidation including cross-CPU shootdown, user/kernel window facts for `UserWindow`. |
| Synchronization | x86-64 has a comparatively strong memory model, but the portable core must still use explicit atomics and barriers; the backend supplies `fullBarrier`, `cpuRelax`, and interrupt save/restore. |
| DMA and devices | Detect and use an IOMMU where present; document the trust consequence where absent (Batch 1, ARCHITECTURE.md section 11). |
| SMP | Application-processor startup and per-CPU state. Deferred. |
| Verification | First run in a VM, with the result recorded; never claimed from reading code alone. |

Unresolved: which boot protocol, which Zig features are usable freestanding at the
pinned version, and how the user-space ABI register conventions are fixed.
