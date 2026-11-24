#!/usr/bin/env python3
"""Host tests (Mission A / A2). Non-destructive: outputs go under build/.

Stage 1 compiles the original self-contained modules one by one (memory.rs,
bootinfo.rs). Stage 2 compiles the A2 modules into ONE test crate, because they
refer to each other through `crate::`. Stage 3 type-checks the same modules as a
`#![no_std]` library with warnings denied. Host tests never execute privileged
instructions and prove nothing about hardware."""
import os
import pathlib
import subprocess

root = pathlib.Path(__file__).resolve().parents[1]
build_dir = root / 'build'
build_dir.mkdir(exist_ok=True)
rustc = os.environ.get('EUK_HOST_RUSTC', 'rustc')
A2 = ('memory', 'bootinfo', 'range', 'memmap_norm', 'reserve', 'early_alloc', 'phys_handoff')


def run(cmd, **kw):
    print('+', ' '.join(str(c) for c in cmd), flush=True)
    subprocess.run([str(c) for c in cmd], check=True, **kw)


for mod in ('memory', 'bootinfo'):
    print('== host tests: %s.rs' % mod)
    out = build_dir / ('%s-tests' % mod)
    run([rustc, '--edition=2024', '--test', root / 'src' / ('%s.rs' % mod), '-o', out])
    run([out])


def crate_root(name, header):
    path = build_dir / name
    lines = [header] + ['#[path = "%s"] pub mod %s;' % (root / 'src' / (m + '.rs'), m) for m in A2]
    path.write_text('\n'.join(lines) + '\n')
    return path


print('== host tests: combined A2 crate')
combined = crate_root('a2_combined.rs', '#![allow(dead_code)]')
out = build_dir / 'a2-tests'
run([rustc, '--edition=2024', '--test', '-D', 'warnings', combined, '-o', out])
run([out])

print('== no_std check (warnings denied)')
nostd = crate_root('a2_nostd.rs', '#![no_std]\n#![deny(warnings)]')
run([rustc, '--edition=2024', '--crate-type=lib', '--emit=metadata', nostd, '-o', build_dir / 'a2_nostd.rmeta'])

print('ALL HOST TESTS PASSED')
