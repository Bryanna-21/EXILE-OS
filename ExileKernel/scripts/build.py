#!/usr/bin/env python3
"""No downloads or mutable dependency resolution; supply a verified Rust sysroot."""
import argparse, os, pathlib, subprocess
ROOT = pathlib.Path(__file__).resolve().parents[1]
p = argparse.ArgumentParser()
p.add_argument('--output', default=str(ROOT / 'build'))
p.add_argument('--fault', choices=['magic','cpu','map_missing','map_truncated','handoff','map_length','table_dirty','trap_ud','trap_df','a2_nomem'])
p.add_argument('--test-exit', action='store_true')
p.add_argument('--demo', action='store_true', help='build the D3 on-target scheduler demo (cfg euk_d3_demo)')
p.add_argument('--traps', action='store_true', help='install and self-test the C2 exception subsystem')
p.add_argument('--irq', action='store_true', help='enable the C3 interrupt-controller/timer self-test (implies --traps)')
p.add_argument('--d6', action='store_true', help='compile the D6 capability/IPC/service/driver subsystem and its D5 dependencies into the image (cfg euk_d6); nothing runs at boot')
a = p.parse_args()
out = pathlib.Path(a.output).resolve(); out.mkdir(parents=True, exist_ok=True)
rustc = os.environ.get('EUK_RUSTC', 'rustc')
version = subprocess.check_output([rustc, '--version'], text=True)
# Defaults are the pinned toolchain and target. Both can be overridden ONLY by
# explicit environment variables, and every override is announced, because a
# build made that way is not the pinned reproducible build:
#   EUK_ALLOW_UNPINNED_RUSTC=1  accept a rustc other than 1.91.1
#   EUK_TARGET=host             compile against the host target's `core` (for
#                               machines without the x86_64-unknown-none library);
#                               the image is still freestanding and static.
unpinned = os.environ.get('EUK_ALLOW_UNPINNED_RUSTC') == '1'
if not version.startswith('rustc 1.91.1 '):
    if not unpinned: raise SystemExit('Requires Rust 1.91.1 (or set EUK_ALLOW_UNPINNED_RUSTC=1 for a non-reproducible build)')
    print('WARNING: unpinned rustc:', version.strip(), '- not the verified reproducible toolchain', flush=True)
target = os.environ.get('EUK_TARGET', 'x86_64-unknown-none')
target_flags = []
if target == 'host':
    print('WARNING: EUK_TARGET=host - built against the host target libraries, not x86_64-unknown-none', flush=True)
else:
    target_flags = ['--target', target]
sysroot = os.environ.get('EUK_SYSROOT')
flags = ['--sysroot', sysroot] if sysroot else []
def run(args):
    print('+', ' '.join(str(x) for x in args), flush=True)
    subprocess.run([str(x) for x in args], check=True, cwd=ROOT)
traps = a.traps or a.irq or a.fault in ['trap_ud','trap_df']
defs = (['-DTEST_EXIT'] if a.test_exit else []) + ([f'-DFAULT_{a.fault.upper()}'] if a.fault in ['magic','cpu','table_dirty'] else [])
run([os.environ.get('EUK_CC','gcc'), '-m64','-c','-ffreestanding','-fno-pie', *defs, ROOT/'src/arch/x86_64/entry.S','-o',out/'entry.o'])
objects = [out/'entry.o']
if traps:
    run([os.environ.get('EUK_CC','gcc'), '-m64','-c','-ffreestanding','-fno-pie', ROOT/'src/arch/x86_64/exceptions.S','-o',out/'exceptions.o'])
    objects.append(out/'exceptions.o')
cfg = (['--cfg','test_exit'] if a.test_exit else []) + (['--cfg','euk_d3_demo'] if a.demo else []) + (['--cfg','euk_host_mem'] if target == 'host' else []) + (['--cfg','euk_traps'] if traps else []) + (['--cfg','euk_irq'] if a.irq else []) + (['--cfg','euk_d6'] if a.d6 else []) + (['--cfg',f'fault_{a.fault}'] if a.fault in ['map_missing','map_truncated','handoff','map_length','trap_ud','trap_df','a2_nomem'] else [])
run([rustc, *flags, '--edition=2024',*target_flags,'--crate-type','staticlib','-C','panic=abort','-C','opt-level=2','-C','relocation-model=static','-C','no-redzone=yes','-C','debuginfo=0','--remap-path-prefix',f'{ROOT}=EUK', *cfg, ROOT/'src/kernel.rs','-o',out/'kernel.a'])
run([os.environ.get('EUK_LD','ld'), '-nostdlib','--gc-sections','--build-id=none','-z','max-page-size=0x1000','-T',ROOT/'src/arch/x86_64/linker.ld',*objects,out/'kernel.a','-o',out/'euk.elf'])
run(['python3',ROOT/'scripts/check.py',out/'euk.elf'])
