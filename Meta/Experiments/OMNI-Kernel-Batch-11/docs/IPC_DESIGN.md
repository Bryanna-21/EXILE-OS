# Capability and channel IPC (Batch 11): design and ABI

Crate `omni-kernel-batch11` (`omni11`). Hosted tests only. Source: `src/caps.rs`, `src/ipc.rs`, `src/sys.rs`.

## Capabilities

- Per-space table of 32 slots. A handle is `generation << 16 | index`; generation 0 is invalid. Removing a capability bumps its slot
  generation, so the old handle goes stale; a slot whose generation would wrap is **retired** instead of reused.
- Rights (9 bits, same order as Batch 2): read, write, execute, duplicate, transfer, wait, signal, admin, inspect. Reserved bits must be zero.
- Lookup order: handle live, object type, rights. A handle from another space never resolves.
- Objects are channel ends (32 in the pool). A pair is created together. An object is alive while it is reachable from a capability table
  or from a queued message of a reachable end; a mark-and-sweep runs after release, close or space exit, so cycles are collected.
  Explicit close (ADMIN) is revocation: the end dies and its peer sees `PEER_DEAD`.

## Syscalls (Batch 5 numbers and layouts)

| Nr | Name | Registers (OMNI convention) | Notes |
|---|---|---|---|
| 0x100 | endpoint_create | rdi = out ptr (8 bytes: two handles) | output probed first; on copy failure both handles are released |
| 0x101 | endpoint_close | rdi = handle | needs ADMIN |
| 0x102 | send | rdi = handle, rsi = send_args ptr (48 B) | needs WRITE; queue depth 4, full returns `WOULD_BLOCK` |
| 0x103 | recv | rdi = handle, rsi = recv_args ptr (48 B) | needs READ; timeout in scheduler ticks (0 = poll) |
| 0x104, 0x105 | cancel create/signal | | `NOT_SUPPORTED` |
| 0x106 | endpoint_stats | rdi = handle, rsi = out (32 B) | needs INSPECT |
| 0x107 | handle_release | rdi = handle | |

Unused argument registers must be zero; handles wider than 32 bits are rejected, not truncated. Any other number is passed through to the
Batch 10 layer (`NO_SYS` for unknown numbers).

Structures (little endian): send_args 48 B {msg_ptr u64, msg_len u32, desc_count u32, desc_ptr u64, timeout u64, cancel u32, flags u32, reserved u64};
descriptor 16 B {handle u32, type u8, mode u8 (1 move, 2 copy), 0 u16, rights u32, 0 u32}; recv_args 48 B {buf_ptr u64, buf_cap u32, flags u32,
result_ptr u64, timeout u64, cancel u32, 0 u32, 0 u64}; recv_result 88 B {payload_len u32, op u16, kind u16, request_id u64, 0 u32, ndesc u32,
slots 8 x {status u32 (1 delivered, 2 revoked), handle u32}}; stats 32 B. Wire header 32 B as in Batch 2 (ABI version 1).

`scripts/check_ipc_conformance.py` compares 39 of these constants with the Zig sources (and detects 5 of 5 injected mutations).

## Deviations from the Zig model (deliberate, each listed)

1. Cancellation objects (0x104, 0x105) are not implemented.
2. Send never blocks: a full queue returns `WOULD_BLOCK`; a non-zero send timeout returns `NOT_SUPPORTED`.
3. Timeouts are scheduler ticks, not nanoseconds; unbounded waits are rejected (`INVALID_ARGUMENT`); the longest wait is 2^20 ticks.
4. Receive is all-or-nothing: if the receiver's table lacks room, the message stays queued and `QUOTA_EXCEEDED` is returned.
5. The receive buffer must hold the largest payload (992 bytes): a message is never truncated or consumed undeliverably. Messages are at most 1024 bytes
   (the Zig limit is 4096) and at most 8 descriptors; this keeps kernel stack use within the 16 KiB thread stacks (an earlier version overflowed them in debug builds and was fixed).
6. Handle table of 32, 32 objects, 8 spaces, 4 waiters per end: small fixed bounds, exhaustion returns `QUOTA_EXCEEDED`/`BUSY`.

## Concurrency and lifecycle

Lock order: IPC state lock, then scheduler lock. A receiver takes its block ticket while holding the state lock, so a sender cannot enqueue and wake it
between the emptiness check and the block. Killed threads are refused before dispatch, so a dead thread cannot recreate a table after
`cleanup_space`. `terminate_space` kills the threads and releases the space's capabilities. These properties were exercised from cooperative
threads on the real context-switch code; they were **not** exercised under preemption or SMP.
