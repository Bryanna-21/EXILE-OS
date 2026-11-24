# EUK D5 — Process & Scheduling Subsystem: delivery report

Branch: `d5/process-scheduling` (local, uncommitted; `main` untouched). Baseline: D1–D3 + C3 (`main`, 616c410a).

## Integration changes kept in the repository (not in the ZIP)
dispatch.rs (IRQ-masked scheduler lock and state-change+switch, blocking guards, switch-in hook, 4 real-stack tests),
kthread.rs (+3 Error variants), thread.rs / sched.rs (priority-inheritance floor, `set_inherited`),
kernel.rs (module declarations, IRQ enter/leave + exit hook in the trap path), scripts/test.py (new modules).

## Results actually executed
- `python3 scripts/test.py`: exit 0, 3 consecutive runs. A1/C1 modules 18+12+0+0+13+14+13; D1–D5 binary 258 passed / 0 failed; C3 142 passed; no_std `-D warnings` check passes for all modules.
- New tests: sync 16, preempt 7, process 18, address_space 21, elf 7, tls 14, usermode 16 (99) + 4 real-context-switch tests (timer tick preempts a thread that never yields; preempt-disable defers the switch; priority inheritance on real stacks; blocking semaphore).
- Kernel builds `build.py` (default, --traps, --irq, --demo): all link, ELF/Multiboot/entry/static checks PASS, 0 warnings. Built with rustc 1.97 in unpinned host-target mode (EUK_ALLOW_UNPINNED_RUSTC=1 EUK_TARGET=host).
- rustfmt --check clean on the 7 new files. Bare-metal cfg (`target_os="none"`) path of address_space type-checked.
- 8 mutation checks (no inheritance, no de-boost, deadlock check off, lock order off, preempt ignores depth, W+X allowed, threads not killed, address space not freed): all detected.

## NOT done / limitations
- No QEMU/GRUB/hardware run (not installed). Nothing here has run in an emulator. Ring-3 entry, syscall entry stub, CR3 write, MSR writes, trap exit hook are compiled/linked but never executed.
- Pinned Rust 1.91.1 / `x86_64-unknown-none` build not performed (not installed, not installed without approval). No reproducibility claim.
- Not wired into the boot path: no process is created at boot; there is no on-target D5 demo.
- Single CPU only. FPU/SSE/x87 state is not saved across preemption (user code must not use it).
- No sync timeouts; deadlock detection covers mutex cycles only.
- Page-table code assumes the bootstrap identity map for physical access (`IdentityPhys`); no kernel frame allocator exists yet (A2 workstream, untouched), so frames come from a caller-supplied FrameAlloc.
- Spec scope: the D5 definition used is the attached brief (7 modules); the repo itself defines no D5.
