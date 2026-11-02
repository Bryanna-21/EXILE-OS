# Test plan and results

All tests are in `kernel/tests.rs`; run with `cargo test --offline` (or `scripts/verify.sh`). "H" = executed on the
Linux x86-64 host. No test runs in ring 0, in QEMU, or on bare metal.

## Executed (56 tests, debug and release builds)

| id | what it proves | kind |
|---|---|---|
| ctx_01 | initial frame words, alignment arithmetic, canary, scrubbing | H, real layout |
| ctx_02 | `validate_saved_rsp` boundaries and overflow | H |
| ctx_03 | callee-saved regs and rflags preserved across real switches, 3 interleaved threads | H, real asm |
| ctx_04 | new thread's entry gets its argument and rsp = 8 mod 16 | H, real asm |
| ctx_05 | entry that returns runs the exit hook | H |
| ctx_06 | corrupted canary: thread freed, never resumed | H |
| ctx_07 | misaligned / out-of-range saved rsp refused; intact thread still runs | H |
| ctx_08 | stale ids rejected; slot reuse changes generation | H |
| sch_01 | FIFO round robin order | H |
| sch_02 | blocking calls outside a thread are refused | H |
| sch_03 | block then wake; state observed Blocked then Runnable | H |
| sch_04 | wake between prepare and block not lost | H |
| sch_05 | wake between leaving and commit not lost (fault-injected) | H, mutation-checked |
| sch_06 | reason precedence; stale reason not carried over | H |
| sch_07 | sleep times out exactly at the tick; observed Sleeping | H |
| sch_08 | sleep woken early | H |
| sch_09 | zero-tick sleep returns at once | H |
| sch_10 | kill wakes a blocked thread; kill is sticky | H |
| sch_11 | kill before first run: reaped, never ran | H |
| sch_12 | terminate_space kills only its space; kernel space refused | H |
| sch_13 | table full, slot reuse, spawn/exit accounting | H |
| sch_14 | run stops with blocked threads, resumes after wake | H |
| sch_15 | idle only when nothing runnable | H |
| sch_16 | space hook before switch, skipped when unchanged; thread sees own space | H |
| sch_17 | 20 000 wakes from a second OS thread, none lost | H, mutation-checked |
| sys_01 | unknown / aliased numbers are NO_SYS | H |
| sys_02 | identity from scheduler, not registers | H |
| sys_03 | syscall without thread context refused | H |
| sys_04 | exit never returns, code recorded, slot freed | H |
| sys_05 | yield syscall interleaves threads | H |
| sys_06 | sleep bounds, timeout, early wake results | H |
| sys_07 | bad user rip / rsp terminate the thread; boundary value passes | H |
| sys_08 | rflags sanitised | H |
| sys_09 | killed thread exits at the boundary | H |
| sys_10 | `extern "C"` shim: no kernel, installed kernel, null frame | H |
| sys_11 | frame layout and per-CPU offsets match the stub | H |
| sys_12 | entry stub bytes (swapgs, gs moves, cld, ends swapgs; sysretq) | H, reads code bytes, does not run them |
| sys_13 | rdi,rsi,rdx,r10,r8,r9 map to argument positions | H |
| sys_14 | debug syscalls off by default | H |
| ua_01 | map validation (alignment, null page, overlap, top, overflow, table full) | H |
| ua_02 | range validation (edges, spans, permissions, wrap, kernel half) | H |
| ua_03 | copy round trip through syscalls | H, simulated memory |
| ua_04 | same address in two spaces, different memory; unmapped in the other space faults | H, simulated |
| ua_05 | caller must equal loaded space; kernel space and unknown space refused | H |
| ua_06 | copy length cap | H |
| ua_07 | faulting copy writes nothing | H |
| ua_08 | read-only region cannot be written | H |
| ua_09 | process cleanup: threads killed, space destroyed | H |
| ua_10 | kernel threads get no user access | H |
| ua_11 | backing failure is a fault and zeroes the buffer | H |
| msr_01..06 | STAR, selectors, GDT descriptors, FMASK, CPUID SYSCALL bit, MSR numbers | H (CPUID executed; no MSR access) |

## Not executed (and why)

| required item | status |
|---|---|
| boot under QEMU, serial evidence | **not done**: no QEMU, no GRUB / xorriso, no rustc 1.91.1 / `x86_64-unknown-none` |
| `cargo build --target x86_64-unknown-none` | **not done**: target not installed (`verify.sh` reports SKIPPED) |
| SYSCALL from ring 3 through the stub | **not done**: needs ring 0, GDT / TSS / MSRs |
| context switch in ring 0 | **not done** |
| real page-table isolation | **not done**: no page tables |
| blocking IPC wake-up | **not done**: IPC layer is Zig and hosted |
| ARM64 anything | **not done** |

## Boot-time self-test to add when a QEMU environment exists

Under `cfg(test_exit)`, after `kernel_main` has set up GDT/TSS/IDT and a page allocator: spawn three kernel threads that
yield, block and wake each other through `Sched`; run `init_syscall`; enter a ring-3 stub that issues `0x1F3`, `0x1F5`
on its own mapped page, an invalid number and a kernel-address pointer; print results on serial and exit with the
isa-debug-exit code. Expected serial lines should be written next to the test, and CI should compare them verbatim.
