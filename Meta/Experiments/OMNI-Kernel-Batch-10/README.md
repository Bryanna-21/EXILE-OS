# OMNI Kernel Batch 10 (experimental)

Software fixes for Batch 9 findings, the architecture decision proposal, a toolchain-pin checker, and the programme status.

- `src/txmap.rs` all-or-nothing page mapping and table teardown (SEC-14)
- `src/irqlock.rs` interrupt-masking spin lock (SEC-05)
- `src/abi.rs` ABI version query and register-convention adaptor (SEC-18, SEC-20)
- `scripts/check_toolchain.py` + `toolchain.pin` pinned-toolchain report (SEC-19)
- `docs/` architecture decision (needs owner approval), security status, programme status, verification report

**Hosted tests only.** No QEMU, no hardware, not the pinned Rust. Not production-ready. Depends on `../OMNI-Kernel-Batch-7`, `-8` and (tests) `-9`.
Run `scripts/verify.sh`. Apply and roll back: `APPLY_BATCH_10.md`.
