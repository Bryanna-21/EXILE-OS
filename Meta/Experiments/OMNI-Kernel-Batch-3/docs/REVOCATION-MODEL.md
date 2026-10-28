# Revocation model (Batch 3)

Status: describes `Kernel.revoke` and the rules around it in the hosted model. Test outcomes are
recorded only in `REPORT.md`. Batch 1 left revocation unimplemented and recommended option R2
(derivation tree). This batch implements R2 for kernel-resident objects.

## 1. Derivation tree

- Every capability node has at most one parent. `createObject` makes a **root** (no parent).
  `derive` makes a **child** of the node it was derived from.
- A child always refers to the same object and has rights that are a subset of its parent's rights.
- `transfer` changes which space holds a node. It does **not** change the node's place in the tree.
- `release` of a node removes only that node. Its children are re-parented to its parent (or become
  roots if it was a root). Authority is never silently lost from the tree, and an ancestor can still
  revoke the children. Batch 2 test CAP-08 ("derived handles survive release of their source") is
  consistent with this.
- Roots have no ancestor, so nothing can revoke them except their holders releasing them.

## 2. What `revoke(space, h)` does

1. The caller names a node by `(space, handle)`. The node must carry `admin`, else `AccessDenied`.
2. Every **descendant** of that node is removed, in whichever space it is currently held: its table
   slot is freed (so its generation advances and the handle goes stale), its `handles` charge is
   refunded to the holder's account, and its reference to the object is dropped.
3. The named node **survives**, together with its ancestors and siblings.
4. The result is the number of nodes removed (0 is a success and a no-op).
5. Revocation is atomic: it happens inside one critical section (section 4).
6. Revocation never destroys an object (the surviving node keeps a reference). It can leave an object
   with a smaller reference count only.

Traversal is iterative (no recursion), so a derivation chain of any length is revoked without stack
growth (test REV-09 uses a chain of 3,000). Batch 1 recommended a bounded derivation depth; this model
does not bound depth and is limited only by the node quota. Cost of `revoke` is proportional to the
number of descendants, and it holds the kernel lock for that whole time (a latency risk, see the
risks list in `REPORT.md`).

## 3. Propagation, stale authority and slot reuse

- **Through derived capabilities:** descendants of descendants are removed (REV-01, REV-09).
- **Across spaces:** a descendant that was transferred elsewhere is still a descendant and is revoked
  (REV-03, LC-05).
- **After a middle node is released:** its children now hang from the grandparent and are revoked by
  the grandparent (LC-04, REV-04).
- **Stale handles:** a revoked handle's slot generation has advanced. If the slot is reused, the old
  handle still fails and the new holder receives only the rights of the new derive (REV-05). If the
  generation space is exhausted the slot is retired and never reused (GEN-02, GEN-03).
- **Stale tokens:** an in-flight operation's `OpToken` names the node by index **and** generation,
  so it cannot be mistaken for a later node in the same slot (GEN-03).

## 4. Linearization points

Every public operation is linearized at the single critical section in which it commits.

| Operation               | Linearization point                                                                                                    |
| ----------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `derive`                | The commit step of `derive`: the new node is in the table and the tree                                                 |
| `revoke`                | The commit of `revokeBelow`: all descendants are gone at once                                                          |
| `release`, `closeSpace` | The commit that removes the node(s)                                                                                    |
| `transfer`              | The commit that moves the node: the source handle dies and the destination handle appears in the same critical section |
| `beginOp`               | The commit that checks authority and increments the object's pin count                                                 |
| `endOp`                 | The commit that frees the pin                                                                                          |
| Object destruction      | The commit that moves the object to `dying` (before the destructor runs)                                               |

Consequences, all tested:

- A `derive` linearized before a `revoke` of its ancestor is revoked by it; a `derive` linearized after
  the revoke from the surviving node creates a node that the earlier revoke did not touch (REV-10,
  CON-03).
- No observer can see a partly applied revoke: two descendants removed by one revoke disappear together
  (CON-02: an observer that reads one then the other never finds the later-read one alive after the
  earlier-read one was gone).
- `transfer` racing `revoke`: the moved node is either revoked in its old place before the move
  (the transfer then fails with `InvalidHandle`) or moved first and revoked at its new holder (CON-06).
- Two `release` or `transfer` calls on the same handle: exactly one succeeds (CON-04, CON-05).

## 5. In-flight operations (Batch 1 B.8 "unresolved challenge")

The rule implemented here:

1. An operation calls `beginOp(space, handle, type, rights)`. Authority is checked **once**, at this
   point, and that is the operation's linearization point. The call also **pins** the object.
2. A revoke linearized **before** `beginOp` makes `beginOp` fail when the handle it names was revoked. A revoke linearized **after**
   `beginOp` does not stop the operation: it was authorized when it started. (Batch 3 does not claim
   that operations are aborted.)
3. After a later revoke or release, `opStillAuthorized(token)` returns `false`. A long operation that
   wants to stop at the next safe point calls it between steps. It returns `true` across a `transfer`,
   because the capability still exists. It becomes `false` if the holder's space is closed.
4. The pin keeps the object (and its charge) alive until `endOp`, even if the last capability is
   released or revoked. The destructor runs at the final `endOp` (REV-06, REV-08).
5. `endOp` with a used or stale token is `InvalidHandle`.

What this does **not** give: it does not cancel an operation already running, does not make `revoke`
wait for in-flight operations to finish ("synchronous revocation"), and does not apply to
operations that never called `beginOp`. A caller that skips `beginOp` after a successful `lookup`
has a time-of-check/time-of-use gap: `lookup` returns a value, not a pin.

## 6. Not implemented or not claimed

- R1 (indirection for service-defined authority) and R3 (epochs).
- Synchronous revocation (revoke returning only after in-flight operations stopped).
- A depth bound on the derivation tree.
- Revoking a root's own capability on behalf of someone who does not hold one: roots have no ancestor.
- Concurrency beyond the single global lock (see `CONCURRENCY.md`).
- Anything about IPC messages that carry handles while a revoke runs: IPC is not integrated.
