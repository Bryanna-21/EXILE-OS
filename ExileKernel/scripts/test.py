#!/usr/bin/env python3
"""Host tests v4 (Mission A). Compiles each self-contained module with
rustc --test: memory.rs (original 18), bootinfo.rs, memmap_norm.rs,
early_alloc.rs. Non-destructive: outputs go under build/, harness-owned."""
import os
import pathlib
import subprocess

root = pathlib.Path(__file__).resolve().parents[1]
build_dir = root / 'build'
build_dir.mkdir(exist_ok=True)

rustc = os.environ.get('EUK_HOST_RUSTC', 'rustc')


def run(cmd, **kw):
    print('+', ' '.join(str(c) for c in cmd), flush=True)
    subprocess.run([str(c) for c in cmd], check=True, **kw)


for mod in ('memory', 'bootinfo', 'memmap_norm', 'early_alloc'):
    src = root / 'src' / ('%s.rs' % mod)
    out = build_dir / ('%s-tests' % mod)
    print('== host tests: %s.rs' % mod)
    run([rustc, '--edition=2024', '--test', src, '-o', out])
    run([out])

print('ALL HOST TESTS PASSED')
