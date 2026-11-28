#!/usr/bin/env python3
"""Host-side integration tests for EUK Mission B4 (kernel IPC integration).

HOST tests only: host rustc, host threads, the real D1-D3 scheduler (including REAL context
switches on host stacks, the technique `dispatch::real_tests` uses), the real D5 process table,
address-space code and syscall types, and the real B3 capability/IPC code driven through the B4
integration modules. Nothing here boots the kernel, runs under QEMU or touches hardware.

Steps
  1. Build the whole tree the way the kernel declares it (D5 modules and `b3` at crate root, the
     `b3` children below it) and run every B3 and B4 test. Every module must contribute at least
     one passing test, and a list of REQUIRED tests (one or more per mission requirement) must
     exist and pass, so a silently deleted test fails the run.
  2. Compile the same tree as `#![no_std]` with `-D warnings` (metadata only).
  3. `unsafe` audit: none in any `b3` source except `scheduler_bridge.rs`, where only the two
     documented sites (`DispatchPort::new` and its single `dispatch::reschedule` call, plus the
     test-only host-stack code under `#[cfg(test)]`) are allowed.
  4. rustfmt --check on every changed or new Rust file (skip_children).
  5. Link check: the kernel image (which now declares `b3`) still builds and passes
     scripts/check.py against the HOST target. This is NOT a boot test.

Run with `python3 scripts/test_b4.py` (EUK_TEST_BUILD overrides the build directory).
"""

import os
import pathlib
import re
import shutil
import subprocess

root = pathlib.Path(__file__).resolve().parents[1]
build_dir = pathlib.Path(os.environ.get("EUK_TEST_BUILD", root / "build")) / "b4"
build_dir.mkdir(parents=True, exist_ok=True)
rustc = os.environ.get("EUK_HOST_RUSTC", "rustc")

deps = {
    "frame": "src/frame.rs",
    "cpu": "src/cpu.rs",
    "gdt": "src/gdt.rs",
    "irq": "src/irq.rs",
    "timer": "src/timer.rs",
    "thread": "src/thread.rs",
    "runqueue": "src/runqueue.rs",
    "sched": "src/sched.rs",
    "spin": "src/spin.rs",
    "kstack": "src/kstack.rs",
    "context": "src/arch/x86_64/context.rs",
    "switch": "src/arch/x86_64/switch.rs",
    "kthread": "src/kthread.rs",
    "dispatch": "src/dispatch.rs",
    "address_space": "src/d5/address_space.rs",
    "elf": "src/d5/elf.rs",
    "preempt": "src/d5/preempt.rs",
    "process": "src/d5/process.rs",
    "sync": "src/d5/sync.rs",
    "tls": "src/d5/tls.rs",
    "usermode": "src/d5/usermode.rs",
}
b3_root = "src/b3/mod.rs"
# B3 modules (unchanged semantics; one `super::` edit and two additive methods) and B4 modules.
b3_old = ["cap_rights", "kobject", "cap_space", "ipc_message", "ipc_endpoint", "ipc_notify",
          "ipc", "ipc_abi"]
b4_new = ["kernel_domain", "scheduler_bridge", "resume", "user_memory", "ipc_service",
          "syscall_dispatch", "thread_lifecycle", "process_lifecycle", "initialization"]
b3_files = [b3_root] + [f"src/b3/{n}.rs" for n in b3_old + b4_new]
changed_outside_b3 = ["src/kernel.rs", "src/d5/usermode.rs"]

for rel in list(deps.values()) + b3_files + changed_outside_b3:
    if not (root / rel).is_file():
        raise SystemExit(f"Missing source: {root / rel}")

decls = "".join(f'#[path = "{root / p}"]\npub mod {n};\n' for n, p in deps.items())
decls += f'#[path = "{root / b3_root}"]\npub mod b3;\n'

# --------------------------------------------------------------------------------------------
print("== Host tests: B3 + B4 (real scheduler, real context switches, real process table)", flush=True)
test_root = build_dir / "b4_host_root.rs"
test_root.write_text(decls)
binary = build_dir / "b4-tests"
subprocess.run([rustc, "--edition=2024", "--test", str(test_root), "-o", str(binary)], check=True)


def run(filter_, threads=4):
    return subprocess.run(
        [str(binary), filter_, f"--test-threads={threads}"],
        check=True, capture_output=True, text=True,
    ).stdout


def count(out):
    m = re.search(r"test result: ok\. (\d+) passed; 0 failed", out)
    return int(m.group(1)) if m else 0


total_b3 = 0
for name in b3_old:
    n = count(run(f"b3::{name}::tests::"))
    if n == 0:
        raise SystemExit(f"B3 module {name} ran no passing tests")
    total_b3 += n
print(f"   B3 modules (8): {total_b3} tests passed", flush=True)

total_b4 = 0
for name in b4_new:
    n = count(run(f"b3::{name}::"))
    if n == 0:
        raise SystemExit(f"B4 module {name} ran no passing tests")
    print(f"   {name}: {n} tests passed", flush=True)
    total_b4 += n
print(f"   B4 modules (9): {total_b4} tests passed", flush=True)

