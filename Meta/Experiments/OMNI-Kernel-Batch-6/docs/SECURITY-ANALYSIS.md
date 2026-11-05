# Security analysis (hosted, simulated)

Scope: the architecture and scheduler boundary as modelled and tested in a hosted process. Real hardware behaviour,
real user/kernel memory, speculative-execution effects and timing channels are out of scope. This is an analysis of
the model and a list of requirements, not an audit.

| requirement | how it is met in the model | evidence |
|---|---|---|
| capability authority preserved | all authority checks stay in Batch 3/4/5; the entry only forwards | ENT-01, ENT-03, IPC-10 |
| calling space from trusted kernel state | `syscallEntry` takes it from the thread found through the CPU; no register or user value names a space | ENT-03, IPC-10 |
| user arguments cannot select another space | spare registers stuffed with other spaces' ids are rejected (omni) or ignored; handle values resolve only in the caller's table | ENT-03, ENT-04 |
| syscall validation and reserved fields kept | Batch 5 `dispatch` is called unchanged; wide numbers and wide handles rejected, not truncated | ARCH-03, ENT-05, ENT-06 |
| invalid entry states | not user mode, IOPL, bad pc/sp, wrong exception class, IRQ context, interrupts off, spinlocks held, no current thread, dying process, unknown space: all FATAL, nothing executed | ARCH-05..07, ENT-02 |
| user pointers never trusted | only copy functions; overflow-safe range check; kernel half refused | UM-01, UM-02, UM-07 |
| thread exits while blocked | woken with `killed`; the operation returns `Cancelled`; slot returned; nothing consumed | IPC-07, IPC-08 |
| endpoint closes while blocked | waiter woken with `PeerDead`; no stuck waiter | IPC-05 |
| process terminates while blocked | kill, close, wake; every thread exits; no leak | PR-01, PR-02, PR-04 |
| stale or forged thread ids | generation-checked ids; stale ids ignored or refused | TS-03 |
| lost wake-up / invalid state change | eventcount protocol; transition table enforced; invalid operations change nothing | WK-02, WK-06, TS-01, TS-02 |

## Findings and limits

1. **Two blocking paths.** A thread blocked inside Batch 5 `dispatch` is invisible to the scheduler model, so a kill of one
   thread of a live process cannot wake it (PR-03 shows only whole-process closure does). Until Batch 5 takes a wait
   provider, a real integration would have a thread that cannot be killed promptly. Severity: availability, not authority.
2. **Broadcast wake-ups.** Every IPC state change wakes every registered waiter; a process with many waiters costs wake-ups
   (bounded by the Batch 5 waiter limits of 16 per space and 64 in total).
3. **Partial copy_out on an unmap race** leaves user memory in an unspecified state for the written range; the kernel stays
   consistent (UM-04, UM-08). A real implementation has the same race unless the address space is locked.
4. **ABI undecided.** The two register conventions differ; neither is a production ABI. A wrong choice is an
   interoperability risk, not a model flaw.
5. **Process-fatal entry errors rely on the stub.** If a real stub ignored `EntryResult.fatal` and returned to user mode,
   the guarantee would be lost; the stub contract states it must not.
6. **Model, not kernel.** Races exist that the hosted model cannot show (IRQ re-entrancy, SMP memory ordering on weakly
   ordered ARM64, TLB shootdown). Concurrency evidence is stress testing on x86-64 Linux, Debug build, one compiler.
7. **Counters** of Batch 5 count Batch 5 interface attempts; a blocked `IpcSched` call polls, so `*_failed` counters
   include each `WouldBlock` poll. They are statistics, not accounting.
