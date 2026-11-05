# Kernel-facing IPC integration

Status: HOSTED, KERNEL-FACING. Nothing here is linked into a bootable kernel.

## Layering

```
  arch entry stub (x86-64 / ARM64)   specified only, see SYSCALL-CONTRACT.md
        |  Args{nr, a[6]}, caller SpaceId (from the kernel, never from user registers)
  src/syscall.zig        argument decoding, validation, user-memory boundary, status codes
        |
  src/kernel_ipc.zig     KernelIpc: waiting, policy, wake-ups, counters, audit
        |    uses src/wait_queue.zig (eventcount) and src/accounting.zig (waiter ledger)
  Batch 4 ipc.zig        channels, bounded queues, capability transfer, cancellation
        |
  Batch 3 kernel.zig     spaces, objects, capability tree, quotas, pins
        |
  Batch 2 core           rights, wire format, status codes
```

Batches 2 to 4 are compiled from their existing directories and are not modified. Nothing below
`kernel_ipc.zig` calls back into it.

## Objects, ownership and lifetime

| object | created by | owner / billed to | destroyed when |
|---|---|---|---|
| endpoint (one channel end) | `endpointCreate` | creating space (`channels` quota) | last capability to the end is released, revoked or its space is closed; queued messages and in-flight capabilities are released exactly once |
| cancellation object | `cancelCreate` | creating space | last capability released |
| queued message | `send` | sending space (`queued_bytes`) | received, or its end is destroyed |
| in-flight capability | `send` with descriptors | the receiving end's hold space | delivered to the receiver, rolled back to the sender, or destroyed with the end |
| blocked-waiter slot | a blocking `send`/`recv` | calling space (this layer) | the call returns (always, on every path) |

A message that cannot be delivered is never half-published: `send` either publishes the message together with
all its capabilities or changes nothing (a moved capability is rolled back; if its original slot is gone it
returns under a new handle value, which the syscall layer reports back to user memory).

## Rights on every operation

Authorization is performed by the capability layer on each call; a syscall number confers nothing. The
table in `syscall.zig` (`table`) and SYSCALL-CONTRACT.md list the rights per operation. A capability that is
revoked while a caller is blocked makes the retry fail with `InvalidHandle`.

## Kernel-facing interface (`KernelIpc`)

`endpointCreate`, `endpointClose`, `endpointStats`, `cancelCreate`, `cancelSignal`, `releaseHandle`,
`revoke`, `spaceClose`, `send`, `recv`, `deinitReceived`, `audit`, `create`, `destroy`. Errors are the
Batch 2 `OmniError` set; the syscall layer adds `Fault` and `NoSys`.

Use the wrappers (`spaceClose`, `releaseHandle`, `revoke`, `endpointClose`, `cancelSignal`) for state changes
that must wake blocked callers immediately. Changing the same state by calling Batch 3 or Batch 4
directly is allowed but is only noticed by blocked callers at their deadline (test BLK-11 pins this).

## Teardown order

`Kernel.destroy()` first, then `KernelIpc.destroy()`, which returns the number of leaked registry entries
(expected 0).
