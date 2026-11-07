# Architecture support matrix

Categories: IMPLEMENTED AND TESTED / IMPLEMENTED BUT NOT FULLY TESTED / EXPERIMENTAL / NOT IMPLEMENTED / BLOCKED.
Nothing is inferred from directory names, interfaces or compilation. Evidence columns name the test or the reason.

## By execution environment

| Category | Status | Evidence and limits |
|---|---|---|
| Architecture-neutral logic: wire format, handles, rights, capability lifecycle, IPC model, scheduler model (Zig B2 to B6) | IMPLEMENTED AND TESTED (hosted models) | Zig test suites; they are models, not a kernel, and are not linked to the Rust kernel |
| Architecture-neutral logic in Rust: frame allocator, W^X policy checks, `PagedBacking` policy | IMPLEMENTED AND TESTED (hosted) | B8 `frames_tests`, `paging_tests`; B9 `mem_*` |
| x86-64 implementation: cooperative scheduler, context-switch asm, SYSCALL stub | IMPLEMENTED AND TESTED (hosted, executing real asm on an x86-64 host) | B7 tests, B9 `sys_*`, `cf_04`. Hosted execution is not ring-3 execution |
| x86-64 implementation: GDT/TSS/IDT, paging, boot, ring-3 entry, timer | IMPLEMENTED BUT NOT FULLY TESTED | B8 unit tests cover encodings and policy; privileged paths never executed |
| ARM64 implementation | NOT IMPLEMENTED | Only a Zig SIM contract (B6 `arch.zig`) names ARM64 constants. No Rust, no assembly, no tests of any ARM64 code |
| Hosted testing | IMPLEMENTED AND TESTED | See verification report for counts |
| Bare-metal compilation (pinned `x86_64-unknown-none`, rustc 1.91.1) | BLOCKED | Target and compiler unavailable here |
| Bare-metal image built with host-target fallback | EXPERIMENTAL | B8 `scripts/build.sh`, `check_image.py`; an ELF exists and is statically checked; not proof of runtime support |
| QEMU execution | BLOCKED | No QEMU, GRUB, xorriso or KVM available; B8 QEMU tests report NOT RUN |
| Real hardware execution | NOT IMPLEMENTED (never attempted) | No hardware access |
| Production-kernel integration (`Kernel/`, `ExileKernel/`) | NOT IMPLEMENTED | Neither was modified; protected paths. Proposals only |

## By capability

| Capability | Zig model | Rust kernel | Status of the Rust kernel part |
|---|---|---|---|
| Capabilities / rights / revocation | tested | none | NOT IMPLEMENTED |
| IPC channels, queues, capability transfer | tested | none | NOT IMPLEMENTED |
| Resource accounting (pages, handles, threads, queued bytes) | tested | fixed table limits only | NOT IMPLEMENTED (as accounting) |
| Thread lifecycle and cooperative scheduling | tested (SIM) | tested hosted | IMPLEMENTED AND TESTED (hosted); preemption NOT IMPLEMENTED; SMP NOT IMPLEMENTED |
| Context switch | SIM only | real asm, hosted | IMPLEMENTED AND TESTED (hosted); FPU/SSE not per thread |
| Syscall dispatch (5 syscalls) | proposed table | implemented | IMPLEMENTED AND TESTED (hosted); IPC syscalls NOT IMPLEMENTED |
| User-address validation | model | region model + page-table walk | IMPLEMENTED AND TESTED (hosted, software page tables) |
| Hardware page-table enforcement | n/a | B8 | IMPLEMENTED BUT NOT FULLY TESTED (never executed) |
| Ring-3 isolation | n/a | B8 optional stage | IMPLEMENTED BUT NOT FULLY TESTED (never executed). A syscall dispatcher is not proof apps can run isolated |
| Timer / interrupts | n/a | B8 | IMPLEMENTED BUT NOT FULLY TESTED |
| Interrupt masking inside locks | n/a | `target_os = "none"` only | IMPLEMENTED BUT NOT FULLY TESTED; absent in the fallback image |
| Syscall ABI version query | wire format only | none | NOT IMPLEMENTED |
| Boot under Exile OS | n/a | own Multiboot image | EXPERIMENTAL; not part of the OS build |
