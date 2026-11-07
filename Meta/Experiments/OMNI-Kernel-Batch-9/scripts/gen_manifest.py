#!/usr/bin/env python3
"""Write manifest.json and SHA256SUMS for this package.

Usage: gen_manifest.py [--results RESULTS.json]

RESULTS.json (optional) is a JSON list of {"name", "result", "detail"} records taken from a verify run;
it is copied into manifest.json as "test_results". Without it the field records NOT RECORDED.
manifest.json lists every file except itself and SHA256SUMS; SHA256SUMS lists every file except itself.
"""
import hashlib
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SKIP_DIRS = {"target", ".zig-cache", "zig-out", "__pycache__", ".git"}
SELF = ("manifest.json", "SHA256SUMS")


def sha(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def main():
    results = "NOT RECORDED"
    if "--results" in sys.argv:
        with open(sys.argv[sys.argv.index("--results") + 1], encoding="utf-8") as f:
            results = json.load(f)
    files = []
    for d, dirs, fs in os.walk(ROOT):
        dirs[:] = sorted(x for x in dirs if x not in SKIP_DIRS)
        for name in sorted(fs):
            rel = os.path.relpath(os.path.join(d, name), ROOT).replace(os.sep, "/")
            if rel in SELF:
                continue
            p = os.path.join(d, name)
            files.append({"path": rel, "bytes": os.path.getsize(p), "sha256": sha(p)})
    files.sort(key=lambda e: e["path"])
    manifest = {
        "manifest_schema_version": "1.0",
        "package_name": "OMNI-Kernel-Batch-9",
        "package_version": "0.9.0",
        "overlay_root": "Meta/Experiments/OMNI-Kernel-Batch-9/",
        "repository": "Bryanna-21/EXILE-OS",
        "base_commit": "686eecaf6598f34c84b06f783f508ab35f764398",
        "batch8_dependency": {
            "commit": "4ab10276f316280eec5cf15a5b33688b1ce9b88e",
            "branch": "experiment/omni-kernel-batch-8",
            "bundle_sha256": "dbfd15538a95f5f7b2e00c323918a94e02ce63a01a145282728a529e9bb126e2",
            "merged_into_base": False,
        },
        "checksum_algorithm": "SHA-256",
        "checksum_statement": "Sizes and hashes cover the exact bytes of each listed file. manifest.json and SHA256SUMS do not list themselves; SHA256SUMS also lists manifest.json.",
        "files": files,
        "added_files": [e["path"] for e in files] + list(SELF),
        "modified_files": [],
        "removed_files": [],
        "test_commands": [
            "cd Meta/Experiments/OMNI-Kernel-Batch-9 && scripts/verify.sh",
            "cargo test --offline --locked",
            "cargo test --offline --locked --release",
            "python3 -I scripts/check_abi_conformance.py",
            "python3 -I scripts/check_abi_conformance.py --self-test",
            "cargo test --offline --locked --manifest-path ../OMNI-Kernel-Batch-7/Cargo.toml",
            "cargo test --offline --locked --manifest-path ../OMNI-Kernel-Batch-8/Cargo.toml",
            "(cd ../OMNI-Kernel-Batch-N && zig build test --summary all)   # N = 2..6, zig 0.15.2",
        ],
        "test_results": results,
        "known_limitations": [
            "Experimental; not production-ready.",
            "Hosted tests only: nothing booted, nothing run under QEMU or on hardware (QEMU NOT RUN).",
            "Built with rustc 1.97.0 on the host target, not the pinned 1.91.1 / x86_64-unknown-none.",
            "Batch 8 is not merged into the base commit and must be present as a dependency.",
            "Capabilities and IPC exist only as Zig models; the Rust kernel does not implement them.",
            "Cooperative scheduling, uniprocessor, x86-64 only; no ARM64.",
            "Open findings: SEC-05, SEC-12 (hardware), SEC-13, SEC-14, SEC-17, SEC-18, SEC-19, SEC-20 (docs/BATCH_9_SECURITY_REVIEW.md).",
            "Nothing was committed or pushed.",
        ],
    }
    with open(os.path.join(ROOT, "manifest.json"), "w", encoding="utf-8", newline="\n") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")
    lines = [f"{e['sha256']}  {e['path']}" for e in files]
    lines.append(f"{sha(os.path.join(ROOT, 'manifest.json'))}  manifest.json")
    lines.sort(key=lambda s: s.split("  ", 1)[1])
    with open(os.path.join(ROOT, "SHA256SUMS"), "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines) + "\n")
    print(f"manifest.json: {len(files)} files listed; SHA256SUMS: {len(lines)} lines")


if __name__ == "__main__":
    main()
