# Architecture decision: Kernel, ExileKernel and OMNI

Status: **PROPOSED, needs repository-owner approval.** Nothing here changes `Kernel/` or `ExileKernel/`; alternatives are preserved.
This decision rests on reading the source. None of `Kernel/` was built or run in this session (no cross toolchain, no QEMU).

## What exists (from source inspection)

| Tree | What it is | Evidence |
|---|---|---|
| `Kernel/` (1228 files) | Serenity-derived C++ kernel. Memory manager with address spaces, regions and VM objects; processes and threads; a preemptive scheduler with a timer tick; SMP (AP startup, TLB flush broadcast); about 73 syscall handlers; filesystems (Ext2, FAT, ISO9660, DevPts, FUSE); networking; PCI, USB, VirtIO; storage, audio, GPU, input drivers; x86-64, aarch64 and riscv64 arch trees; 55 files in `Tests/Kernel` | `Kernel/Memory/*`, `Kernel/Tasks/*`, `Kernel/Arch/x86_64/Processor.cpp`, `Kernel/Syscalls/*`, `Kernel/Net`, `Kernel/Bus`, `Kernel/Devices` |
| `ExileKernel/` (EUK) | Separate original `no_std` Rust boot milestone: Multiboot v1, long-mode switch, bounded memory-map validation, serial. Its own report records QEMU `pc`/`q35` boots and six negative boot tests, in a different environment with the pinned Rust 1.91.1. Not userspace-capable | `ExileKernel/docs/VERIFICATION.md`, `ARCHITECTURE.md` |
| OMNI Batches 2 to 10 | Zig semantic models (capabilities, IPC, scheduler contracts) and Rust `no_std` experiments (cooperative scheduler, syscall dispatch, W^X mapper, descriptors, boot image) | `Meta/Experiments/OMNI-Kernel-Batch-*` |

Facts about `Kernel/` that matter for this decision (source only):
- Syscall entry switches to the stack in the current CPU's `TSS.rsp0` (`syscall_entry`, `SyscallEntry.cpp`), and a context switch loads `rsp0` from the thread being switched in (`Processor.cpp`, `to_thread->regs().rsp0`). Each thread has its own kernel stack, so SEC-13 as written for OMNI does not describe `Kernel/` (source reading only; not run).
- `Processor.cpp` detects and enables PGE, NX, SMEP, SMAP and UMIP when the CPU has them.
- Syscall register convention (`Kernel/Arch/x86_64/RegisterState.h`): function in rax, arguments in rdx, rdi, rbx, rsi (four arguments).
- `sysretq` is used, except `sigreturn`, which uses `iretq`.
- Not found by search: KPTI. A KASLR claim for the prekernel was not verified.

## Decision

1. **`Kernel/` stays the primary kernel and the only one that boots Exile OS userland.** It already contains most of what phases 1 to 13 ask for on x86-64. Rewriting a working kernel in Rust would duplicate it, discard its test and driver coverage, and force userland rewrites; the programme's own rules forbid unnecessary duplication and incompatible APIs.
2. **`ExileKernel/` (EUK) is kept as the independent, verified-in-QEMU boot reference and as the Rust sandbox** for memory, interrupt and scheduler designs. It is not promoted and not wired in.
3. **OMNI is a specification-and-test-bed layer**, not a competing kernel. Reusable pieces: the capability and IPC *semantics* and their Zig tests (as conformance models for any implementation), the W^X mapper and its transactional wrapper, the interrupt-lock protocol, the ABI version query and the register-convention adaptor.
4. **Syscall ABI (SEC-20):** adopt the `Kernel/` register convention (rax; rdx, rdi, rbx, rsi) for anything that must run Exile userland, because LibC already uses it. The OMNI six-register convention remains only for OMNI test code. Batch 10 `abi::AbiMode::Exile` converts one to the other. Syscall numbers are NOT translated; `Kernel/` and OMNI number spaces are unrelated.
5. **Boot protocol:** unchanged for `Kernel/` (existing prekernel/Multiboot/GRUB flow). EUK/OMNI images stay Multiboot v1.
6. **Components to port into `Kernel/` later (C++), each behind its own approval:** the ABI version query, SMEP/SMAP/UMIP audit results, anything where a OMNI test finds a gap.

## Alternatives preserved (not chosen)

- A. Replace `Kernel/` with a Rust kernel grown from EUK/OMNI. Rejected for now: months of work that cannot be validated without QEMU and hardware, and it discards a working kernel.
- B. Keep both with a shared ABI layer. Possible later; needs the ABI decision above first.
- C. Make OMNI the kernel. Rejected: it is a cooperative, uniprocessor, never-booted test bed.

## Interfaces (summary)

| Topic | Choice / status |
|---|---|
| Memory management | `Kernel/Memory` for production; OMNI `txmap` for the Rust experiments (rollback, teardown) |
| Interrupt and exception architecture | `Kernel/Arch/x86_64` for production; OMNI Batch 8 `idt` is never booted |
| Driver architecture | `Kernel/Bus` and `Kernel/Devices`; no new drivers written here |
| User-space interface | `Kernel/Syscalls` and LibC; OMNI layer is test-only |

## What needs the owner

Approve or change items 1 to 4. Until then no file under `Kernel/` or `ExileKernel/` is edited.
