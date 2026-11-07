#!/usr/bin/env python3
"""Compare the installed toolchain with toolchain.pin and say plainly what is blocked.

Exit status: 0 everything pinned and present; 3 at least one pin mismatched or tool missing
(the caller decides whether that is fatal); 2 the pin file is unreadable.
Nothing is installed or changed, and the pinned version is never replaced silently.
"""
import os
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def run(cmd):
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
        return (r.stdout + r.stderr).strip()
    except (OSError, subprocess.TimeoutExpired):
        return None


def main():
    pins = {}
    need = []
    try:
        with open(os.path.join(HERE, "toolchain.pin"), encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line or line.startswith("#"):
                    continue
                k, _, v = line.partition("=")
                if k == "need_tool":
                    need.append(v)
                else:
                    pins[k] = v
    except OSError as e:
        print("cannot read toolchain.pin:", e)
        return 2
    bad = 0

    out = run(["rustc", "--version"]) if shutil.which("rustc") else None
    got = out.split()[1] if out else None
    if got == pins["rustc"]:
        print(f"OK       rustc {got} (pinned)")
    else:
        bad += 1
        print(f"MISMATCH rustc is {got or 'missing'}, pinned {pins['rustc']}: results are NOT evidence for the pin")

    sysroot = run(["rustc", "--print", "sysroot"]) if got else None
    have_target = bool(sysroot) and os.path.isdir(os.path.join(sysroot, "lib", "rustlib", pins["rust_target"]))
    if have_target:
        print(f"OK       target {pins['rust_target']} installed")
    else:
        bad += 1
        print(f"MISSING  target {pins['rust_target']}: bare-metal build is BLOCKED")

    zig = os.environ.get("ZIG", "zig")
    zv = run([zig, "version"]) if shutil.which(zig) or os.path.isfile(zig) else None
    if zv == pins["zig"]:
        print(f"OK       zig {zv}")
    else:
        bad += 1
        print(f"MISMATCH zig is {zv or 'missing'}, pinned {pins['zig']}")

    for t in need:
        if shutil.which(t):
            print(f"OK       {t} present")
        else:
            bad += 1
            print(f"MISSING  {t}: runtime validation that needs it is BLOCKED")

    print("RESULT: toolchain matches the pin" if not bad else f"RESULT: {bad} pin or tool problem(s)")
    return 0 if not bad else 3


if __name__ == "__main__":
    sys.exit(main())
