# Milestone gates

1. Current: isolated original x86-64 boot, serial diagnostics, bounded loader map parsing; portable tests, ELF checks, reproducible ELF, emulator boot and negative tests. No change to Exile OS defaults.
2. Real reserved-region physical allocator, exhaustion/double-free tests; small-page mappings, W^X, guard pages; exception handlers with fault injection.
3. IDT, IRQ controller, timer, kernel heap; allocation failure and interrupt tests before enabling interrupts.
4. Kernel threads, scheduling, locks, tested user-mode address spaces and validated syscall ABI.
5. Capability enforcement, IPC, minimal user service and driver boundaries with crash containment.
6. Separately validated AArch64 cross-build and emulator boot; no support claim from compile alone.
7. SMP, named storage/network/display/input drivers, real hardware, ABI bridge and old OS regression suite.
8. Signed loader/update policy, recovery/A-B boot, review and soak tests; explicit owner approval before default migration.

No Android compatibility work is part of the boot skeleton.
