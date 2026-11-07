# Applying Batch 9

Nothing here commits, pushes or publishes. Do not push to `main`; do not commit without the repository owner's approval.

## 1. Base commit

`686eecaf6598f34c84b06f783f508ab35f764398` (`origin/main`, contains Batches 2 to 7).

```sh
git fetch origin
git switch --detach 686eecaf6598f34c84b06f783f508ab35f764398
git status --short        # must be empty
```

## 2. Batch 8 dependency

Batch 8 is **not** on `main`. It must exist at `Meta/Experiments/OMNI-Kernel-Batch-8/` before Batch 9 builds. Identity: commit
`4ab10276f316280eec5cf15a5b33688b1ce9b88e`, branch `experiment/omni-kernel-batch-8`, bundle SHA-256
`dbfd15538a95f5f7b2e00c323918a94e02ce63a01a145282728a529e9bb126e2`.

```sh
sha256sum batch8.bundle                                   # compare with the value above
git fetch ./batch8.bundle experiment/omni-kernel-batch-8
git rev-parse FETCH_HEAD                                  # expect 4ab10276f316280eec5cf15a5b33688b1ce9b88e
git checkout FETCH_HEAD -- Meta/Experiments/OMNI-Kernel-Batch-8
```

This stages Batch 8 only. Review it separately; it is an owner decision whether Batch 8 is merged first.

## 3. Files included

Everything under `Meta/Experiments/OMNI-Kernel-Batch-9/` (listed with sizes and hashes in `manifest.json`). All files are **added**; none is
modified or removed. The same documents are mirrored by name for `Documentation/OMNI-Kernel/` (see section 9) but are not placed there.

## 4. Inspect the ZIP

```sh
unzip -t EXILE-OS-OMNI-KERNEL-BATCH-9.zip
unzip -l EXILE-OS-OMNI-KERNEL-BATCH-9.zip           # every path must start with Meta/Experiments/OMNI-Kernel-Batch-9/
unzip -p EXILE-OS-OMNI-KERNEL-BATCH-9.zip Meta/Experiments/OMNI-Kernel-Batch-9/manifest.json | head
```

## 5. Apply the overlay

```sh
unzip EXILE-OS-OMNI-KERNEL-BATCH-9.zip -d .          # from the repository root
git status --short                                   # expect only ?? Meta/Experiments/OMNI-Kernel-Batch-9/ (and Batch 8 from step 2)
```

## 6. Verify

```sh
cd Meta/Experiments/OMNI-Kernel-Batch-9
CARGO_TARGET_DIR=/path/outside/repo/cargo9 ZIG_CACHE_ROOT=/path/outside/repo/zig scripts/verify.sh
```

Needs rustc and cargo, Zig 0.15.2, Python 3, gcc. The script prints SKIPPED for anything it cannot run. The pinned Rust 1.91.1 was
not available when this was produced; results are from 1.97.0 (see the verification report).

## 7. Inspect the final diff

```sh
git diff --stat HEAD -- Kernel ExileKernel Documentation Meta/Experiments/OMNI-Kernel-Batch-{2,3,4,5,6,7}   # must print nothing
git status --short --untracked-files=all | sed -n '1,200p'
(cd Meta/Experiments/OMNI-Kernel-Batch-9 && sha256sum -c SHA256SUMS)
```

## 8. Revert

Nothing was committed, so remove the untracked directory:

```sh
rm -r Meta/Experiments/OMNI-Kernel-Batch-9
```

If the owner later commits it, revert with `git revert <commit>` on a branch. Batch 8's staging (step 2) is reverted with
`git rm -r --cached Meta/Experiments/OMNI-Kernel-Batch-8 && rm -r Meta/Experiments/OMNI-Kernel-Batch-8` if it was only staged for this purpose.

## 9. Protected paths

`Kernel/`, `ExileKernel/`, `Documentation/`, Batches 1 to 8 and the repository build files are untouched. Copying `docs/*.md` into
`Documentation/OMNI-Kernel/` (names: `FINAL_KERNEL_ARCHITECTURE.md`, `BATCH_9_VERIFICATION_REPORT.md`, `BATCH_9_SECURITY_REVIEW.md`,
`OMNI_KERNEL_PROGRAMME_COMPLETION_REPORT.md`, `BATCH_1_TO_8_INTEGRATION_AUDIT.md`) requires the owner's explicit approval and is not done.

## 10. Known limitations

Experimental; hosted tests only; never booted, no QEMU, no hardware; unpinned toolchain; cooperative, uniprocessor, x86-64 only; no
ARM64; capabilities and IPC not in the Rust kernel; open security findings listed in `docs/BATCH_9_SECURITY_REVIEW.md`.
