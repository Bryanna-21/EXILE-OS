#!/usr/bin/env python3
"""Build the delivery archive EXILE-OS-KERNEL-COMPLETION-BATCH.zip.

usage: build_package.py REPO_ROOT OUT_DIR [--results RESULTS.json]

Stages the files this batch adds (repo-relative paths preserved), writes README.md, APPLY_BATCH.md, verify_package.sh, manifest.json and
SHA256SUMS at the archive root, then writes a deterministic ZIP. Nothing is read from caches or build directories.
"""
import hashlib, json, os, sys, shutil, zipfile

TOP = "EXILE-OS-KERNEL-COMPLETION-BATCH"
PKG = "Meta/Experiments/OMNI-Kernel-Batch-11"
EXTRA = [".github/workflows/omni-kernel.yml"]
SKIP = {"target", ".zig-cache", "zig-out", "__pycache__", "build", ".git"}
SUBSYSTEM = [
    ("src/caps.rs", "capability security"), ("src/ipc.rs", "IPC"), ("src/sys.rs", "syscalls and ABI"), ("src/status.rs", "syscalls and ABI"),
    ("src/lib.rs", "build"), ("Cargo", "build"), ("tests/caps_unit.rs", "capability security tests"), ("tests/ipc_e2e.rs", "IPC tests"),
    ("arch/aarch64-virt", "boot (ARM64)"), ("scripts/qemu", "boot tests"), ("scripts/build_aarch64", "boot (ARM64)"),
    ("scripts/check_ipc", "ABI conformance"), ("scripts/kernel_syntax", "kernel static check"), ("scripts/verify", "test infrastructure"), ("scripts/build_package", "packaging"),
    ("docs/THREAT", "kernel security"), ("docs/IPC", "IPC"), ("docs/ARCH", "architecture"), ("docs/GAP", "programme"),
    ("docs/COMPONENT", "programme"), ("docs/USERLAND", "userland"), ("docs/SUPPORT", "programme"), ("docs/BUILD", "build"),
    ("docs/VERIF", "test infrastructure"), (".github", "CI"), ("README", "documentation"), ("APPLY", "documentation"),
]
DEPS = {
    "src/": ["OMNI-Kernel-Batch-7 (sched, syscall, uaccess)", "OMNI-Kernel-Batch-10 (abi)"],
    "tests/": ["OMNI-Kernel-Batch-8 and -9 (test fixtures)"],
    "scripts/check_ipc": ["OMNI-Kernel-Batch-2 and -5 Zig sources"],
    "scripts/qemu_x86": ["OMNI-Kernel-Batch-8/scripts/qemu_test.py"],
}
NOTES = {
    "src/sys.rs": "Entry point IpcKernel::syscall; wrap the Batch 7 Kernel and its UserBacking. Call cleanup_space/terminate_space when a space dies.",
    "src/ipc.rs": "Lock order: IPC state lock before scheduler lock.",
    "arch/aarch64-virt/boot.zig": "Source only; never assembled or booted.",
    ".github/workflows/omni-kernel.yml": "New workflow; YAML syntax checked only, never run.",
}


def sha(b):
    return hashlib.sha256(b).hexdigest()


def classify(rel):
    key = rel[len(PKG) + 1:] if rel.startswith(PKG + "/") else rel
    sub = next((s for p, s in SUBSYSTEM if key.startswith(p)), "documentation")
    deps = next((d for p, d in DEPS.items() if key.startswith(p)), [])
    return sub, deps, NOTES.get(key, NOTES.get(rel, ""))


README = """# EXILE-OS kernel completion batch (OMNI Batch 11)

Additive overlay for `Bryanna-21/EXILE-OS` at `origin/main` 92ab9921. It adds Meta/Experiments/OMNI-Kernel-Batch-11/ and
.github/workflows/omni-kernel.yml. No existing file is modified or removed; `Kernel/` and `ExileKernel/` are untouched.

What it contains: enforced capabilities and channel IPC for the Rust OMNI kernel (syscalls 0x100..0x107), 18 hosted tests, an IPC conformance
checker against the Zig models, the evidence-based gap matrix, the proposed hybrid x86-64/ARM64 architecture and migration plan, a threat model and
security review, component disposition, userland notes, a support matrix, boot-test scripts (NOT RUN), a source-only ARM64 stub (not assembled), a host-clang static syntax check of `Kernel/` (x86-64 and AArch64; not a build), and a CI workflow.

What it is not: no boot evidence exists (no QEMU in the authoring environment), `Kernel/` was not built, the pinned toolchain was unavailable (rustc 1.97.0
on the host target was used), SMP, drivers, filesystems and networking are not implemented in the Rust experiments, and nothing is production-ready.
Read docs/SUPPORT_MATRIX.md and docs/VERIFICATION_REPORT.md first. Nothing was committed or pushed.

Files: manifest.json (paths, change types, SHA-256, subsystem, dependencies, notes), SHA256SUMS, APPLY_BATCH.md, verify_package.sh.
"""
APPLY = """# Applying the batch

1. Check the archive: `bash verify_package.sh` (verifies SHA256SUMS and every manifest hash; needs only bash, python3).
2. Start from a clean checkout of `origin/main` 92ab9921 (Batches 8 to 10 must already be present).
3. Copy the contents of `Meta/` and `.github/` from this directory into the repository root, e.g. `cp -a Meta .github /path/to/repo/`.
4. `cd Meta/Experiments/OMNI-Kernel-Batch-11 && scripts/verify.sh` (see docs/BUILD_AND_TOOLCHAIN.md; set OMNI_BASE_REF to the base commit to check scope).
5. Review and push on a branch yourself. Do not push to `main`.

To undo: delete the two added paths. Nothing else changed.
"""
VERIFY = r"""#!/usr/bin/env bash
# Verify the archive contents against SHA256SUMS and manifest.json. Exit 0 ok, 1 mismatch.
set -u
cd "$(dirname "${BASH_SOURCE[0]}")" || exit 1
python3 -I - <<'PY'
import hashlib, json, os, sys
m = json.load(open("manifest.json", encoding="utf-8"))
bad = 0
listed = {e["path"] for e in m["files"]}
for e in m["files"]:
    if not os.path.isfile(e["path"]): print("missing:", e["path"]); bad += 1; continue
    d = open(e["path"], "rb").read()
    if hashlib.sha256(d).hexdigest() != e["sha256"] or len(d) != e["bytes"]: print("mismatch:", e["path"]); bad += 1
actual = {os.path.relpath(os.path.join(a, f), ".").replace(os.sep, "/") for a, _, fs in os.walk(".") for f in fs}
for x in sorted(actual - listed - {"manifest.json", "SHA256SUMS"}): print("unlisted:", x); bad += 1
sums = {}
for line in open("SHA256SUMS", encoding="utf-8"):
    h, _, p = line.rstrip("\n").partition("  "); sums[p] = h
want = {e["path"]: e["sha256"] for e in m["files"]}
want["manifest.json"] = hashlib.sha256(open("manifest.json", "rb").read()).hexdigest()
if sums != want: print("SHA256SUMS differs from manifest"); bad += 1
print("verify_package: %s (%d files)" % ("FAILED" if bad else "OK", len(m["files"])))
sys.exit(1 if bad else 0)
PY
"""


