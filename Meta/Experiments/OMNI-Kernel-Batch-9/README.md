# OMNI Kernel Batch 9: final integration (experimental)

Joins Batch 7 (cooperative scheduler, syscall dispatch, user-address model) to Batch 8 (page tables, frames, descriptors) through
`PagedBacking`, adds 34 hosted end-to-end and conformance tests, a cross-language ABI conformance checker, and the programme
documentation and security review.

- **Experimental. Not production-ready.** Hosted tests only; nothing was booted, run under QEMU or run on hardware.
- Not integrated into `Kernel/` or `ExileKernel/`; neither was modified, nor were Batches 1 to 8.
- Capabilities and IPC exist only as Zig models; the Rust kernel does not implement them.
- Cooperative scheduling, uniprocessor, x86-64 only. No ARM64.
- Depends on `../OMNI-Kernel-Batch-7` and `../OMNI-Kernel-Batch-8` (Cargo path dependencies).

## Layout

| Path | What |
|---|---|
| `src/lib.rs` | `PagedBacking`: user access allowed only if Batch 7's region model and Batch 8's page-table walk both agree |
| `tests/` | `e2e_memory.rs` (17), `e2e_syscall_sched.rs` (10), `e2e_conformance.rs` (7) |
| `scripts/check_abi_conformance.py` | Zig and Rust constant cross-check (`--self-test` mutates and expects failure) |
| `scripts/verify.sh` | Whole verification, honest about what is skipped |
| `docs/` | Audit, architecture, security review, support matrix, proposals, verification report, completion report |
| `APPLY_BATCH_9.md` | Apply, verify, inspect, roll back |
| `manifest.json`, `SHA256SUMS` | Integrity records |

Run `scripts/verify.sh` from this directory. See `docs/BATCH_9_VERIFICATION_REPORT.md` for results.
