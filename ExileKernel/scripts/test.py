#!/usr/bin/env python3
import os
import pathlib
import subprocess

root = pathlib.Path(__file__).resolve().parents[1]
build_dir = root / 'build'
build_dir.mkdir(exist_ok=True)

rustc = os.environ.get('EUK_HOST_RUSTC', 'rustc')
for source, output in [
    (root / 'src/memory.rs', build_dir / 'memory-tests'),
    (root / 'src/platform.rs', build_dir / 'platform-tests'),
]:
    subprocess.run(
        [rustc, '--edition=2024', '--test', source, '-o', output],
        check=True,
    )
    subprocess.run([output], check=True)
