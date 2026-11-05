# OMNI-Kernel Batch 4: hosted IPC core

Experimental, hosted, architecture-neutral IPC prototype built on the OMNI-Kernel Batch 3 capability kernel.
**Not a kernel**: no boot, no hardware isolation, no syscall or scheduler integration, no production ABI,
no network IPC.

## What it provides

* Channels with two ends (Batch 3 objects with explicit lifecycle and rights).
* Bounded per-end queues (message count and byte limits) with deterministic full-queue behaviour.
* Versioned messages using the unmodified Batch 2 wire format, with provisional limits.
* Capability transfer through messages: `move` / `copy`, attenuation on copy, escalation rejected,
  atomic publication, rollback on failure, documented revocation-race outcomes.
* Waiting with timeout and cancellation (condition variable; no busy-waiting), closure that wakes waiters.
* Resource accounting reusing Batch 3 (`queued_bytes` charged to the sender, refunded on receive/drop).

## Layout

* `src/ipc.zig` the implementation; `src/batch2.zig` re-exports the unmodified Batch 2 core.
* `src/tests.zig` + `src/tests/*.zig` the tests.
* `docs/` IPC-MODEL, CAPABILITY-TRANSFER, CANCELLATION-AND-CLOSURE, CONCURRENCY, TEST-PLAN, PROPOSED-ABI-CHANGES.
* `REPORT.md` results, commands, unresolved issues. `manifest.json` SHA-256 of every other file.

Batches 1-3 are not modified; this package compiles Batch 3 `kernel.zig`, and the Batch 3 and Batch 2
test suites, from their existing locations.

## Build and test

Requires Zig 0.15.2.

```
cd Meta/Experiments/OMNI-Kernel-Batch-4
zig build test --summary all      # Batch 4 + Batch 3 + Batch 2 suites
zig build fmt                     # format check
bash scripts/verify.sh            # files, toolchain, format, build+tests, whitespace, manifest
```

`verify.sh` exits 0 on success, 1 on a failed check, 2 if Zig is missing. Use `ZIG=/path/to/zig`.

## API sketch

```zig
const x = try Ipc.create(allocator, kernel, .{});
const ends = try x.createChannel(space);
try x.send(space, ends[0], wire_bytes, &descs, .{ .timeout_ns = null, .cancel = cx }, &report);
var out: Received = undefined;
try x.recv(other_space, peer_end, .{ .timeout_ns = 50 * ns_per_ms }, &out);
defer out.deinit(x);
```

Destroy order: `Kernel.destroy()` first (runs every destructor), then `Ipc.destroy()` (returns the number of
leaked registry entries, expected 0).