# Everything in the b3 tree at once, in the default parallel mode, to catch cross-test interference.
all_out = subprocess.run([str(binary), "b3::"], check=True, capture_output=True, text=True).stdout
all_n = count(all_out)
if all_n != total_b3 + total_b4:
    raise SystemExit(f"b3:: ran {all_n} tests, per-module total is {total_b3 + total_b4}")
print(f"   whole b3 tree, parallel: {all_n} tests passed", flush=True)

# D5 must still pass unchanged (usermode.rs gained `apply_outcome`; D5 behaviour is the same).
d5_total = 0
for name in ["usermode", "process", "address_space", "sync", "preempt", "tls", "elf"]:
    out = run(f"{name}::tests::")
    n = count(out)
    if n == 0:
        raise SystemExit(f"D5 module {name} ran no passing tests")
    d5_total += n
print(f"   D5 modules re-run against the edited usermode.rs: {d5_total} tests passed", flush=True)

# Requirement -> tests that must exist (and, because the whole tree passed, pass).
required = {
    "dispatch routing, valid and invalid numbers": [
        "b3::syscall_dispatch::tests::numbers_outside_the_ipc_range_are_d5s_business_and_stay_unchanged",
        "b3::syscall_dispatch::tests::handle_frame_sets_rax_and_returns_by_d5s_rules",
    ],
    "domain init / shutdown / uninitialised refusal": [
        "b3::kernel_domain::tests::install_activates_once_and_never_replaces_live_state",
        "b3::initialization::tests::init_checks_the_scheduler_then_memory_and_installs_last",
        "b3::initialization::tests::shutdown_wakes_blocked_threads_and_allows_a_clean_reinit",
        "b3::syscall_dispatch::tests::an_uninitialised_or_shut_down_subsystem_answers_enosys_and_touches_nothing",
    ],
    "service ownership": [
        "b3::ipc_service::tests::handles_never_cross_process_boundaries",
        "b3::ipc_service::tests::a_service_bound_to_a_thread_that_is_not_running_touches_nothing",
        "b3::syscall_dispatch::tests::handles_of_another_process_and_malformed_arguments_are_rejected",
    ],
    "user-pointer validation and copy failures": [
        "b3::user_memory::tests::kernel_null_wrapping_and_out_of_range_addresses_are_faults",
        "b3::user_memory::tests::permissions_are_enforced_per_direction",
        "b3::user_memory::tests::a_physical_write_error_is_a_fault",
        "b3::syscall_dispatch::tests::bad_user_pointers_fault_before_any_state_changes",
    ],
    "blocking, resumption, direct hand-off": [
        "b3::syscall_dispatch::real_switch::a_blocked_receive_really_yields_and_is_completed_by_a_direct_handoff",
        "b3::scheduler_bridge::tests::real_switch::blocked_thread_really_yields_and_resumes_after_a_wake",
        "b3::resume::tests::blocked_receive_is_completed_into_the_original_buffer",
    ],
    "queued delivery and FIFO / priority": [
        "b3::syscall_dispatch::real_switch::a_sender_blocked_on_a_full_queue_resumes_and_queue_order_is_send_order",
        "b3::syscall_dispatch::real_switch::waiters_are_served_by_priority_then_fifo",
    ],
    "capability transfer and attenuation": [
        "b3::syscall_dispatch::real_switch::a_capability_travels_through_a_blocked_receive_attenuated_and_owner_bound",
        "b3::ipc_service::tests::capability_transfer_attenuates_and_requires_grant",
    ],
    "failed copy-out capability cleanup": [
        "b3::resume::tests::failed_copy_out_closes_the_delivered_capability",
        "b3::user_memory::tests::failed_copy_out_through_the_adapter_closes_the_delivered_capability",
        "b3::syscall_dispatch::tests::capability_transfer_and_failed_copy_out_through_dispatch",
    ],
    "process-exit cleanup": [
        "b3::process_lifecycle::tests::exit_closes_every_capability_kills_blocked_threads_and_invalidates_handles",
        "b3::process_lifecycle::tests::shared_objects_survive_one_owner_and_die_with_the_last",
        "b3::process_lifecycle::tests::fault_exit_and_kill_event_clean_up_too",
        "b3::process_lifecycle::tests::a_failed_process_operation_is_reported_and_cleanup_is_retried_by_the_retry",
    ],
    "thread-exit cleanup, no re-wake": [
        "b3::thread_lifecycle::tests::cleanup_removes_wait_and_pending_but_keeps_process_capabilities",
        "b3::thread_lifecycle::tests::a_dead_thread_is_never_woken_or_handed_a_message",
        "b3::process_lifecycle::tests::a_thread_exit_keeps_the_process_capabilities_until_the_last_thread_goes",
        "b3::syscall_dispatch::real_switch::a_thread_killed_while_blocked_never_resumes_and_leaves_no_ipc_state",
    ],
    "interrupted operations, duplicate wake-up prevention": [
        "b3::syscall_dispatch::real_switch::an_interrupted_receive_returns_eintr_leaves_the_buffer_and_loses_no_message",
        "b3::thread_lifecycle::tests::interrupt_wakes_an_ipc_waiter_once_and_it_completes_as_eintr",
        "b3::resume::tests::interruption_reports_eintr_and_leaves_the_buffer_alone",
        "b3::scheduler_bridge::tests::interrupt_wakes_only_threads_blocked_in_ipc_and_only_once",
    ],
    "stale handles, repeated cycles, no leaks": [
        "b3::syscall_dispatch::tests::many_create_use_close_cycles_leak_nothing",
        "b3::process_lifecycle::tests::exit_closes_every_capability_kills_blocked_threads_and_invalidates_handles",
        "b3::syscall_dispatch::real_switch::shutdown_while_blocked_completes_the_waiter_with_eintr",
    ],
    "pending-operation protocol (no stale / unrelated / duplicate completion)": [
        "b3::resume::tests::a_pending_record_of_another_process_is_never_completed",
        "b3::resume::tests::no_pending_record_is_a_protocol_error_that_cleans_up",
        "b3::resume::tests::begin_drops_a_record_left_by_a_dead_thread_in_a_reused_slot",
        "b3::resume::tests::table_arm_take_cancel_and_counters",
    ],
    "B3 regressions preserved / new B3 teardown API": [
        "b3::ipc::tests::close_all_destroys_everything_and_wakes_every_waiter_with_closed",
        "b3::cap_space::tests::remove_all_empties_the_table_and_invalidates_every_handle",
    ],
}
listing = subprocess.run([str(binary), "--list"], check=True, capture_output=True, text=True).stdout
available = {l.split(": test")[0] for l in listing.splitlines() if l.endswith(": test")}
missing = [t for ts in required.values() for t in ts if t not in available]
if missing:
    raise SystemExit("required tests are missing:\n  " + "\n  ".join(missing))
