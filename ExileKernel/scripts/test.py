#!/usr/bin/env python3
"""Host-side tests for EUK Missions A1, C1 and D1-D3.

These are HOST tests (host rustc, host threads). They do not boot the kernel.
The D3 context-switch cases run the real assembly on the x86-64 host in user
mode; bare-metal behaviour is covered only by scripts/boot_test.py (QEMU).
"""

import os
import pathlib
import shutil
import subprocess

root = pathlib.Path(__file__).resolve().parents[1]

def rust_path(path: pathlib.Path) -> str:
    """Return a path literal that the selected host rustc can resolve."""
    resolved = path.resolve()
    cygpath = shutil.which("cygpath")
    if cygpath:
        return subprocess.run(
            [cygpath, "-m", str(resolved)],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
    return resolved.as_posix()

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
    f'#[path = "{rust_path(root / "src" / (module + ".rs"))}"]\npub mod {module};\n'
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
    f'#[path = "{rust_path(root / path)}"]\npub mod {name};\n' for name, path in d3_modules.items()
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
    "smp": "src/smp/mod.rs",
}
c3_decls = "".join(
    f'#[path = "{rust_path(root / path)}"]\npub mod {name};\n' for name, path in c3_modules.items()
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
    f'#[path = "{rust_path(root / path)}"]\npub mod {name};\n'
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


# Mission A4-A5: bounded FDT, CPU topology and page-permission foundations.
# These modules are independently testable and must also compile without std/alloc.
a4a5_modules = ("fdt", "topology", "page_perms")

for module in a4a5_modules:
    source = root / "src" / f"{module}.rs"
    binary = build_dir / f"a4a5-{module}-tests"

    if not source.is_file():
        raise SystemExit(f"Missing A4-A5 module: {source}")

    print(f"== Host tests: A4-A5 {module}.rs", flush=True)
    subprocess.run(
        [rustc, "--edition=2024", "--test", "-D", "warnings",
         str(source), "-o", str(binary)],
        check=True,
    )
    subprocess.run([str(binary)], check=True)

a4a5_decls = "".join(
    f'#[path = "{rust_path(root / "src" / (module + ".rs"))}"]' + chr(10)
    + f"pub mod {module};" + chr(10)
    for module in a4a5_modules
)

print("== no_std compile check: A4-A5 (warnings are errors)", flush=True)
a4a5_nostd = build_dir / "a4a5_nostd_root.rs"
a4a5_nostd.write_text(
    "#![no_std]" + chr(10) + "#![deny(warnings)]" + chr(10) + a4a5_decls,
    encoding="utf-8",
)
subprocess.run(
    [rustc, "--edition=2024", "--crate-type=lib", "--emit=metadata",
     "-D", "warnings", str(a4a5_nostd), "-o", str(build_dir / "a4a5_nostd.rmeta")],
    check=True,
)

print("A4-A5 HOST TESTS AND no_std COMPILE CHECK PASSED", flush=True)

print("ALL A1 + C1 + C3 + D1-D3 + D5 + A4-A5 HOST TESTS AND no_std CHECKS PASSED", flush=True)


# Mission D6: namespaced capability, IPC, transfer, wait, service and driver tests.
# D5 modules remain at crate root; D6 modules are isolated under crate::d6.
d6_modules = (
    "capability", "object", "handle", "ipc", "ipc_message",
    "ipc_endpoint", "ipc_transfer", "ipc_wait", "service", "driver",
)
d6_decl = "pub mod d6 {\n" + "".join(
    f'    #[path = "{rust_path(root / "src/d6" / (name + ".rs"))}"]\n'
    f'    pub mod {name};\n'
    for name in d6_modules
) + "}\n"
d6_decls = d5_decls + d6_decl

print("== Host tests: D6 (namespaced; includes D5 dependencies)", flush=True)
d6_root = build_dir / "d6_host_root.rs"
d6_root.write_text(d6_decls)
subprocess.run(
    [rustc, "--edition=2024", "--test", str(d6_root),
     "-o", str(build_dir / "d6-tests")],
    check=True,
)
subprocess.run(
    [str(build_dir / "d6-tests")],
    check=True,
    env={**os.environ, "RUST_MIN_STACK": "67108864"},
)

print("== no_std compile check: D6 + D5 dependencies", flush=True)
d6_nostd = build_dir / "d6_nostd_root.rs"
d6_nostd.write_text("#![no_std]\n" + d6_decls)
subprocess.run(
    [
        rustc, "--edition=2024", "--crate-type=lib", "--emit=metadata",
        "-D", "warnings", str(d6_nostd),
        "-o", str(build_dir / "d6_nostd.rmeta"),
    ],
    check=True,
)
print("D6 host tests and no_std compilation passed.", flush=True)
