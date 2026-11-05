# EUK architecture decision — milestone 1

EUK is a separately built, originally authored `no_std` Rust kernel with minimal x86-64 assembly. It does not import Exile/Serenity kernel, AK, or LibC implementation. Rust 1.91.1 and its x86_64-unknown-none target are selected for this milestone; compiler and target archives are checksum-verified. No Cargo dependencies are used.

## Implemented boundary
Legacy Multiboot v1 enters at a low 32-bit entry point. The bootstrap checks the boot magic and required CPU features, sets up a private GDT, enables SSE and long mode, and identity maps the first GiB using supervisor-only 2 MiB pages. Interrupts stay disabled. Rust validates the loader memory map with bounded reads and checked arithmetic, reports on COM1, then halts. QEMU test builds additionally exit through an explicitly configured debug device. This is a real boot test, not an OS self-test.

The bootstrap mapping is RWX: W^X, a guard page, exception handlers, and small-page kernel mappings are mandatory future gates. NX capability is checked and enabled but per-section NX protections are NOT implemented. No user address space or scheduler exists. A portable usable-page iterator is tested but is not a live physical allocator.

## Planned hybrid split (not implemented)
Privileged core: physical/virtual memory management, interrupt dispatch, scheduling, bounded syscall entry, capability-table operations, IPC transfer, and architecture-specific context switching. These mechanisms need controlled privilege and auditable transitions.
User services: filesystems, network stacks, package operations, most device drivers, discovery policy and crash-report storage. Drivers need explicit bounded MMIO/IRQ capabilities; DMA requires IOMMU isolation or a documented trusted-driver exception.

Architecture-independent core owns checked ranges, allocation policy and future capability/IPC semantics. `src/arch/x86_64` owns the boot CPU transition. AArch64 has no implementation or support claim yet; its contract will use its own boot protocol and validated page-table/exception setup.

## Future contracts
- Memory: deny overlaps/overflow; reserve firmware, loader, kernel, modules and page tables before a real allocator. Per-process page tables, W^X and validated copyin/copyout are prerequisites for user mode.
- Threads/processes: explicit ownership, fallible allocation, scheduler queues; lock ordering and IRQ-safe synchronization before concurrency.
- ABI: versioned syscalls with bounded handles/buffers; no inherited ABI until compatibility review. Existing Exile userspace does not run on this milestone.
- IPC/capabilities: generation-checked object handles; rights attenuation and bounded messages; reject stale handles and invalid transfers. No ambient root authority.
- Interrupts/timers: real IDT and fault diagnostics, then timer/preemption tests before enabling interrupts; panic is serial-and-halt for now.
- VFS/storage: user service protocol, mount namespaces, block service; no fake filesystems or device methods.
- SMP: CPU-local state, AP startup, TLB shootdowns and race tests are later milestones.
- Secure boot/update: external verified-loader contract and rollback policy require separate key/caller review. This GRUB test chain is NOT verified boot.
- Configuration/debugging: explicit compile-time fault injection is test-only, separate images; no runtime bypass of validation.
