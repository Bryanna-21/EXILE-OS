#!/usr/bin/env python3
"""Focused host tests for the EUK Section A(i) boot foundation.

This complements scripts/test.py without changing the legacy Multiboot boot path.
It compiles the real no_std modules as one host test crate, then executes the
BootInfo, range, normalization, reservation, and validated-handoff tests.
"""
import os
import pathlib
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
rustc = os.environ.get("EUK_HOST_RUSTC", "rustc")

modules = {
    "bootinfo": "src/bootinfo.rs",
    "memory": "src/memory.rs",
    "range": "src/range.rs",
    "memmap_norm": "src/memmap_norm.rs",
    "boot_foundation": "src/boot_foundation.rs",
}

with tempfile.TemporaryDirectory(prefix="euk-a1-") as directory:
    directory = pathlib.Path(directory)
    root = directory / "a1_host.rs"
    declarations = "".join(
        f'#[path = "{ROOT / path}"]\npub mod {name};\n'
        for name, path in modules.items()
    )
    root.write_text("#![allow(dead_code)]\n" + declarations)
    binary = directory / "a1-tests"
    subprocess.run(
        [rustc, "--edition=2024", "--test", "-D", "warnings", str(root), "-o", str(binary)],
        check=True,
    )
    subprocess.run([str(binary)], check=True)

print("EUK Section A(i) focused host tests passed")
