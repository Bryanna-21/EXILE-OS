# IPC model (Batch 4)

Hosted prototype. Provisional limits are marked PROVISIONAL and are not ABI.

## 1. Objects

* **Channel**: two **ends**. A message sent through end X is queued in the *inbox* of the other end.
* Each end is a Batch 3 object of type `channel` with a destructor. Callers hold ordinary Batch 3
  capabilities to it. Rights used: READ (recv), WRITE (send), ADMIN (close), INSPECT (stats).
  `createChannel` returns two handles with every right except EXECUTE; it charges 1 `channels` per end
  to the creating space and is all-or-nothing.
* **Hold space**: each end owns a private Batch 3 space. Queued messages' capabilities and accounting
  capabilities live there, so in-flight authority has an owner, is counted, stays in the derivation
  tree (so it is revocable) and is released exactly once.
* **Cancellation object**: Batch 3 type `cancellation` (rights WAIT, SIGNAL, DUPLICATE, TRANSFER).

## 2. Ownership and lifecycle of an end

An end is *closed* by (a) `closeEnd` (needs ADMIN), or (b) its Batch 3 object being destroyed (last
capability released, owning space closed, or kernel destroyed). After closing:

| operation on the closed end | result |
|---|---|
| send / recv through it | `PeerDead` |
| send from the peer | `PeerDead` |
| recv on the peer | queued messages are still delivered in order, then `PeerDead` |
| blocked waiters on either side | woken; return `PeerDead` |

Teardown of an end (drop queued messages, release the hold space, free memory) runs exactly once, outside
all locks, when the end is closed **and** no operation is using it (`users == 0`). The channel structure is
freed when both ends are torn down and both objects destroyed. `closeEnd` leaves the capability valid.

## 3. Messages

Messages use the unmodified Batch 2 wire format (32-byte header + payload; version, flags, kind and length
validation by `wire.decode`). Handle slots are not in the bytes: the header's `handle_count` must equal the
number of `Desc` values passed (`InvalidArgument` otherwise). Provisional limits (`Config`):
`max_message_bytes` 4096, `max_handles` 8 (hard cap `max_desc` = 8), `max_messages` 8 per inbox,
`max_queue_bytes` 16384 per inbox. `Config.validate` rejects zero queues, a byte limit below one maximal
message, and handle limits above 8.

## 4. Bounded queue behaviour

An inbox holds at most `max_messages` messages and `max_queue_bytes` wire bytes, counting messages
*being published* (reserved) as well, so the limit can never be exceeded, even transiently. A full inbox
yields `WouldBlock` (timeout 0), `TimedOut` (deadline) or waits (no deadline); FIFO order is by publication
point. Allocation failure while sending is `QuotaExceeded` with the reservation returned and nothing
published (Batch 3 reports allocation failure as `QuotaExceeded`; kept for consistency).

## 5. Accounting

Each queued message is charged `queued_bytes = wire size` to the **sender's** space by creating one small
Batch 3 object there and moving its capability into the receiving end's hold space. Receive, close,
teardown or send rollback releases it; Batch 3's exact-once destruction refunds the sender. Each in-flight
capability additionally occupies one handle slot of the hold space (budget `max_messages * (1 + max_handles)`).

## 6. Operation order

`send`: endpoint capability → wire validation → descriptor validation → (pre-signalled cancel) → peer
closed → room (WouldBlock / TimedOut / Cancelled) → allocation → accounting → capability transfer →
publication. `recv`: endpoint capability → cancel pre-signalled → end closed → message available →
peer closed → non-blocking → cancelled → timed out. A queued message is never lost by a failing receive:
the message remains queued unless delivery committed.

## 7. Linearization points

* send: the critical section that moves the message from *reserved* to *queued*. Before it nothing is
  visible to the peer; after it the message is delivered at-most-once to some receiver.
* recv: claim (message marked claimed under the lock, still counted); commit (unlink) is the point at
  which the message is consumed.
* close: the critical section that sets `closed`.
