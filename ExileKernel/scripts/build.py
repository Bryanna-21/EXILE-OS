#!/usr/bin/env python3
"""No downloads or mutable dependency resolution; supply a verified Rust sysroot."""
import argparse, os, pathlib, subprocess
ROOT = pathlib.Path(__file__).resolve().parents[1]
p = argparse.ArgumentParser()
p.add_argument('--output', default=str(ROOT / 'build'))
p.add_argument('--fault', choices=['magic','cpu','map_missing','map_truncated','handoff','map_length','table_dirty'])
p.add_argument('--test-exit', action='store_true')
a = p.parse_args()
out = pathlib.Path(a.output).resolve(); out.mkdir(parents=True, exist_ok=True)
rustc = os.environ.get('EUK_RUSTC', 'rustc')
version = subprocess.check_output([rustc, '--version'], text=True)
if not version.startswith('rustc 1.91.1 '): raise SystemExit('Requires Rust 1.91.1')
sysroot = os.environ.get('EUK_SYSROOT')
flags = ['--sysroot', sysroot] if sysroot else []
def run(args):
    print('+', ' '.join(str(x) for x in args), flush=True)
    subprocess.run([str(x) for x in args], check=True, cwd=ROOT)
defs = (['-DTEST_EXIT'] if a.test_exit else []) + ([f'-DFAULT_{a.fault.upper()}'] if a.fault in ['magic','cpu','table_dirty'] else [])
run([os.environ.get('EUK_CC','gcc'), '-m64','-c','-ffreestanding','-fno-pie', *defs, ROOT/'src/arch/x86_64/entry.S','-o',out/'entry.o'])
cfg = (['--cfg','test_exit'] if a.test_exit else []) + (['--cfg',f'fault_{a.fault}'] if a.fault in ['map_missing','map_truncated','handoff','map_length'] else [])
run([rustc, *flags, '--edition=2024','--target','x86_64-unknown-none','--crate-type','staticlib','-C','panic=abort','-C','opt-level=2','-C','relocation-model=static','-C','no-redzone=yes','-C','debuginfo=0','--remap-path-prefix',f'{ROOT}=EUK', *cfg, ROOT/'src/kernel.rs','-o',out/'kernel.a'])
run([os.environ.get('EUK_LD','ld'), '-nostdlib','--gc-sections','--build-id=none','-z','max-page-size=0x1000','-T',ROOT/'src/arch/x86_64/linker.ld',out/'entry.o',out/'kernel.a','-o',out/'euk.elf'])
run(['python3',ROOT/'scripts/check.py',out/'euk.elf'])
