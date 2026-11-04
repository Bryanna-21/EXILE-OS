# Security notes

Batch 8 makes no security claim. These are the properties it tries to establish, how each is checked, and what is open.

| Property | Mechanism | Evidence level |
|---|---|---|
| No page is both writable and executable | `Mapper` refuses it; ELF has no RWX segment | unit-tested; image checked; boot not run |
| Null page unmapped | kernel map starts at 4 KiB | boot self-test written, **not run** |
| Kernel text read-only, data no-execute | CR0.WP, EFER.NXE, per-section mapping | boot self-tests written, **not run** |
| Kernel mappings never touch the user window; user mappings stay inside it | `check_common` in `Mapper` | unit-tested |
| Exception on a bad stack cannot corrupt kernel state | all gates use IST stacks | gate encoding unit-tested; **not run** |
| Ring 3 cannot use I/O ports | no I/O bitmap (`iomap_base` = TSS size) | layout unit-tested; **not run** |
| Ring 3 cannot reach kernel pages | supervisor bit on all kernel leaves | `translate` unit-tested; ring3-fault boot test **not run** |
| Frames are never handed out twice or freed twice | bitmap allocator | unit-tested |
| Loader data is not overwritten | `reserved_ranges` and `usable_ranges` | unit-tested |
| Bad boot input fails closed | magic, CPU feature, info and map checks | unit-tested; boot fault variants **not run** |

## Known weaknesses

* **Unexecuted code.** The most important caveat: every privileged path is untested on a CPU or emulator. A mistake in a
  descriptor, MSR value or CR write would show as a triple fault.
* **Toolchain deviation.** The image is built with a host-target compiler flow, not the pinned bare-metal target.
* **No SMEP/SMAP/UMIP, no KPTI, no ASLR, no stack protector.** The user and kernel share a page-table root layout
  (kernel mappings are present in the user root, supervisor-only), so a speculative-execution attack is not mitigated.
* **Shared FPU state** between kernel and ring 3 (`SCHEDULER.md`).
* **Single syscall stack** (`PRIVILEGE-TRANSITION.md`).
* **No guard pages** below kernel stacks; Batch 7's canary only detects overflow at the next switch.
* **Trusting the loader.** Memory map entries are checked for structure but not against a second source.
* **Interrupts from unknown sources** are acknowledged and counted, not investigated.
* **Serial output** is polled with a bounded wait; a stuck UART drops bytes, so a failure report could be lost.

## ARM64 implications

Portable in design: the frame allocator, the mapper's policy checks (W^X, window discipline, permission arithmetic) and the
self-test scenario. They need new bit layouts for AArch64 stage-1 descriptors (AP, UXN/PXN, AF, MAIR indexes) and a
TTBR0/TTBR1 split instead of one root. Everything else is x86-specific and would be rewritten: Multiboot and COM1 (UEFI/DTB
and PL011), GDT/TSS/IDT (exception vectors and VBAR_EL1), PIC/PIT (GIC and the generic timer), SYSCALL/SYSRET (SVC and
`eret`), the Batch 7 context switch and entry stub. Nothing in Batch 8 pretends otherwise.
