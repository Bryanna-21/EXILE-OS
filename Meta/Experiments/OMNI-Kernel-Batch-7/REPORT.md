# OMNI-Kernel Batch 7 report

## Verdict, stated first

The batch's own success criterion is that functionality is "connected to the real kernel execution path" and proven
there, and that hosted models, register structures, dispatcher functions and compilation alone do not count.
**That criterion is not met.** There is no QEMU boot, no run in ring 0, and no change to `ExileKernel/`. The reasons are
environmental and structural, listed below, and none of them is papered over. What was achieved instead is real,
tested code that runs on the Linux x86-64 host and is shaped to be dropped into `ExileKernel/` once it can be built.

| spec item | result |
|---|---|
| real x86-64 syscall execution path | **Partial.** Rust dispatch executed (host); entry stub assembled, disassembled, byte-checked, **never executed**; MSR init compiled, never executed |
| ARM64 | abstraction only, as the spec allows; nothing here claims ARM64 works (crate is x86-64 only by `compile_error!`) |
| real scheduler | **Implemented and executed on the host**: runnable, blocked, sleeping, termination, wake-up, kill, reaping, no lost wake-ups (including a concurrent waker). Cooperative only, uniprocessor |
| real context switching | **Implemented and executed on the host** on real stacks: register and flag preservation, alignment, new-thread entry, canary, saved-rsp validation, no switching into freed threads. Not run in ring 0 |
| address-space handling in the switch | scheduler loads the space (hook) before switching, tested; the hook is not a `cr3` write; no page tables |
| syscall + IPC integration | syscall + scheduler: yes (host). **IPC: not done** (Zig / hosted vs Rust kernel) |
| user-memory isolation in the real kernel | **Rules implemented and tested over simulated memory.** No hardware isolation |
| tests | 56 hosted tests, all areas in the spec's list except the ones that need the missing environment |
| formatting and static checks | rustfmt clean, clippy `-D warnings` clean |
| kernel build | **not done** for `x86_64-unknown-none` (toolchain unavailable) |
| QEMU boot tests | **not done** (no QEMU) |

## Why integration into ExileKernel/ was not done

1. **Toolchain.** `ExileKernel/scripts/build.py` requires rustc 1.91.1 and `--target x86_64-unknown-none`. Here:
   `rustc 1.97.0` with only `x86_64-unknown-linux-gnu`; `static.rust-lang.org` returns HTTP 403 so neither the pinned
   compiler nor the bare-metal standard library can be fetched.
