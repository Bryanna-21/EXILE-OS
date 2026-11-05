# Syscall integration

## Path

```
user: syscall            (rax = number, rdi rsi rdx r10 r8 r9 = arguments, rcx = rip, r11 = rflags)
  -> omni_syscall_entry  swapgs; user rsp -> gs:8; rsp = gs:0 (kernel stack); push 16 registers; cld
  -> omni_syscall_common(frame)          extern "C", reads the installed Kernel
  -> Kernel::syscall(frame)
       1. caller = Sched::current()      identity comes from the scheduler only
       2. validate_frame                 rip and user rsp below USER_TOP, r11 sanitised
       3. dispatch(number, args)         number > u32::MAX is NO_SYS (no truncation)
       4. result -> frame.rax
       5. killed? -> exit_current        a killed thread never returns to user mode
  <- pops; pop rsp; swapgs; sysretq
```

Steps 1-5 are Rust and **run in the tests** (`sys_01`..`sys_14`, `ua_*`). The assembly around them is **not
executed**; it is assembled, its bytes are asserted (`sys_12`) and the frame layout it builds is asserted field
by field against `SyscallFrame` (`sys_11`). The disassembly of the stub as linked is recorded in `REPORT.md`.

## Frame

`SyscallFrame` is `repr(C)`, 128 bytes, lowest address first:
`r15 r14 r13 r12 rbp rbx r11 r10 r9 r8 rcx rdx rsi rdi rax user_rsp`. The stub's push order is the reverse of that.
Kernel stack alignment: `gs:0` must be 16-byte aligned; sixteen pushes keep it aligned at the `call`.

## Register convention

x86-64: number in `rax`; arguments `rdi, rsi, rdx, r10, r8, r9` (`rcx` is overwritten by SYSCALL); result in `rax`.
This is the Batch 5 *proposed* convention, **not** the one the existing `Kernel/` uses (number rax; arguments
rdx, rdi, rbx, rsi). The choice is open and is the project owner's; `sys_13` pins the mapping implemented here.
ARM64: not implemented. (Batch 6's `arch.zig` has a hosted register abstraction; its x8 / x0..x5 convention is
unchanged and nothing in this batch runs on ARM64.)

## Syscalls implemented (all PROVISIONAL numbers)

| number | name | behaviour |
|---|---|---|
| 0x1F0 | yield | switch away, returns 0 |
| 0x1F1 | exit(code) | terminate the calling thread; does not return |
| 0x1F2 | sleep(ticks) | 0 on timeout, 1 if woken early, CANCELLED if killed or cancelled; > 2^20 ticks is INVALID_ARGUMENT |
| 0x1F3 | thread_self | the caller's thread id (generation << 16 \| index) |
| 0x1F4 | space_self | the caller's address-space id |
| 0x1F5-0x1F7 | debug checksum / fill / args | only when the kernel was built with `debug_syscalls`; otherwise NO_SYS |

Everything else, including the Batch 5 numbers 0x100-0x107, is `NO_SYS` (-33). **The Batch 5 IPC syscalls are not
connected**: that layer is Zig and hosted. Status codes are the Batch 2/5 values (`status.rs`).

## Validation rules (each has a test)

* Caller from the scheduler; no register selects it (`sys_02`); no thread context means INVALID_STATE (`sys_03`).
* Unknown numbers and numbers with high bits set, including `(1<<32)|0x1F0`, are NO_SYS (`sys_01`).
* User rip or rsp at or above `USER_TOP` terminates the thread with `EXIT_BAD_FRAME` (`sys_07`). The rip rule exists
  because SYSRET to a non-canonical address faults in ring 0 on Intel CPUs.
* `r11` is masked to CF PF AF ZF SF DF OF and forced to IF | reserved bit 1, so IOPL, TF, NT, RF, VM, AC cannot
  reach user mode (`sys_08`).
* A killed thread exits at the boundary (`sys_09`).

## Integrating into ExileKernel (not done)

1. Rust >= 1.91.1 with `x86_64-unknown-none`; add the crate as a module or path dependency.
2. Extend `entry.S`'s GDT to `null, 0x08 kcode64, 0x10 kdata, 0x18 udata, 0x20 ucode64` plus a TSS, and load them
   (`msr.rs` has the descriptor constants). Add an IDT; a SYSCALL needs no IDT entry but faults do.
3. Allocate a kernel stack per CPU, fill a `PerCpu { kernel_rsp, user_rsp }`, call `msr::init_syscall(&mut percpu)`
   on each CPU, call `install_kernel(&KERNEL)`.
4. Build real user page tables; implement the `Sched` space hook as a `cr3` write; build a ring-3 entry (`iretq`).
5. Boot under QEMU with the isa-debug-exit device and add the boot-time self-test from `TEST-PLAN.md`.
