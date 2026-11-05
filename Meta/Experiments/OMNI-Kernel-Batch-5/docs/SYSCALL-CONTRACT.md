# Syscall contract (proposed)

Status: PROPOSED and implemented as a hosted dispatcher (`src/syscall.zig`). Numbers and layouts are
PROVISIONAL (Batch 1 assigned none). No architecture stub exists, and nothing is integrated into a kernel.

## Entry contract

An architecture stub must:
1. fill `Args{ nr, a[0..5] }` from registers (zero-extended to 64 bits);
2. pass the calling address space as obtained from the kernel's own thread state, never from arguments;
3. provide `UserMemory` (`copy_in`, `copy_out`, `probe_write`), each all-or-nothing and fault-safe;
4. write the returned `i64` back as the result register.

### Register mapping (specified, NOT implemented)

| | number | a0 | a1 | a2 | a3 | a4 | a5 | result |
|---|---|---|---|---|---|---|---|---|
| x86-64 (`syscall`) | rax | rdi | rsi | rdx | r10 | r8 | r9 | rax |
| ARM64 (`svc #0`) | x8 | x0 | x1 | x2 | x3 | x4 | x5 | x0 |

Both adapters feed the same `dispatch`; no operation depends on the architecture.

## Operations

| nr | name | a0 | a1 | return | rights required |
|---|---|---|---|---|---|
| 0x100 | endpoint_create | out_ptr (8 bytes: two u32 handles) | 0 | 0 | none; charges `channels` quota |
| 0x101 | endpoint_close | endpoint | 0 | 0 | ADMIN |
| 0x102 | send | endpoint | args_ptr | 0 | WRITE on the end; TRANSFER (+DUPLICATE for copy) per capability; WAIT on cancel |
| 0x103 | recv | endpoint | args_ptr | 0 | READ on the end; WAIT on cancel |
| 0x104 | cancel_create | 0 | 0 | the new handle | none |
| 0x105 | cancel_signal | cancel handle | 0 | 0 | SIGNAL |
| 0x106 | endpoint_stats | endpoint | out_ptr (32 bytes) | 0 | INSPECT |
| 0x107 | handle_release | handle | 0 | 0 | none (own handle) |

Unknown numbers return `-33` (no_sys). Unused argument registers must be zero. A handle argument above
`u32::MAX` is rejected (`-1`), never truncated.

## User structures (little-endian, fixed offsets, no in-place reinterpretation)

SendArgs (48 bytes): `msg_ptr u64@0`, `msg_len u32@8`, `desc_count u32@12`, `desc_ptr u64@16`,
`timeout_ns u64@24`, `cancel u32@32`, `flags u32@36` (must be 0), `reserved u64@40` (must be 0).

Descriptor (16 bytes): `handle u32@0`, `otype u8@4`, `mode u8@5` (move or copy), `reserved u16@6`,
`rights u32@8`, `reserved u32@12`. Reserved fields must be zero.

RecvArgs (48 bytes): `buf_ptr u64@0`, `buf_cap u32@8`, `flags u32@12` (0), `result_ptr u64@16`,
`timeout_ns u64@24`, `cancel u32@32`, `reserved u32@36`, `reserved u64@40`.

RecvResult (88 bytes): `payload_len u32@0`, `op u16@4`, `kind u16@6`, `request_id u64@8`,
`flags u32@16` (partial), `count u32@20`, then 8 slots of `{status u32, handle u32}` from byte 24.

## Validation rules

* `msg_len` in [32, `max_message_bytes`] (default 4096); `desc_count` <= `max_handles` (default 8).
* All lengths are bounded before use; address arithmetic is checked (`addr + len` overflow is a fault).
* `buf_cap` must be at least the largest payload, so a message is never truncated.
* `timeout_ns`: 0 polls; 1..`max_wait_ns` (default 30 s) waits; `u64::MAX` waits for events and requires a
  cancel handle. Any other value is invalid.
* The output buffer and result block of `recv` are probed before the message is consumed.
* If a `copy_out` faults after the message was consumed, the delivered capabilities are released, the
  event is counted (`delivery_faults`) and `-32` is returned. The message is dropped (documented loss).
* If `send` fails after capabilities were rolled back under new handle values, the new values are written
  into the user descriptor array; if that write faults the new handles stay valid in the caller's space.

## Status codes

Batch 2 codes are unchanged: -1 invalid_argument, -2 invalid_handle, -3 access_denied, -4 not_found,
-5 already_exists, -6 quota_exceeded, -7 would_block, -8 timed_out, -9 cancelled, -10 peer_dead,
-11 not_supported, -12 bad_version, -13 overflow, -14 busy, -15 io_error, -16 internal, -17 invalid_state.
New (proposed): -32 fault (bad user pointer), -33 no_sys. Success is 0, or the returned handle for
`cancel_create`.
