# Failure model

| failure | behaviour | guarantee | test |
|---|---|---|---|
| invalid syscall number or arguments | negative status, no side effect | nothing changed | SYS-04..08 |
| bad user pointer | -32; nothing published | no partial effect (create: handles taken back) | SYS-10 |
| fault while copying out a received message | message consumed, capabilities released, -32, counted | no leaked authority; message dropped (documented) | SYS-11 |
| queue full | WouldBlock, or wait then TimedOut | message not enqueued | RES-01, RES-02 |
| quota exceeded | -6, state unchanged, later retry works | | RES-03, RES-04, RES-12 |
| allocation failure (kernel, IPC or wait layer) | -6 or normal result, never corruption | | RES-09..11 |
| peer closed while blocked | woken, PeerDead | | BLK-08, BLK-09 |
| capability revoked while blocked or queued | InvalidHandle / slot reported `revoked` | | BLK-10, STRESS-02 |
| cancellation | Cancelled; cancel before call wins without consuming | | BLK-05..07, BLK-17 |
| timeout | TimedOut, not earlier than the deadline, nothing left behind | | BLK-03, BLK-16 |
| space destroyed with queued traffic | exactly-once release of messages and capabilities | | RES-07, RES-08 |
| waiter limit | -6 | | BLK-13 |

Not handled: hardware faults, memory corruption by other kernel code, panics in the Batch 3/4 layers, and
power loss (no persistence exists).
