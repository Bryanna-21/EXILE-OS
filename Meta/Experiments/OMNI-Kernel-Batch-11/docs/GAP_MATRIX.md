# Kernel gap matrix

Written from the source at `origin/main` 92ab9921 (which already contains OMNI Batches 8 to 10), not from earlier reports. Evidence
levels: **S** read in source; **T** a hosted test ran in this batch's verification; **N** nothing executed. `Kernel/` was never built
or run by the author of this batch (no C++ cross toolchain, no QEMU), so every statement about it is **S**. One static check was possible: `scripts/kernel_syntax_check.sh` parses and type-checks
`Kernel/` translation units with host clang 18 (no code generation, no link, no execution): 432 of 439 x86-64 units and 437 of 444 AArch64 units pass, none fail, 7 per architecture are not
checked (they need LibVT's generated state-machine header). That shows the sources are syntactically and type consistent under those flags, nothing more.

Status vocabulary (used in all documents of this batch): *Implemented and tested* (hosted tests ran), *Implemented, not runtime-tested*,
*Partially implemented*, *Experimental*, *Blocked*, *Not implemented*.

| Area | Where it stands at 92ab9921 | Evidence | Gap | Disposition in this batch |
|---|---|---|---|---|
| x86-64 boot | `Kernel/` boots through its Serenity-derived loader; `ExileKernel/` (EUK) boots a Multiboot Rust milestone (QEMU-verified elsewhere, per its own docs); OMNI Batch 8 image never booted | S | No boot evidence reproducible in this environment | **Blocked**: no QEMU. `scripts/qemu_x86_q35.py` written for `q35`, NOT RUN |
| ARM64 boot | `Kernel/Arch/aarch64` exists (Serenity port); no OMNI/EUK ARM64 code | S | No tested ARM64 path | **Partially implemented**: `arch/aarch64-virt/boot.zig` source only; the authoring Zig could not assemble it. NOT BUILT, NOT BOOTED |
| Memory management | `Kernel/Memory` (full VMM, NX, SMEP/SMAP enabling); OMNI Batch 8 `Mapper`, Batch 9 `PagedBacking`; Batch 10 `txmap` | S, T (OMNI parts) | Frame ownership leak on `Mapper` failure was fixed in Batch 10 (`txmap`); not applied to `Kernel/` | No change. Documented |
| Processes and scheduling | `Kernel/` preemptive scheduler, SMP; OMNI Batch 7 cooperative uniprocessor, 16 threads | S, T | OMNI is not preemptive or SMP | **Not implemented** for OMNI. `Kernel/` is the production-direction scheduler |
| Syscalls and ABI | `Kernel/` ~73 syscalls, register convention rax; rdx, rdi, rbx, rsi (confirmed in `Kernel/API/Syscall.h`); OMNI Batch 7/10 syscalls 0x1F0..0x1F8 with `AbiMode::{Omni, Exile}` | S, T | IPC numbers 0x100..0x107 returned `NO_SYS` in the Rust kernel | **Implemented and tested** (hosted): 0x100..0x107 in `omni11::sys` |
| IPC | Zig models (Batches 4, 5) only; `Kernel/` has local sockets, pipes, shared memory but no capability channels | S | Not in any Rust kernel | **Implemented and tested** (hosted): channel IPC with bounded queues, blocking receive with tick timeouts, move/copy transfer |
| Capability security | Zig models (Batches 2 to 4). `Kernel/` uses pledge/unveil promises, not capabilities | S | Not enforced anywhere in Rust | **Implemented and tested** (hosted): per-space tables, generations, rights, type check, retirement, GC of cycles |
| Synchronization | `Kernel/` spinlocks, mutexes, wait queues; Batch 7 `SpinLock`; Batch 10 `irqlock` | S, T | OMNI locks are single-CPU assumptions | **Partially implemented**: lock order for IPC documented (IPC state lock, then scheduler lock); no SMP |
| SMP | `Kernel/` has SMP; OMNI none | S | none in OMNI | **Not implemented** (OMNI) |
| Drivers | `Kernel/Devices`, PCI, USB, VirtIO, storage, network drivers | S | OMNI has no driver interface | **Not implemented**; interface notes only (`ARCHITECTURE_AND_MIGRATION.md`) |
| Filesystems | `Kernel/FileSystem` (Ext2, FAT, ISO9660, Plan9, tmp, proc and others) | S | none in OMNI | **Not implemented** in OMNI. No change to `Kernel/` |
| Networking | `Kernel/Net` (TCP/UDP/IPv4, ARP, loopback, drivers) | S | none in OMNI | **Not implemented** in OMNI |
| Kernel security | SMEP/SMAP/UMIP/NX enabled in `Kernel/` (S); KPTI not found; OMNI SEC-01..20 list | S, T | KPTI absent in `Kernel/`; hardware enforcement in OMNI never executed | See `THREAT_MODEL_AND_SECURITY_REVIEW.md`; SEC-01/02/03/09 now have Rust enforcement (hosted) |
| Diagnostics | `Kernel/` debug output, KASAN-like options per its CMake; OMNI self-tests and report modules | S | no unified diagnostics | **Partially implemented**: `Stats` per endpoint; scheduler stats exist from Batch 7 |
| Build/toolchain/CI | Pinned `rustc 1.91.1` + `x86_64-unknown-none` + Zig 0.15.2 (Batch 10 `toolchain.pin`); no CI for OMNI | S | pin unavailable here | **Implemented, not runtime-tested**: `.github/workflows/omni-kernel.yml` (YAML syntax checked only) |
| Tests | Zig 287 raw test declarations (Batches 2 to 6); Rust Batches 7 to 10; Batch 11 adds 18 | T | no boot test passes anywhere in this environment | See `VERIFICATION_REPORT.md` |
| Userland | LibC wrappers use `Kernel/` convention; SystemServer and LibIPC use sockets | S | no Exile userland uses OMNI IPC | `USERLAND_INTEGRATION.md` |
