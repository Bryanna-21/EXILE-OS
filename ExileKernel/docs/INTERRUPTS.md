# EUK interrupt controllers, ACPI discovery and timers (Mission C3)

Status: **implemented and host-tested; linked into the kernel image; not run in an emulator or
on hardware.** The `--irq` boot case in `scripts/boot_test.py` exists but has never been
executed (no QEMU/GRUB/`x86_64-unknown-none` in the authoring environment).

## Modules

| Package | File | Role |
|---|---|---|
| C3.1 | `src/irq.rs` | vector map, ownership, lock-free dispatch, controller trait, EOI/spurious/unhandled policy, `IrqGuard` |
| C3.2 | `src/pic.rs` | 8259 pair: remap, masks, EOI, spurious IRQ7/15, `PortIo` abstraction |
| C3.3 | `src/acpi/mod.rs`, `src/acpi/madt.rs` | bounded RSDP/RSDT/XSDT/SDT validation, MADT parsing |
| C3.4 | `src/apic.rs`, `src/ioapic.rs` | local APIC (xAPIC/x2APIC), IOAPIC, GSI routing, `ApicController` |
| C3.5 | `src/timer.rs`, `src/pit.rs`, `src/hpet.rs` | timer trait, monotonic clock, PIT fallback, HPET |

## Vector map

`0x00-0x1F` exceptions (never registrable, never EOI'd) | `0x20-0x2F` legacy PIC lines |
`0x30-0xEF` dynamic | `0xF0-0xFE` reserved | `0xFF` APIC spurious. `IrqTable::register` and
`IoApic` routing both refuse anything outside legacy/dynamic, so an exception vector cannot
become a hardware IRQ vector.

## Handler and concurrency rules

* Handlers run with interrupts off (interrupt gates), never re-entered on the same CPU, and
  must not allocate, block, switch context, or take a lock that normal code can hold with
  interrupts on. `spin::SpinLock` does **not** mask interrupts; use `irq::IrqGuard` around
  such sections or use atomics. The C3 handler (`timer::tick_handler`) uses atomics only.
* Table state is atomic: dispatch is lock-free. `register`/`deregister` must be serialised by
  the caller (boot code today). Single CPU only: no per-CPU state exists, so SMP-safe
  deregistration (waiting for a handler running on another CPU) is not implemented.
* A handler is invoked, then the interrupt is acknowledged, then (if unhandled) storm
  protection runs.

## Policy

* **Spurious**: PIC IRQ7/IRQ15 are recognised via ISR (no EOI for IRQ7; master-only EOI for
  IRQ15); the APIC spurious vector 0xFF is counted and never acknowledged.
* **Unhandled** (no handler, or handler returned `NotHandled`): counted, acknowledged, and the
  line is masked after 16 unhandled interrupts since the last registration.
* **Exception vectors** reaching `dispatch` return `NotAnIrq` and touch nothing.

## Kernel integration

`kernel.rs` declares the modules, so the default image compiles and links them, but nothing
runs. Under `--irq` (cfg `euk_irq`, implies `--traps`): PIC remap, read-only ACPI discovery,
timer-source choice, an unhandled-IRQ containment check, a registered-handler tick check, then
everything masked again. `euk_trap_dispatch` routes vectors >= 32 to `irq::dispatch` **only
after a controller is installed**; exceptions and the no-controller case take the unchanged
fatal path (`traps.rs` is not modified). Interrupts are enabled only inside bounded waits,
after the IDT, controller, handlers, EOI path and timer are all in place.

The default image built from this tree is byte-identical to the one built from `main`
(host-target link, same flags).

## Mapping constraint (important)

The bootstrap identity map covers only the first GiB. The local APIC (0xFEE00000), IOAPIC
(0xFEC00000) and HPET (0xFED00000) are outside it, so on real and QEMU hardware:

* `apic::mmio_reachable` is false and the kernel stays on **PIC + PIT**; the APIC, IOAPIC and
  HPET drivers are host-tested with fakes but **never run on hardware**;
* ACPI tables themselves normally sit below 1 GiB and are read through a bounded reader that
  refuses anything beyond the map.

Using the APIC/IOAPIC/HPET needs uncached mappings for those pages (memory-management work,
or a change to `entry.S` that must carry its own emulator evidence). This was not done here.

## Verification matrix

| Check | Result |
|---|---|
| 142 host tests in the C3 group (fakes only; no port/MSR/MMIO access) | pass |
| `#![no_std]` compile of all C3 modules, warnings are errors | pass |
| `clippy` on the C3 files | clean |
| 16 deliberate source mutations (PIC EOI/cascade, dispatch EOI/threshold/vector classes, IOAPIC mask-first and broadcast check, APIC EOI value and spurious handling, RSDP checksum, MADT zero-length entries, HPET comparator writes, tick-conversion overflow, PIT mode) | every one fails the suite; one (MADT zero-length guard) was first undetected, a test was added, and without the guard it now hangs |
| Kernel link, default / `--traps` / `--irq`, host-target `core`, `check.py` | pass, no undefined symbols |
| Default image vs `main` | identical |
| QEMU `pc`/`q35` boot of `--irq`, PIC fallback, HPET, APIC | **not run** |

What host tests do not show: that real 8259/PIT/APIC hardware behaves as modelled, that
interrupts are delivered to these stubs, or that the ACPI tables of a real machine parse.

## Not done

* APIC timer, MSI, IPIs, SMP start-up, per-CPU interrupt state, FPU state in context switches.
* HPET/APIC/IOAPIC on hardware (mapping, above). HPET periodic mode on timers 2+ and
  level-triggered HPET interrupts.
* Scheduler preemption (deliberately not enabled).
* MADT: NMI source, IOSAPIC, x2APIC NMI and platform entries are counted, not decoded.
