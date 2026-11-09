# Support matrix (Batch 11)

Classification: **IT** Implemented and tested (hosted tests ran); **INR** Implemented, not runtime-tested; **P** Partially implemented;
**E** Experimental; **B** Blocked; **NI** Not implemented. "Hosted" means a host process on x86-64 Linux, not a booted kernel.

| Capability | x86-64 | ARM64 |
|---|---|---|
| Capability tables, rights, generations (Rust) | IT (hosted) | NI (the Rust code is portable `no_std`, never built for ARM64) |
| Channel IPC syscalls 0x100..0x103, 0x106, 0x107 | IT (hosted) | NI |
| Cancellation objects 0x104, 0x105 | NI (return `NOT_SUPPORTED`) | NI |
| Blocking receive with timeout | IT (hosted, real context switch) | NI |
| Blocking send | NI | NI |
| Cleanup on kill and space termination | IT (hosted) | NI |
| Conformance with Zig model | IT (39 checks, 5/5 mutations) | n/a |
| Boot under QEMU | B (no QEMU) | B |
| `q35` boot script | INR (written, NOT RUN) | n/a |
| `virt` boot stub | n/a | P (source only; not assembled, not booted) |
| SMP, preemption | NI in OMNI (exists in `Kernel/`, unbuilt here) | NI |
| Drivers, filesystems, networking | NI in OMNI (exist in `Kernel/`, unbuilt here) | same |
| CI workflow | INR (YAML syntax only) | n/a |
| `Kernel/` build | B (no C++ cross toolchain) | B |
| `Kernel/` static syntax and type check (host clang 18, `scripts/kernel_syntax_check.sh`) | INR as a static check: 432 passed, 0 failed, 7 not checked of 439 | 437 passed, 0 failed, 7 not checked of 444. Not a build, not a test |
| Pinned toolchain (rustc 1.91.1, `x86_64-unknown-none`) | B (unavailable; 1.97.0 on host target used) | B |
| Production readiness | **No** | **No** |
