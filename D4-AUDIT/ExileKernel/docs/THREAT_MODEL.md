# Boot milestone threat model

Trusted: firmware, GRUB loader, Rust/GNU toolchain and emulator. The loaded kernel and loader-supplied physical pointers are not authenticated. Host compromise, malicious firmware and physical attacks are outside the tested model.

Enforced now: reject invalid magic/unsupported CPU features; bound handoff/map addresses to the mapped GiB; bound map length to 64 KiB and record count to 128; reject empty, truncated, zero-length, overflowing or overlapping regions; checked alignment/range arithmetic; serial errors and halt rather than continued initialization. Supervisor-only page tables prevent a hypothetical user mapping, but no user-mode process exists to verify isolation.

Unsafe code: assembly I/O, GDT/page-table/CPU setup and bounded physical pointer reads. Address bounds alone do NOT prove the range is genuine RAM or protect against a malicious loader. No IDT means an unexpected CPU exception may triple-fault rather than produce diagnostics. COM1 is assumed present; Rust and assembly diagnostic writes use bounded polling. Page tables are explicitly zero-initialized, including a dirty-table regression boot variant.

Overlapping firmware entries deliberately fail closed in this restricted milestone. Such entries can be legitimate, so this is a compatibility limitation, not evidence of malicious firmware. Reserved-precedence normalization is a future prerequisite for broader platform support and a live allocator. BOOT_OK means long-mode entry, bounded map parsing and halt-only bring-up, not that loader/kernel reservations are ready for allocation.

Not enforced: W^X bootstrap (current 2 MiB identity pages are RWX), stack guard, exception recovery, live allocator reservations, capability access, syscall validation, DMA/IOMMU isolation, secure/verified boot, anti-rollback or signed updates. No claims of overall memory safety, exploit immunity or formal verification. Fault injection tests explicit compile-time test variants; production has no runtime validation bypass.

Before running untrusted userspace, implement and test all privilege transitions and page protections. Before real hardware, add IDT and robust failure diagnostics and validate platform memory/device assumptions.
