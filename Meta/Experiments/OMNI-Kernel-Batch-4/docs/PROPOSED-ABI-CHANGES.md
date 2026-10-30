# Proposed ABI / API changes (not applied)

Batch 1 and Batches 2-3 are unchanged. Batch 4 only documents what would simplify or harden IPC.

1. **Batch 3 `transfer` with attenuation** (`transferWithRights(src, h, dst, rights)`): lets a move drop
   TRANSFER or other rights in one atomic step; today a moved capability keeps all rights.
2. **Transactional multi-transfer** (`transferMany` all-or-nothing, handle values of the source preserved on
   failure): removes the rebound-handle behaviour and the `lost` case on rollback.
3. **Account reservation without an object** (`reserve(space, resource, n)` / `refund`): replaces the
   per-message accounting object.
4. **Revocation notification / waiter wake** (a kernel hook or condition tied to a capability node): lets
   revocation wake blocked IPC waiters.
5. **Distinct error for "space gone"** versus "capability revoked": Batch 3 returns `InvalidHandle` for both
   when a destination space is freed; Batch 4 disambiguates with an extra lookup.
6. **Batch 1 C.4**: state explicitly that a message's capability array is separate from the byte payload
   and that `handle_count` is validated against it; define `Cancelled`, `TimedOut`, `WouldBlock` as
   first-class ABI errors (Batch 2 already has them).