2. **Emulator.** `qemu-system-x86_64`, `grub-mkrescue` and `xorriso` are absent; apt cannot reach its mirrors.
3. **Missing prerequisites in ExileKernel/** (read in source): no IDT, no TSS, no user GDT entries, no interrupt
   controller or timer, no allocator, no W^X page tables; `entry.S` ends in `cli; hlt`. Its ROADMAP puts these before
   threads and syscalls.
4. **Language boundary** to the Zig IPC layer.

Editing `ExileKernel/` without being able to compile it for its target or boot it would have produced unverified
changes to a protected tree. The spec allows an isolated design when integration cannot fit; this is it.

## Toolchain and commands (exact)

Host: Linux 6.18.44 x86_64.
`rustc 1.97.0 (2d8144b78 2026-07-07)`, `cargo 1.97.0 (c980f4866 2026-06-30)`, `clippy 0.1.97`, `rustfmt 1.9.0-stable`,
GNU objdump 2.42 (Binutils for Ubuntu). Zig 0.15.2 (locally built, no-LLVM) for the regression.

| command (from the package directory) | exit | result |
|---|---|---|
| `cargo fmt --check` | 0 | clean |
| `cargo clippy --offline --all-targets -- -D warnings` | 0 | clean |
| `cargo test --offline` | 0 | 56 passed, 0 failed |
| `cargo test --offline --release` | 0 | 56 passed, 0 failed; repeated 3 more times, 56 passed each time |
| debug test binary run 5 times in a row | 0 each | 56 passed each time |
| `zig build test --summary all` in `OMNI-Kernel-Batch-6` | 0 | 282/282 (Batch 6: 54, 5: 57, 4: 55, 3: 57, 2: 59) |
| `cargo build --target x86_64-unknown-none` | not run | target not installed |
| QEMU | not run | not installed |

`scripts/verify.sh` runs all of the above and reports SKIPPED for the two it cannot; the full output of the final run
accompanies the delivery as `verify7.txt`. Test counts here are what the commands printed.

## Evidence for the parts that were not executed

Entry stub as linked into the test binary (`objdump -d`), which `sys_12` also asserts byte-wise:

```
swapgs ; mov %rsp,%gs:0x8 ; mov %gs:0x0,%rsp ; push %gs:0x8
push rax rdi rsi rdx rcx r8 r9 r10 r11 rbx rbp r12 r13 r14 r15
mov %rsp,%rdi ; cld ; call omni_syscall_common
pop r15 r14 r13 r12 rbp rbx r11 r10 r9 r8 rcx rdx rsi rdi rax ; pop %rsp ; swapgs ; sysretq
```
The pop order mirrors the push order and the `SyscallFrame` field order (`sys_11`). This shows the code is what was
written; it does not show it works on a CPU.

The bare-metal interrupt masking in `sync.rs` (`pushfq; pop; cli` / `sti`) was assembled once in isolation with the host
compiler (`--emit=obj`); it was not built for a bare-metal target and never executed.

## Mutation checks I ran

* Removing the sequence-number re-check from the block commit made `sch_05` and `sch_17` fail (and `sch_17` took its
  25 s guard to give up). Reverted.
* Replacing one `popq %r12` in `omni_switch_context` with `popq %rax` crashed the test binary (SIGSEGV, exit 139).
  Reverted.

## Modified existing files

**None.** The change is entirely under `Meta/Experiments/OMNI-Kernel-Batch-7/`. `Kernel/`, `ExileKernel/`,
`Documentation/` and Batches 2-6 are untouched (`verify.sh` checks this).

## Classification

See `docs/ARCHITECTURE.md` (fully implemented / partial / hosted simulations / architecture-specific / unsupported).

## Known limitations

* Not booted; not built for the bare-metal target; the SYSCALL stub, MSR writes and bare-metal interrupt masking have never run.
* Cooperative only: no preemption, so a thread that never yields or makes a syscall cannot be descheduled or killed.
* Uniprocessor; the lock is only proven against a second host thread, not against SMP hardware.
* No FPU / SSE state switching (only valid because switches happen at call boundaries).
* No page tables; user-memory isolation is a rule set over a simulated backing store.
* No guard pages; the 8-byte canary misses writes that jump over it.
* 16-bit thread-id generation wraps; 16 threads, 16 KiB stacks, fixed tables.
* Syscall numbers 0x1F0-0x1F7 are provisional; Batch 5 IPC syscalls are not wired; the register convention (Batch 5's)
  differs from `Kernel/`'s and remains undecided.
* `exit_code_of` reads a 16-entry ring; there is no join / zombie state.
* The host tests rely on Linux x86-64 behaviour (no CET shadow stacks, user-mode `popfq`).
* Verified with rustc 1.97.0, not the 1.91.1 pin; the crate needs edition 2024 (rustc >= 1.88).

## Recommended Batch 8

1. Provision an environment with rustc 1.91.1 + `x86_64-unknown-none`, QEMU and GRUB; first re-run `ExileKernel`'s own
   build and boot test to establish a baseline.
2. Land the prerequisites in `ExileKernel/`: GDT with user segments and a TSS, IDT with fault handlers, a physical page
   allocator, kernel page tables with W^X.
3. Add this crate as a module; boot three kernel threads through `Sched` under QEMU with serial output compared
   verbatim; this is the first real evidence for the context switch in ring 0.
4. Run `msr::init_syscall` and a ring-3 stub; execute the SYSCALL stub for real; add a fault-handled `UserBacking`.
5. Add a timer interrupt for tick and preemption; decide FPU state policy at that point.
6. Decide the syscall register convention and FFI boundary to the IPC layer, then connect Batch 5 IPC through
   `prepare_block` / `block` / `wake`.
7. Only after 1-4 pass in QEMU, consider ARM64.
