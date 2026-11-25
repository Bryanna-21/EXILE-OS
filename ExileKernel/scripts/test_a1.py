#!/usr/bin/env python3
"""Focused host tests for the EUK Section A(i) boot foundation.

This complements scripts/test.py without changing the legacy Multiboot boot path.
It compiles the real no_std modules as one host test crate, then executes the
BootInfo, range, normalization, reservation, early-allocation, physical-handoff,
and validated-handoff tests.
"""
import os
import pathlib
import subprocess
import shutil
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
rustc = os.environ.get("EUK_HOST_RUSTC", "rustc")

modules = {
    "bootinfo": "src/bootinfo.rs",
    "memory": "src/memory.rs",
    "range": "src/range.rs",
    "memmap_norm": "src/memmap_norm.rs",
    "reserve": "src/reserve.rs",
    "early_alloc": "src/early_alloc.rs",
    "phys_handoff": "src/phys_handoff.rs",
    "boot_foundation": "src/boot_foundation.rs",
}

with tempfile.TemporaryDirectory(prefix="euk-a1-") as directory:
    directory = pathlib.Path(directory)
    root = directory / "a1_host.rs"
    def rust_source_path(path):
        resolved = str(ROOT / path)
        converter = shutil.which("cygpath")
        if converter:
            resolved = subprocess.check_output(
                [converter, "-m", resolved], text=True
            ).strip()
        return resolved

    declarations = "".join(
        f'#[path = "{rust_source_path(path)}"]\npub mod {name};\n'
        for name, path in modules.items()
    )
    root.write_text("#![allow(dead_code)]\n" + declarations)
    binary = directory / "a1-tests.exe"
    compiler_env = os.environ.copy()
    native_temp = subprocess.check_output(
        ["cygpath", "-w", tempfile.gettempdir()], text=True
    ).strip()
    compiler_env["TMP"] = native_temp
    compiler_env["TEMP"] = native_temp
    compiler_env["TMPDIR"] = native_temp

    subprocess.run(
        [
            rustc,
            "--target", "x86_64-pc-windows-gnu",
            "--edition=2024",
            "--test",
            "-D", "warnings",
            "-C", "linker=x86_64-w64-mingw32-gcc",
            str(root),
            "-o", str(binary),
        ],
        check=True,
        env=compiler_env,
    )
    subprocess.run([str(binary)], check=True)

print("EUK Section A(i) focused host tests passed")
