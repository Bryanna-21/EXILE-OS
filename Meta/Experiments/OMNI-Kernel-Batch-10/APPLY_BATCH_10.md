# Applying Batch 10

Nothing here commits or pushes. Do not push to `main`.

1. Base: `686eecaf6598f34c84b06f783f508ab35f764398`. Batches 8 and 9 must already be present under `Meta/Experiments/` (see `OMNI-Kernel-Batch-9/APPLY_BATCH_9.md`; Batch 8 identity: commit `4ab10276f316280eec5cf15a5b33688b1ce9b88e`).
2. Inspect: `unzip -t EXILE-OS-OMNI-KERNEL-BATCH-10.zip` and `unzip -l ...` (every path starts with `Meta/Experiments/OMNI-Kernel-Batch-10/`).
3. Apply from the repository root: `unzip EXILE-OS-OMNI-KERNEL-BATCH-10.zip -d .`
4. Verify: `cd Meta/Experiments/OMNI-Kernel-Batch-10 && CARGO_TARGET_DIR=/path/outside/repo/c10 scripts/verify.sh`
5. Check integrity: `sha256sum -c SHA256SUMS`
6. Roll back: `rm -r Meta/Experiments/OMNI-Kernel-Batch-10` (nothing else was modified).
7. Protected paths are untouched. The architecture decision in `docs/ARCHITECTURE_DECISION.md` needs the owner's approval before any change under `Kernel/` or `ExileKernel/`.
