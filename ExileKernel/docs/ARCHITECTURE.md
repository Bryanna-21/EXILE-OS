# EUK architecture decision — current isolated milestone

EUK is a separately built, originally authored `no_std` Rust kernel. It remains alongside, and does not replace, the inherited Serenity-derived `Kernel/`. The current EUK build uses Rust 1.91.1 and targets `x86_64-unknown-none`; it has a minimal x86-64 assembly entry and a bounded Multiboot v1 memory-map parser.

## Shared architecture contract

`src/platform.rs` defines an architecture and boot-capability profile independent of the entry assembly. The kernel prints the selected profile during boot, and the same module is compiled by the host unit-test harness. This contract is deliberately descriptive: capability is marked implemented only when the current EUK path performs that operation.

For this EUK milestone, x86-64 supports only its boot entry, bounded memory-map parsing, and early serial output. Exception dispatch, interrupt controller setup, timers, a general virtual-memory subsystem, SMP, PCI, DMA, power management, native userspace, compatibility runtimes, and a guest hypervisor are not implemented. The AArch64 profile is explicitly reported as unsupported by EUK, despite the separate inherited kernel containing AArch64 source.

## Inherited kernel HAL

The existing `Kernel/Arch/` already selects architecture-specific register, interrupt-controller and thread-register headers for x86-64, AArch64 and RISC-V. It contains platform code such as x86 APIC/IOAPIC and AArch64 GIC/PSCI, timers, and DeviceTree initialization. Those are inherited kernel implementations, not EUK backends, and their presence does not alone prove successful target builds or hardware support. This change does not modify or replace them.

## Execution environments

The inherited Exile OS kernel provides the existing native process, thread, syscall and IPC environment. EUK has no userspace, process scheduler, syscall ABI, or native application execution yet. No compatibility runtime registration or guest VM lifecycle manager is implemented by this batch. `Meta/run.py` launching the OS in QEMU is host-side OS testing, not an in-guest virtualization service. Hardware acceleration detection in the run script is not guest virtualization support.

## Integration and verification

`platform.rs` is included in `src/kernel.rs` and is unit-tested by `scripts/test.py` together with the existing memory parser tests. The EUK link/build path remains x86-64-only. AArch64 boot support requires a separate entry protocol, exception/vector setup, translation-table implementation, timer/interrupt backend, linker/build target, and QEMU boot tests. No claim is made for AArch64 EUK, native userspace in EUK, compatibility execution, or in-guest virtualization.
