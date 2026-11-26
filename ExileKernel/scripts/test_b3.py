#!/usr/bin/env python3
"""Host-side tests for EUK Mission B3 (capabilities and bounded IPC).

HOST tests only: host rustc, host threads, the real D1-D3 scheduler model and the real D5 user-mode
syscall types. Nothing here boots the kernel, runs under QEMU or touches hardware.

Steps
  1. Build and run the B3 unit and scheduler-driven integration tests under a generated crate root
     (same #[path] pattern as scripts/test.py). Every B3 module must contribute at least one test.
  2. Compile the same modules as `#![no_std]` with `-D warnings` (metadata only).
  3. Check that the B3 sources contain no `unsafe` code.
  4. If rustfmt is installed, check formatting of the B3 files only (skip_children, so modules
     reached through #[path] are never reformatted or compared).

This script is separate from scripts/test.py on purpose: B3 modules are not part of the kernel
image and are not declared in kernel.rs. Run it with `python3 scripts/test_b3.py`.
"""

import os
import pathlib
import re
import shutil
import subprocess

root = pathlib.Path(__file__).resolve().parents[1]
build_dir = pathlib.Path(os.environ.get("EUK_TEST_BUILD", root / "build")) / "b3"
build_dir.mkdir(parents=True, exist_ok=True)
rustc = os.environ.get("EUK_HOST_RUSTC", "rustc")

# Existing EUK modules B3 builds on (declared at crate root so their `crate::` paths resolve).
deps = {
    "frame": "src/frame.rs",
    "cpu": "src/cpu.rs",
    "gdt": "src/gdt.rs",
    "thread": "src/thread.rs",
    "runqueue": "src/runqueue.rs",
    "sched": "src/sched.rs",
    "usermode": "src/d5/usermode.rs",
}
# B3 modules in dependency order. Kept at crate root as well: the B3 sources refer to each other as
# `crate::cap_rights`, `crate::ipc`, ... so integrating them only needs the same flat declarations.
b3 = {
    "cap_rights": "src/b3/cap_rights.rs",
    "kobject": "src/b3/kobject.rs",
    "cap_space": "src/b3/cap_space.rs",
    "ipc_message": "src/b3/ipc_message.rs",
    "ipc_endpoint": "src/b3/ipc_endpoint.rs",
    "ipc_notify": "src/b3/ipc_notify.rs",
    "ipc": "src/b3/ipc.rs",
    "ipc_abi": "src/b3/ipc_abi.rs",
}

for rel in list(deps.values()) + list(b3.values()):
    if not (root / rel).is_file():
        raise SystemExit(f"Missing source: {root / rel}")

decls = "".join(
    f'#[path = "{root / path}"]\npub mod {name};\n'
    for name, path in {**deps, **b3}.items()
)

print("== Host tests: B3 (capabilities, object table, messages, endpoints, notifications, IPC, ABI)", flush=True)
test_root = build_dir / "b3_host_root.rs"
test_root.write_text(decls)
binary = build_dir / "b3-tests"
subprocess.run(
    [rustc, "--edition=2024", "--test", str(test_root), "-o", str(binary)],
    check=True,
)

total = 0
for name in b3:
    out = subprocess.run(
        [str(binary), f"{name}::tests::", "--test-threads=4"],
        check=True, capture_output=True, text=True,
    ).stdout
    m = re.search(r"test result: ok\. (\d+) passed; 0 failed", out)
    if not m or int(m.group(1)) == 0:
        raise SystemExit(f"B3 module {name} ran no passing tests:\n{out}")
    print(f"   {name}: {m.group(1)} tests passed", flush=True)
    total += int(m.group(1))
print(f"   B3 total: {total} tests passed", flush=True)

print("== no_std compile check: B3 (warnings are errors)", flush=True)
nostd_root = build_dir / "b3_nostd_root.rs"
nostd_root.write_text("#![no_std]\n" + decls)
subprocess.run(
    [
        rustc, "--edition=2024", "--crate-type=lib", "--emit=metadata",
        "-D", "warnings", str(nostd_root), "-o", str(build_dir / "b3_nostd.rmeta"),
    ],
    check=True,
)

print("== B3 sources contain no unsafe code", flush=True)
for name, rel in b3.items():
    text = (root / rel).read_text()
    code = "\n".join(
        line for line in text.splitlines() if not line.lstrip().startswith("//")
    )
    if re.search(r"\bunsafe\b", code):
        raise SystemExit(f"{rel}: contains `unsafe`")

if shutil.which("rustfmt"):
    print("== rustfmt --check: B3 files only", flush=True)
    for rel in b3.values():
        subprocess.run(
            ["rustfmt", "--edition=2024", "--check", "--config", "skip_children=true",
             str(root / rel)],
            check=True,
        )
else:
    print("== rustfmt not installed: formatting not checked", flush=True)

print(f"B3 HOST TESTS PASSED ({total} tests)", flush=True)
