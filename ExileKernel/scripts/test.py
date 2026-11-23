#!/usr/bin/env python3
"""Host-side tests for EUK Missions A1, C1 and D1-D3.

These are HOST tests (host rustc, host threads). They do not boot the kernel.
The D3 context-switch cases run the real assembly on the x86-64 host in user
mode; bare-metal behaviour is covered only by scripts/boot_test.py (QEMU).
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

# Mission D1-D3: these modules reference each other through `crate::`, so they
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
}
decls = "".join(
    f'#[path = "{root / path}"]\npub mod {name};\n' for name, path in d3_modules.items()
)

print("== Host tests: D1-D3 (scheduler, stacks, real context switch)", flush=True)
test_root = build_dir / "d3_host_root.rs"
test_root.write_text(decls)
subprocess.run(
    [rustc, "--edition=2024", "--test", str(test_root), "-o", str(build_dir / "d3-tests")],
    check=True,
)
subprocess.run([str(build_dir / "d3-tests")], check=True)

# Compile-time validation only (nothing executes): the modules must build as
# `#![no_std]` with no warnings, i.e. no std/alloc dependency.
print("== no_std compile check: D1-D3", flush=True)
nostd_root = build_dir / "d3_nostd_root.rs"
nostd_root.write_text("#![no_std]\n" + decls)
subprocess.run(
    [
        rustc, "--edition=2024", "--crate-type=lib", "--emit=metadata",
        "-D", "warnings", str(nostd_root), "-o", str(build_dir / "d3_nostd.rmeta"),
    ],
    check=True,
)

# Mission C3: interrupt layer, controllers, ACPI and timers. These modules reference each other
# through `crate::`, so they are compiled under a generated root. Every hardware access goes
# through a trait with a host fake (PortIo, Msr, Mmio, PhysReader); the real implementations are
# compiled but never executed here. The `cfg(test)` build uses a per-thread fake interrupt flag.
c3_modules = {
    "frame": "src/frame.rs",
    "cpu": "src/cpu.rs",
    "irq": "src/irq.rs",
    "pic": "src/pic.rs",
    "acpi": "src/acpi/mod.rs",
    "apic": "src/apic.rs",
    "ioapic": "src/ioapic.rs",
    "timer": "src/timer.rs",
    "pit": "src/pit.rs",
    "hpet": "src/hpet.rs",
}
c3_decls = "".join(
    f'#[path = "{root / path}"]\npub mod {name};\n' for name, path in c3_modules.items()
)

print("== Host tests: C3 (irq, pic, acpi/madt, apic, ioapic, timer, pit, hpet)", flush=True)
c3_root = build_dir / "c3_host_root.rs"
c3_root.write_text(c3_decls)
subprocess.run(
    [rustc, "--edition=2024", "--test", str(c3_root), "-o", str(build_dir / "c3-tests")],
    check=True,
)
subprocess.run([str(build_dir / "c3-tests")], check=True)

print("== no_std compile check: C3 (warnings are errors)", flush=True)
c3_nostd = build_dir / "c3_nostd_root.rs"
c3_nostd.write_text("#![no_std]\n" + c3_decls)
subprocess.run(
    [
        rustc, "--edition=2024", "--crate-type=lib", "--emit=metadata",
        "-D", "warnings", str(c3_nostd), "-o", str(build_dir / "c3_nostd.rmeta"),
    ],
    check=True,
)

print("ALL A1 + C1 + C3 + D1-D3 HOST TESTS PASSED", flush=True)
