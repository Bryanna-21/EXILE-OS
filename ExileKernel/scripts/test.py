#!/usr/bin/env python3
"""Host-side EUK tests and compile-time validation.

These are HOST tests: portable algorithm/model tests compiled with the host
rustc. They do not boot the kernel and do not prove context switching,
interrupts, user mode or SMP behaviour.
"""
import os
import pathlib
import subprocess
import shlex
import tempfile

root = pathlib.Path(__file__).resolve().parents[1]
build_dir = pathlib.Path(
    os.environ.get('EUK_BUILD_DIR', pathlib.Path(tempfile.gettempdir()) / 'euk-build')
)
build_dir.mkdir(parents=True, exist_ok=True)

rustc = os.environ.get('EUK_HOST_RUSTC', 'rustc')
rustc_flags = shlex.split(os.environ.get('EUK_HOST_RUSTC_FLAGS', ''))
exe_suffix = '.exe' if os.name == 'nt' else ''


def run(args):
    print('+', ' '.join(str(a) for a in args), flush=True)
    subprocess.run([str(a) for a in args], check=True)


# Mission A: memory-map parsing.
run([rustc, *rustc_flags, '--edition=2024', '--test', root / 'src/memory.rs', '-o', build_dir / ('memory-tests' + exe_suffix)])
run([build_dir / ('memory-tests' + exe_suffix)])

# Mission D: thread/run-queue/scheduler model. The modules reference each
# other through `crate::`, so they are compiled under a generated crate root.
# `thread`, `runqueue` and `sched` are all declared in the real kernel root
# the same way once they are wired in.
mods = ['thread', 'runqueue', 'sched', 'spin', 'kstack', 'context', 'kthread']
module_paths = {
    'context': root / 'src' / 'arch' / 'x86_64' / 'context.rs',
}
decls = ''.join(
    f'#[path = "{module_paths.get(m, root / "src" / (m + ".rs")).as_posix()}"]\npub mod {m};\n'
    for m in mods
)

test_root = build_dir / 'sched_host_root.rs'
test_root.write_text(decls)
run([rustc, *rustc_flags, '--edition=2024', '--test', test_root, '-o', build_dir / ('d2-tests' + exe_suffix)])
run([build_dir / ('d2-tests' + exe_suffix)])

# Compile-time validation only: the same modules must build as `#![no_std]`
# (no std/alloc dependency). This does not execute anything.
nostd_root = build_dir / 'sched_nostd_root.rs'
nostd_root.write_text('#![no_std]\n' + decls)
run([
    rustc, '--edition=2024', '--crate-type=lib', '--emit=metadata',
    '-D', 'warnings', nostd_root, '-o', build_dir / 'd2_nostd.rmeta',
])
