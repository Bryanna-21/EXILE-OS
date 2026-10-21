#!/usr/bin/env python3
import os
import pathlib
import subprocess

root = pathlib.Path(__file__).resolve().parents[1]
build_dir = root / 'build'
build_dir.mkdir(exist_ok=True)

rustc = os.environ.get('EUK_HOST_RUSTC', 'rustc')
subprocess.run(
    [
        rustc,
        '--edition=2024',
        '--test',
        root / 'src/memory.rs',
        '-o',
        build_dir / 'memory-tests',
    ],
    check=True,
)
subprocess.run([build_dir / 'memory-tests'], check=True)
