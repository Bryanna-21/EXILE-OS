#!/usr/bin/env python3
"""Host-side tests for EUK Missions A1 and C1."""

import os
import pathlib
import subprocess

root = pathlib.Path(__file__).resolve().parents[1]
build_dir = pathlib.Path(os.environ.get("EUK_TEST_BUILD", root / "build"))
build_dir.mkdir(parents=True, exist_ok=True)

rustc = os.environ.get("EUK_HOST_RUSTC", "rustc")

modules = (
    "memory",
    "bootinfo",
    "memmap_norm",
    "early_alloc",
    "cpu",
    "gdt",
    "idt",
)

for module in modules:
    source = root / "src" / f"{module}.rs"
    binary = build_dir / f"{module}-tests"

    if not source.is_file():
        raise SystemExit(f"Missing test module: {source}")

    print(f"== Host tests: {module}.rs", flush=True)
    subprocess.run(
        [rustc, "--edition=2024", "--test", str(source), "-o", str(binary)],
        check=True,
    )
    subprocess.run([str(binary)], check=True)

print("ALL A1 + C1 HOST TESTS PASSED", flush=True)