def main():
    a = [x for x in sys.argv[1:] if not x.startswith("--")]
    repo, out = a[0], a[1]
    results = "NOT RECORDED"
    if "--results" in sys.argv:
        results = json.load(open(sys.argv[sys.argv.index("--results") + 1], encoding="utf-8"))
    files = {}
    for d, dirs, fs in os.walk(os.path.join(repo, PKG)):
        dirs[:] = sorted(x for x in dirs if x not in SKIP)
        for f in fs:
            p = os.path.join(d, f)
            files[os.path.relpath(p, repo).replace(os.sep, "/")] = open(p, "rb").read()
    for e in EXTRA:
        files[e] = open(os.path.join(repo, e), "rb").read()
    files["README.md"] = README.encode()
    files["APPLY_BATCH.md"] = APPLY.encode()
    files["verify_package.sh"] = VERIFY.encode()
    entries = []
    for rel in sorted(files):
        sub, deps, note = classify(rel)
        entries.append({"path": rel, "change_type": "added", "bytes": len(files[rel]), "sha256": sha(files[rel]),
                        "subsystem": sub, "dependencies": deps, "integration_notes": note})
    manifest = {
        "manifest_schema_version": "1.0", "package_name": TOP, "package_version": "0.11.0",
        "repository": "Bryanna-21/EXILE-OS", "base_commit": "92ab9921c9fff0f2fd457c54050c692bbcf32bf8",
        "checksum_algorithm": "SHA-256",
        "checksum_statement": "Hashes cover the exact bytes of each listed file; manifest.json and SHA256SUMS do not list themselves; SHA256SUMS also lists manifest.json.",
        "files": entries,
        "added_files": [e["path"] for e in entries], "modified_files": [], "removed_files": [],
        "apply_root_entries": ["Meta/", ".github/"],
        "test_commands": ["cd Meta/Experiments/OMNI-Kernel-Batch-11 && scripts/verify.sh",
                          "python3 -I scripts/check_ipc_conformance.py [--self-test]"],
        "test_results": results,
        "known_limitations": [
            "Experimental; not production-ready.", "No boot evidence: QEMU NOT RUN.",
            "rustc 1.97.0 on the host target was used, not the pinned 1.91.1 / x86_64-unknown-none.",
            "Kernel/ was not built, run or modified.", "ARM64 stub is source only and was not assembled.", "Kernel/ static syntax check is host clang 18, not the project toolchain.",
            "SMP, drivers, filesystems, networking not implemented in the Rust experiments.",
            "Cancellation objects and blocking send not implemented.", "Nothing was committed or pushed."],
    }
    mj = (json.dumps(manifest, indent=2) + "\n").encode()
    sums = sorted([f"{e['sha256']}  {e['path']}" for e in entries] + [f"{sha(mj)}  manifest.json"], key=lambda s: s.split("  ", 1)[1])
    files["manifest.json"] = mj
    files["SHA256SUMS"] = ("\n".join(sums) + "\n").encode()
    os.makedirs(out, exist_ok=True)
    zp = os.path.join(out, TOP + ".zip")
    if os.path.exists(zp):
        os.remove(zp)
    with zipfile.ZipFile(zp, "w", zipfile.ZIP_DEFLATED) as z:
        for rel in sorted(files):
            zi = zipfile.ZipInfo(f"{TOP}/{rel}", date_time=(2026, 11, 8, 0, 0, 0))
            zi.compress_type = zipfile.ZIP_DEFLATED
            zi.external_attr = (0o755 if rel.endswith((".sh", ".py")) else 0o644) << 16
            z.writestr(zi, files[rel])
    print(zp, os.path.getsize(zp), sha(open(zp, "rb").read()))


if __name__ == "__main__":
    main()
