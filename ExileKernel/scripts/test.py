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

# A2's allocator modules depend on one another through crate:: and must
# be compiled together. Keep the independent C3/D3 modules below separate.
modules = (
    "memory",
    "bootinfo",
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

# Mission A2: these modules share crate-level dependencies and are tested
# together, both as a host test crate and as a warnings-denied no_std library.
a2_modules = (
    "memory", "bootinfo", "range", "memmap_norm",
    "reserve", "early_alloc", "phys_handoff",
)
a2_decls = "".join(
    f'#[path = "{root / "src" / (module + ".rs")}"]\npub mod {module};\n'
    for module in a2_modules
)

print("== Host tests: A2 (memory, ranges, reservations, allocator, handoff)", flush=True)
a2_root = build_dir / "a2_host_root.rs"
a2_root.write_text("#![allow(dead_code)]\n" + a2_decls)
subprocess.run(
    [rustc, "--edition=2024", "--test", "-D", "warnings",
     str(a2_root), "-o", str(build_dir / "a2-tests")],
    check=True,
)
subprocess.run([str(build_dir / "a2-tests")], check=True)

print("== no_std compile check: A2 (warnings are errors)", flush=True)
a2_nostd = build_dir / "a2_nostd_root.rs"
a2_nostd.write_text("#![no_std]\n#![deny(warnings)]\n" + a2_decls)
subprocess.run(
    [rustc, "--edition=2024", "--crate-type=lib", "--emit=metadata",
     str(a2_nostd), "-o", str(build_dir / "a2_nostd.rmeta")],
    check=True,
)

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


# Mission D5: process/address-space/user-mode foundations. This is a separate
# host-test crate so the D5 modules retain their real crate-level names and use
# the actual D1-D3/C3 dependencies. No D5 module is added to kernel.rs here.
d5_modules = {
    # C3 dependencies used by preemption and user-mode support.
    "frame": "src/frame.rs",
    "cpu": "src/cpu.rs",
    "gdt": "src/gdt.rs",
    "irq": "src/irq.rs",
    "timer": "src/timer.rs",
    # D1-D3 scheduler and context-switch dependencies.
    "thread": "src/thread.rs",
    "runqueue": "src/runqueue.rs",
    "sched": "src/sched.rs",
    "spin": "src/spin.rs",
    "kstack": "src/kstack.rs",
    "context": "src/arch/x86_64/context.rs",
    "switch": "src/arch/x86_64/switch.rs",
    "kthread": "src/kthread.rs",
    "dispatch": "src/dispatch.rs",
    # D5 modules; declared at crate root to preserve crate:: references.
    "address_space": "src/d5/address_space.rs",
    "elf": "src/d5/elf.rs",
    "preempt": "src/d5/preempt.rs",
    "process": "src/d5/process.rs",
    "sync": "src/d5/sync.rs",
    "tls": "src/d5/tls.rs",
    "usermode": "src/d5/usermode.rs",
}
d5_decls = "".join(
    f'#[path = "{root / path}"]\npub mod {name};\n'
    for name, path in d5_modules.items()
)

print("== Host tests: D5 (address spaces, ELF, preemption, processes, sync, TLS, user mode)", flush=True)
d5_root = build_dir / "d5_host_root.rs"
d5_root.write_text(d5_decls)
subprocess.run(
    [rustc, "--edition=2024", "--test", str(d5_root), "-o", str(build_dir / "d5-tests")],
    check=True,
)
subprocess.run([str(build_dir / "d5-tests")], check=True)

# Compile-time validation only. This does not activate address spaces, syscall
# entry, or ring-3 execution. Hardware-specific instructions are compiled only.
print("== no_std compile check: D5 (warnings are errors)", flush=True)
d5_nostd = build_dir / "d5_nostd_root.rs"
d5_nostd.write_text("#![no_std]\n" + d5_decls)
subprocess.run(
    [
        rustc, "--edition=2024", "--crate-type=lib", "--emit=metadata",
        "-D", "warnings", str(d5_nostd), "-o", str(build_dir / "d5_nostd.rmeta"),
    ],
    check=True,
)

print("ALL A1 + C1 + C3 + D1-D3 + D5 HOST TESTS PASSED", flush=True)
