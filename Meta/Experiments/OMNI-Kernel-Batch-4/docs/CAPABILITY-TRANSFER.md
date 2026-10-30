# Capability transfer through IPC (Batch 4)

## 1. Descriptors

`Desc { handle, otype, mode, rights }`. `otype` is the type the sender believes the object has; a
mismatch is `InvalidHandle` (Batch 1 B.3).

| mode | sender keeps it? | needs on sender's capability | receiver gets |
|---|---|---|---|
| `move` | no (on success) | TRANSFER | the same rights (`rights` must equal the sender's exactly, else `InvalidArgument`) |
| `copy` | yes | TRANSFER + DUPLICATE | `rights`, a subset of the sender's rights that includes TRANSFER |

Escalation (`rights` not a subset) is `AccessDenied`. Reserved right bits are `InvalidArgument`. The same
handle twice in one message is `InvalidArgument` (Batch 2 rule). Sending the destination end through
itself is `InvalidArgument` (direct cycle). A copy is a Batch 3 *derived* capability, so revoking the
sender's capability revokes in-flight and delivered copies.

## 2. Atomic publication

Validation reads only. Staging moves capabilities into the destination's hold space one at a time; the
message becomes visible only at the publication point. Any failure before it rolls everything back:
copies are released; moved capabilities are moved back to the sender; the accounting capability is
released. Nothing is published.

## 3. Failure rollback guarantees and limits (honest)

* Staged-but-unpublished capabilities are not visible to the receiver.
* A moved capability returns to the sender **under a new handle value** (Batch 3 `transfer` allocates a new
  slot). The sender is told through `SendReport.rebound` (`old` → `new`). Authority is conserved; the
  number is not.
* If the sender's own space cannot take it back (closed or quota/slot exhausted concurrently) the
  capability is released and counted in `SendReport.lost_count` / `lost_on_rollback`. This needs a
  concurrent change of the sender's own space; it is not exercised deterministically by the tests.
* If a capability was revoked during rollback it is gone and is not counted as lost.

## 4. Receive

Capabilities are moved from the hold space into the receiver's space; each result is a `Slot`:
`delivered` (valid handle), `revoked` (revoked while queued; `Received` still delivered), or `dropped`.
Failure semantics:

* Receiver quota or allocation failure (`QuotaExceeded`): every capability installed so far is moved back,
  the message stays queued and unclaimed; retry is possible.
* Receiver space closed/destroyed mid-install (`PeerDead`): the receive fails; capabilities already
  installed died with that space; the rest are released; the message is consumed (it cannot be delivered).
* `partial` delivery (`dropped` slots) exists as a defensive path for "installed capabilities cannot be
  returned for a reason other than revocation or a dead space". The tests could not construct it through the
  public API; it is **untested** and not relied upon.

## 5. Revocation races

Revocation is Batch 3 `revoke`: it removes all descendants of the revoked capability.

| revoke linearizes… | outcome |
|---|---|
| before the descriptor is validated | send fails (`InvalidHandle` / `AccessDenied`) |
| between a copy's derivation and its staging | send fails with `InvalidHandle`, nothing published, sender intact |
| after staging, before publication | send succeeds; receiver sees the slot `revoked` |
| while queued | receiver sees `revoked` |
| after installation | the receiver's handle is revoked like any other (`InvalidHandle` on use) |

A revocation never leaves a capability usable: the success point of delivery is the successful `transfer`
into the receiver's space; revocation after it applies to the installed handle.

## 6. Limits of Batch 3 that shape this design (see PROPOSED-ABI-CHANGES.md)

Batch 3 `transfer` cannot attenuate and requires TRANSFER, so a received capability always carries
TRANSFER; there is no transactional multi-transfer; accounting cannot be reserved without creating an
object. Batch 4 works around these with the hold space and accounting objects, without modifying Batch 3.
