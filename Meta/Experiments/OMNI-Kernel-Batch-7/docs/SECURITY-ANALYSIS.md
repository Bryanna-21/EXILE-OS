# Security analysis

Scope: what Batch 7 enforces in code that was executed, what it enforces only on paper, and what is missing. Nothing
here has been reviewed by a third party and nothing ran with a hostile user program.

## Trust boundaries

| boundary | enforced by | status |
|---|---|---|
| user -> kernel entry | `omni_syscall_entry` + `validate_frame` | frame checks tested; the stub never executed |
| thread identity | scheduler `current()`; no register carries it | tested (`sys_02`) |
| syscall number | full 64-bit compare; high bits are NO_SYS | tested (`sys_01`) |
| user pointers | `AddressSpace::check_range` before any copy | tested (`ua_02`..`ua_08`); hardware paging absent |
| address-space ownership | caller space must equal the loaded space | tested at unit level (`ua_05`); kernel threads get none (`ua_10`) |
| kernel -> user return | rip / rsp below `USER_TOP`, rflags masked | tested (`sys_07`, `sys_08`) |
| terminated threads | sticky kill, exit at boundary, reaping | tested (`sys_09`, `ua_09`) |
| thread stacks | canary + saved-rsp validation | tested (`ctx_06`, `ctx_07`) |

## Specific threats

* **Confused-deputy via spoofed identity** (a user passes another thread's id in a register): there is no such field.
* **Number aliasing** (`(1<<32) | valid`): rejected, not truncated.
* **Non-canonical SYSRET** (CVE-class on Intel): rcx is validated before return; a bad frame kills the thread instead
  of returning. The check is in Rust and tested; the hardware fault itself was not reproduced.
* **IOPL / TF / NT / RF / VM / AC smuggling through r11:** masked on the return path. FMASK clears TF IF DF NT AC on
  entry (`msr.rs`; the MSR write was not executed).
* **Integer overflow in ranges:** `checked_add`; wrap-around and `u64::MAX` lengths are Faults (`ua_02`).
* **Partial copies:** the whole range is validated first; a faulting write changes nothing (`ua_07`); a failed read zeroes
  the kernel buffer (`ua_11`).
* **Kernel-memory disclosure through user pointers:** the user half ends at `USER_TOP`; the null page cannot be mapped.
* **Stale data between threads:** new stacks are fully zeroed.
* **Lost wake-ups as a denial of service:** closed by the sequence-number protocol and tested under a real concurrent
  waker (`sch_05`, `sch_17`).
* **Slot reuse / ABA on thread ids:** 16-bit generation. It wraps after 65 536 reuses of one slot, so a stale id could
  match again in a very long-running system; a wider counter is a one-line change but has not been made.

## Known gaps (not mitigated)

1. **No preemption.** A user thread that never makes a syscall owns the CPU and cannot be killed.
2. **No real isolation.** `AddressSpace` is a bookkeeping structure. Without page tables nothing stops ring-3 code from
   touching kernel memory, and the TOCTOU between `check_range` and the copy is only addressed by the copy routine
   (a fault-handled `UserBacking`) that has to be written for the real kernel.
3. **Syscall stub unexecuted:** swapgs ordering, the stack switch and the per-CPU block are reasoned about and
   byte-checked, not run. An NMI or machine check between `swapgs` and the stack switch is not handled (the standard
   reason Linux uses IST stacks).
4. **No stack guard pages**, no kernel stack sharing rules, no interrupt stacks.
5. **`sync.rs` interrupt masking** for bare metal compiles in isolation only.
6. **Debug syscalls** exist; they are off unless the kernel is constructed with `debug_syscalls = true` (`sys_14`).
7. **No resource limits** beyond the 16-thread table, 8 regions per space, 8 spaces and a 1 MiB copy cap.
8. **Provisional numbers** 0x1F0-0x1F7 are not an ABI.
