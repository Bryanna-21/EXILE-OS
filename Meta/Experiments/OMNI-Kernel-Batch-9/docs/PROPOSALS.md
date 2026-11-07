# Proposals for protected or earlier-batch paths

Batch 9 must not modify `Kernel/`, `ExileKernel/` or Batches 1 to 8, and must not write under `Documentation/` without owner approval.
These are proposals only. None has been applied or tested.

- **P-01 Interrupt masking in the fallback image (SEC-05).** In Batch 7 `kernel/sync.rs`, enable `irq_save`/`irq_restore` for the
  Batch 8 image build, not only for `target_os = "none"` (for example behind a Cargo feature set by `OMNI-Kernel-Batch-8/scripts/build.sh`).
  Test: a boot-time check that a lock taken in thread context keeps IF clear. Needs QEMU, which is unavailable.
- **P-02 `Mapper` rollback (SEC-14).** In Batch 8 `src/paging.rs`, record frames allocated during one `map` call and return them to the
  `FrameSource` on failure. Replace the expectation in `mem_16` (which documents the leak) with an assertion that the pool is unchanged.
- **P-03 ABI version query (SEC-18).** Add a syscall returning the OMNI ABI version (0x0001 in `core/ipc.zig`) and assert it in
  `check_abi_conformance.py`. Needs a syscall number allocation decision by the owner.
- **P-04 Syscall register convention (SEC-20).** Owner decision between the Batch 5/7 convention and `Kernel/`'s; then update the
  checker's expected table.
- **P-05 Exit log.** Replace the 16-entry exit log with a bounded per-space record or a reaping API (Batch 7).
- **P-06 Single syscall stack.** Per-thread kernel stacks before any user thread may block in the kernel (Batch 7/8).
- **P-07 Placement of docs.** After owner approval copy `docs/*.md` into `Documentation/OMNI-Kernel/` (see `APPLY_BATCH_9.md`).
- **P-08 Versioning.** Give Batches 7 and 8 distinct Cargo versions (currently `0.0.0`).