print(f"   required-coverage table: {sum(len(v) for v in required.values())} named tests present "
      f"for {len(required)} requirement groups", flush=True)

# Repeat the real-context-switch tests: they are serialised by HOST_SWITCH_LOCK and must be stable.
for i in range(5):
    n = count(run("real_switch::", threads=8))
    if n != 8:
        raise SystemExit(f"real_switch repetition {i}: expected 8 tests, got {n}")
print("   real-context-switch tests: 5 repetitions x 8 tests stable", flush=True)

# --------------------------------------------------------------------------------------------
print("== no_std compile check: kernel tree D5 + b3 (warnings are errors)", flush=True)
nostd_root = build_dir / "b4_nostd_root.rs"
nostd_root.write_text("#![no_std]\n" + decls)
subprocess.run(
    [rustc, "--edition=2024", "--crate-type=lib", "--emit=metadata", "-D", "warnings",
     str(nostd_root), "-o", str(build_dir / "b4_nostd.rmeta")],
    check=True,
)

# --------------------------------------------------------------------------------------------
print("== unsafe audit: b3 sources", flush=True)


def non_test_code(text):
    """Source with comments removed and everything from `#[cfg(test)]` on dropped."""
    cut = text.find("#[cfg(test)]")
    body = text if cut < 0 else text[:cut]
    return "\n".join(l for l in body.splitlines() if not l.lstrip().startswith("//"))


unsafe_sites = {}
for rel in b3_files:
    n = len(re.findall(r"\bunsafe\b", non_test_code((root / rel).read_text())))
    if n:
        unsafe_sites[rel] = n
# The only production `unsafe`: `pub unsafe fn new` and the one `unsafe { dispatch::reschedule }`.
if unsafe_sites != {"src/b3/scheduler_bridge.rs": 2}:
    raise SystemExit(f"unexpected unsafe in b3 production code: {unsafe_sites}")
print("   production unsafe: scheduler_bridge.rs only (2 sites: DispatchPort::new, one reschedule call)",
      flush=True)

# --------------------------------------------------------------------------------------------
if shutil.which("rustfmt"):
    print("== rustfmt --check: every changed or new Rust file", flush=True)
    for rel in b3_files + changed_outside_b3:
        subprocess.run(
            ["rustfmt", "--edition=2024", "--check", "--config", "skip_children=true",
             str(root / rel)],
            check=True,
        )
else:
    print("== rustfmt not installed: formatting not checked", flush=True)

# --------------------------------------------------------------------------------------------
if shutil.which("gcc") and shutil.which("ld"):
    print("== link check: kernel image declaring b3 (HOST target, NOT a boot test)", flush=True)
    out_dir = build_dir / "image"
    shutil.rmtree(out_dir, ignore_errors=True)
    env = dict(os.environ, EUK_ALLOW_UNPINNED_RUSTC="1", EUK_TARGET="host")
    subprocess.run(
        ["python3", str(root / "scripts/build.py"), "--output", str(out_dir), "--d6"],
        check=True, env=env, capture_output=True, text=True,
    )
    print("   image linked and scripts/check.py structural checks passed (host-target build)", flush=True)
else:
    print("== gcc/ld missing: image link check skipped", flush=True)

print(f"B4 HOST TESTS PASSED ({total_b4} B4 tests, {total_b3} B3 tests, {d5_total} D5 tests re-run); "
      "not boot-tested", flush=True)
