#!/usr/bin/env python3
"""Host-side tests for EUK Missions A1, C1 and D1-D4.

These are HOST tests (host rustc, host threads). They do not boot the kernel.
The context-switch cases run the real assembly on the x86-64 host in user mode;
the IRQ-lock tests use a model of RFLAGS.IF, and the trap stubs are assembled
but never executed on the host. Bare-metal behaviour (real interrupts, the PIT,
preemption) is covered only by scripts/boot_test.py (QEMU).
"""

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

# Mission D1-D4: these modules reference each other through `crate::`, so they
# are compiled under a generated crate root that declares them the way the kernel
# root does. `context` and `switch` are the x86-64 architecture modules. The
# `real_*` and `switch::tests` cases EXECUTE the real context-switch assembly on
# the x86-64 host (user mode, host stacks); the rest are model tests.
d3_modules = {
    "thread": "src/thread.rs",
    "runqueue": "src/runqueue.rs",
    "sched": "src/sched.rs",
    "spin": "src/spin.rs",
    "kstack": "src/kstack.rs",
    "context": "src/arch/x86_64/context.rs",
    "switch": "src/arch/x86_64/switch.rs",
    "kthread": "src/kthread.rs",
    "dispatch": "src/dispatch.rs",
    # Mission D4: interrupt layer, timer, preemption.
    "gdt": "src/gdt.rs",
    "idt": "src/idt.rs",
    "irq": "src/irq.rs",
    "preempt": "src/preempt.rs",
    "timer": "src/timer.rs",
    "pic": "src/arch/x86_64/pic.rs",
    "trap": "src/arch/x86_64/trap.rs",
}
def rust_source_path(path):
    value = str(path)
    # Convert MSYS paths such as /c/Users/... to Windows drive paths.
    if len(value) >= 3 and value[0] == "/" and value[1].isalpha() and value[2] == "/":
        value = value[1].upper() + ":" + value[2:]
    return value.replace("\\\\", "/")

decls = "".join(
    f'#[path = "{rust_source_path(root / path)}"]\npub mod {name};\n'
    for name, path in d3_modules.items()
)

print("== Host tests: D1-D4 (scheduler, stacks, context switch, interrupts, timer, preemption)", flush=True)
test_root = build_dir / "d3_host_root.rs"
test_root.write_text(decls)
subprocess.run(
    [rustc, "--edition=2024", "--test", str(test_root), "-o", str(build_dir / "d3-tests")],
    check=True,
)
subprocess.run([str(build_dir / "d3-tests")], check=True)

# Compile-time validation only (nothing executes): the modules must build as
# `#![no_std]` with no warnings, i.e. no std/alloc dependency.
print("== no_std compile check: D1-D4", flush=True)
nostd_root = build_dir / "d3_nostd_root.rs"
nostd_root.write_text("#![no_std]\n" + decls)
subprocess.run(
    [
        rustc, "--edition=2024", "--crate-type=lib", "--emit=metadata",
        "-D", "warnings", str(nostd_root), "-o", str(build_dir / "d3_nostd.rmeta"),
    ],
    check=True,
)

print("ALL A1 + C1 + D1-D4 HOST TESTS PASSED", flush=True)
