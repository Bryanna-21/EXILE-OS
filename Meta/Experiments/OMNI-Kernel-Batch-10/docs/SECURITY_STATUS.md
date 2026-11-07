# Security finding status after Batch 10

Rule applied: a finding is not "fixed" because a function exists. Every row says what evidence exists and at what level. Hosted tests are not
hardware evidence. Rows for `Kernel/` are source inspection only; nothing of `Kernel/` was built or run.

| Id | Finding (Batch 9) | Status | Evidence |
|---|---|---|---|
| SEC-05 | interrupt masking in SpinLock fallback | **PARTIALLY FIXED** (OMNI experimental code only) | `irqlock::IrqSpinLock` takes an explicit control, so masking is part of the type. Tests `irq_01..irq_07` pass, including `irq_03` (deferred handler sees a free lock) and `irq_04` (the unmasked design lets a handler observe the held lock). Real `X86Irq` compiles with feature `x86-ring0` and was NEVER executed. Batch 7/8 still use the old lock; nothing was switched over. Not resolved for hardware |
| SEC-12 | hardware isolation not executed | **UNRESOLVED** | No QEMU or hardware here. EUK's own report records QEMU boots in another environment, but those exercise boot, not ring-3 isolation |
| SEC-13 | single syscall stack | **NOT APPLICABLE to `Kernel/` (source); UNRESOLVED in OMNI** | `Kernel/` enters on the current CPU's `TSS.rsp0`, which a context switch reloads from the incoming thread, so each thread has its own kernel stack (`SyscallEntry.cpp`, `Processor.cpp`; source reading only). OMNI Batch 7 still has one; nothing changed in OMNI |
| SEC-14 | page-table frames leaked after mapping failure | **FIXED in the OMNI wrapper (hosted)** | `txmap::map_range_atomic` journals writes and frames and rolls back; tests `tx_02..tx_08` pass; mutation checks (no frame release, no journal undo) made 2 and 5 tests fail. `destroy_tables` frees every table frame (`tx_09`). Batch 8's `Mapper` itself is unchanged and still leaks if called directly. Software page tables only; no TLB or hardware involved |
| SEC-17 | missing hardening | **UNRESOLVED** | `Kernel/` enables SMEP/SMAP/UMIP/NX when the CPU supports them (source). KPTI not found. OMNI/EUK images enable none. Nothing implemented or tested here |
| SEC-18 | no ABI version query | **PARTIALLY FIXED (OMNI)** | `SYS_ABI_QUERY` 0x1F8 returns `major<<16 | minor`, enforces a required major; tests `abi_01`, `abi_04..06`. Exists only in the OMNI wrapper `AbiKernel`, not in `Kernel/` and not in Batch 7's own dispatcher |
| SEC-19 | pinned toolchain verification | **UNRESOLVED (environment)** | `scripts/check_toolchain.py` now reports the mismatch (rustc 1.97.0 vs pinned 1.91.1, no `x86_64-unknown-none`) and the pin was not changed. 1.91.1 cannot be installed offline |
| SEC-20 | register convention mismatch | **PARTIALLY FIXED (mechanism)** | `abi::to_omni(AbiMode::Exile, ..)` and `AbiKernel::syscall`; tests `abi_02`, `abi_03`, `abi_07`. Decision in `ARCHITECTURE_DECISION.md` awaits the owner. Numbers are not translated |

Open weaknesses found while doing this:
- `txmap` limits one transaction to 64 mapping units; a larger request is refused (`TooLarge`), not split, so larger maps are atomic per call only.
- `txmap::destroy_tables` trusts a well-formed tree (built by `Mapper`); a tree with shared children would double-free. The test pool turns that into a failure; production code would need ownership checks.
- Rollback restores memory and frame ownership only; it does not flush any TLB.
- The ABI layer answers a query without the Batch 7 debug/dispatch path, so it duplicates Batch 7's frame validation by calling the same function.
