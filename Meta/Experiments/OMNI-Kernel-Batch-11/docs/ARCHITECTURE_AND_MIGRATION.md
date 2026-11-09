# Final hybrid architecture (x86-64 and ARM64) and migration

**Status: PROPOSED. Nothing here is applied; the owner must approve it.** The author changed no file in `Kernel/` or `ExileKernel/`
because `Kernel/` could not be built or run in the authoring environment. The host clang type-check (see `BUILD_AND_TOOLCHAIN.md`) was found late; it would catch compile errors but not behaviour, and the limited reading of `Kernel/` produced observations, not a change I could justify without a runtime test. They are listed in `THREAT_MODEL_AND_SECURITY_REVIEW.md` for the owner.

## Decision

1. `Kernel/` remains the primary kernel for both x86-64 and ARM64. It already contains a preemptive scheduler, SMP, drivers,
   filesystems and networking; none of those exist in the Rust experiments.
2. `ExileKernel/` (EUK) is kept as a verified-elsewhere Multiboot reference and a place to prototype Rust components.
3. OMNI Batches 2 to 11 stay under `Meta/Experiments/` as a hosted test bed. Parts proven there (capability tables, channel IPC,
   copy-in/copy-out discipline) can be ported to `Kernel/` as separate, reviewed changes.
4. Exile userland uses the `Kernel/` register convention (rax number; rdx, rdi, rbx, rsi) through `AbiMode::Exile`. Syscall numbers
   are not translated: OMNI numbers (0x100..0x1F8) occupy a range that must be reserved in `Kernel/API/Syscall.h` before any port.
5. ARM64 follows `Kernel/` (`svc #0`, number in x8). No OMNI ARM64 syscall entry exists.

## Layers

```
 userland (LibC, LibIPC, services)      uses Kernel/ syscalls today; OMNI IPC is opt-in later
 ------------------------------------------------------------------------------------
 Kernel/ (C++, x86-64, aarch64, riscv64)  production direction
 ------------------------------------------------------------------------------------
 OMNI Rust experiments (hosted tests):  Batch 7 sched+syscalls  Batch 8 boot image  Batch 9 PagedBacking
                                        Batch 10 ABI/txmap/irqlock  Batch 11 capabilities + IPC
 EUK: Multiboot Rust boot milestone (reference)
```

## Migration plan (each step independently reviewable; none performed here)

| Step | Action | Needs |
|---|---|---|
| 1 | Owner approves this architecture | decision |
| 2 | Boot Batch 8 and EUK under QEMU `q35` and `virt` using the written scripts; record real transcripts | QEMU, GRUB tools |
| 3 | Reserve syscall numbers 0x100..0x107 in `Kernel/API/Syscall.h` | owner, C++ build |
| 4 | Port `caps.rs` semantics (generations, rights, retirement) as a `Kernel/` capability table, behind a feature flag, with the negative tests of `tests/caps_unit.rs` re-expressed in the C++ test setup | C++ toolchain, Kernel tests |
| 5 | Port channel IPC with the same wire format (32-byte header, 1024-byte bound) | step 4 |
| 6 | Move userland services from sockets to channels one at a time | steps 4 and 5 |
| 7 | ARM64: replace the source-only stub by `Kernel/Arch/aarch64` boot on `virt` | aarch64 toolchain, QEMU |

## Driver, filesystem and network interfaces

`Kernel/` already owns these. The OMNI experiments define no driver, storage or network interface; a Rust driver would reach the
kernel through channels (capability to a device endpoint) once step 5 is done. That design is not implemented.

## Boot

- x86-64: `pc` (Batch 8 runner) and `q35` (`scripts/qemu_x86_q35.py`, same cases, machine type changed). NOT RUN.
- ARM64 `virt`: `scripts/build_aarch64.sh` and `scripts/qemu_aarch64_virt.sh`. NOT RUN; the stub never assembled here.
