//! Secure capability transfer through IPC (Mission D6, file 7).
//! SPDX-License-Identifier: Apache-2.0
//!
//! Transfer is a two-step protocol so that no failure can leave half-installed authority:
//!
//! 1. **Stage (at `send`)** — [`Ipc::stage_transfers`]. For every `(handle, rights)` entry the
//!    kernel resolves the handle *in the sender's own table*, requires the sender's capability to
//!    hold `TRANSFER`, and requires `rights ⊆ sender's rights` (otherwise `RightsEscalation`;
//!    nothing is silently widened or narrowed). It then derives an **in-flight capability**
//!    (owner `Owner::INFLIGHT`, charged to the sender) with exactly the requested rights. The
//!    in-flight capability holds a reference on the object, so the object cannot be destroyed
//!    while the message is queued. If any entry fails, every capability staged so far is removed
//!    and the send fails: no message is queued.
//! 2. **Deliver (at `recv`)** — [`Ipc::deliver_transfers`]. Everything that can fail is checked
//!    *before* anything changes: every staged capability must still be live (else
//!    `TransferRevoked`), the receiver needs enough free handle slots (`DestinationFull`) and
//!    capability quota (`QuotaExceeded`). Then the capabilities are re-owned and installed one by
//!    one; if an unexpected failure occurs midway, the ones already installed are detached and
//!    returned to the in-flight state. On any error the message stays queued unchanged (except
//!    `TransferRevoked`, where the message is consumed so the queue cannot jam).
//!
//! # Guarantees
//! * A sender can only transfer capabilities it holds, with the `TRANSFER` right, and never with
//!   more rights than it holds. The receiver gets exactly the requested (≤ held) rights.
//! * Authority revoked while in flight is never installed: revocation of an ancestor
//!   (`revoke_descendants`), closure of the endpoint (sweep) and process exit all remove the
//!   in-flight capability; delivery then reports `TransferRevoked`.
//! * No leaks: in-flight capabilities of discarded messages are removed
//!   ([`Ipc::discard_staged`]); `Ipc::check_invariants` proves every `INFLIGHT` capability is
//!   referenced by exactly one queued message or backlog entry.
//! * Duplicate handles in one transfer list are refused (`DuplicateTransfer`).
//!
//! # `MOVE_CAPS`
//! With the `MOVE_CAPS` flag the sender's handles are closed after the message is queued. A moved
//! capability is still a *derived* one, so forwarding the same authority through more than
//! [`crate::d6::capability::MAX_DEPTH`] hops fails with `DepthExceeded` (documented limitation).
#![allow(dead_code)]

use crate::d6::capability::{CapId, Capability, Rights};
use crate::d6::handle::Handle;
use crate::d6::ipc::{Ipc, MAX_CAPS_PER_OWNER};
use crate::d6::ipc_message::{Message, MsgFlags, MAX_TRANSFERS};
use crate::d6::object::{Error, Owner};

impl Ipc {
    /// Stage the transfer list of `msg` (see module docs). On error nothing is staged.
    pub(crate) fn stage_transfers(&mut self, sender: Owner, msg: &mut Message) -> Result<(), Error> {
        msg.staged = [None; MAX_TRANSFERS];
        let n = msg.transfer_count();
        if n == 0 {
            return Ok(());
        }
        // Pass 1: validate everything without mutating.
        let mut srcs: [Option<(CapId, Capability)>; MAX_TRANSFERS] = [None; MAX_TRANSFERS];
        {
            let table = self.procs.get_mut(sender)?;
            for i in 0..n {
                let t = msg.transfers()[i];
                let (id, cap) = match table.lookup(
                    &self.caps,
                    &self.objects,
                    Handle::from_raw(t.handle),
                    Rights::TRANSFER,
                ) {
                    Ok(x) => x,
                    Err(Error::Revoked) => return Err(Error::TransferRevoked),
                    Err(e) => return Err(e),
                };
                if !t.rights.is_subset_of(cap.rights) {
                    return Err(Error::RightsEscalation);
                }
                if srcs[..i].iter().flatten().any(|(sid, _)| *sid == id) {
                    return Err(Error::DuplicateTransfer);
                }
                srcs[i] = Some((id, cap));
            }
        }
        // Pass 2: derive the in-flight capabilities, rolling back on failure.
        for i in 0..n {
            let (id, _) = srcs[i].ok_or(Error::Corrupt("missing transfer source"))?;
            let rights = msg.transfers()[i].rights;
            match self
                .caps
                .derive_kernel(&mut self.objects, id, rights, Owner::INFLIGHT, sender)
            {
                Ok(c) => msg.staged[i] = Some(c),
                Err(e) => {
                    for j in 0..i {
                        if let Some(c) = msg.staged[j].take() {
                            let _ = self.caps.remove(&mut self.objects, c);
                        }
                    }
                    self.stats.transfers_rolled_back += 1;
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// Removes the live in-flight capabilities of a message that will never be delivered.
    pub(crate) fn discard_staged(&mut self, msg: &Message) {
        for id in msg.staged.iter().flatten() {
            if self.caps.exists(*id) && self.caps.lookup_inflight(&self.objects, *id).is_ok() {
                let _ = self.caps.remove(&mut self.objects, *id);
            }
        }
    }

    /// Closes the sender's handles of a `MOVE_CAPS` message after it was queued.
    pub(crate) fn close_moved_handles(&mut self, sender: Owner, msg: &Message) -> Result<(), Error> {
        if !msg.flags.contains(MsgFlags::MOVE_CAPS) {
            return Ok(());
        }
        let table = self.procs.get_mut(sender)?;
        for t in msg.transfers() {
            match table.close(&mut self.caps, &mut self.objects, Handle::from_raw(t.handle)) {
                Ok(()) | Err(Error::Stale) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Installs the staged capabilities of `msg` into `receiver`'s handle table and rewrites the
    /// transfer metadata of this copy to the receiver's handle values and granted rights.
    /// All-or-nothing; the queued original is untouched.
    pub(crate) fn deliver_transfers(&mut self, receiver: Owner, msg: &mut Message) -> Result<(), Error> {
        let n = msg.transfer_count();
        if n == 0 {
            return Ok(());
        }
        for i in 0..n {
            match msg.staged[i] {
                Some(id) if self.caps.lookup_inflight(&self.objects, id).is_ok() => {}
                _ => return Err(Error::TransferRevoked),
            }
        }
        if self.procs.get(receiver)?.free_slots() < n {
            return Err(Error::DestinationFull);
        }
        if self.caps.charged_to(receiver) + n > MAX_CAPS_PER_OWNER {
            return Err(Error::QuotaExceeded);
        }
        let mut done: [Option<(Handle, CapId, Owner)>; MAX_TRANSFERS] = [None; MAX_TRANSFERS];
        let mut rewritten: [Option<(u32, Rights)>; MAX_TRANSFERS] = [None; MAX_TRANSFERS];
        for i in 0..n {
            let id = msg.staged[i].ok_or(Error::TransferRevoked)?;
            let cap = *self.caps.lookup_inflight(&self.objects, id)?;
            let step = self.caps.reown(&self.objects, id, receiver).and_then(|()| {
                match self.procs.get_mut(receiver)?.insert(id) {
                    Ok(h) => Ok(h),
                    Err(e) => {
                        let _ = self.caps.to_inflight(id, cap.charged);
                        Err(e)
                    }
                }
            });
            match step {
                Ok(h) => {
                    done[i] = Some((h, id, cap.charged));
                    rewritten[i] = Some((h.raw(), cap.rights));
                }
                Err(e) => {
                    for j in 0..i {
                        if let Some((h, cid, charged)) = done[j].take() {
                            let _ = self.procs.get_mut(receiver).and_then(|t| t.detach(h));
                            let _ = self.caps.to_inflight(cid, charged);
                        }
                    }
                    self.stats.transfers_rolled_back += 1;
                    return Err(e);
                }
            }
        }
        for (i, r) in rewritten.iter().enumerate().take(n) {
            if let Some((raw, rights)) = r {
                msg.set_delivered(i, *raw, *rights);
            }
        }
        msg.staged = [None; MAX_TRANSFERS];
        self.stats.transfers_committed += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::d6::capability::Rights;
    use crate::d6::handle::Handle;
    use crate::d6::ipc::testutil::*;

    use crate::d6::ipc_message::{MsgFlags, MsgKind, TransferRequest, Message};
    use crate::d6::object::{Error, Owner};

    fn xfer(h: Handle, rights: Rights) -> TransferRequest {
        TransferRequest {
            handle: h.raw(),
            rights,
        }
    }

    fn with(sender: Owner, flags: MsgFlags, list: &[TransferRequest]) -> Message {
        let m = Message::new(sender, MsgKind::Data, MsgFlags::NONE, 0, b"cap")
            .unwrap()
            .with_transfers(list)
            .unwrap();
        if flags.contains(MsgFlags::MOVE_CAPS) { m.with_move().unwrap() } else { m }
    }

    /// a <-> b channel plus a separate channel (a, c) whose `a` end is the thing transferred.
    fn rig3() -> (Rig, Owner, Owner, Owner, Handle, Handle, Handle, Handle) {
        let mut r = Rig::new();
        let (a, b, c) = (r.proc(1), r.proc(2), r.proc(3));
        let (ab_a, ab_b) = r.pair(a, b);
        let (ac_a, ac_c) = r.pair(a, c);
        (r, a, b, c, ab_a, ab_b, ac_a, ac_c)
    }

    #[test]
    fn successful_transfer_with_attenuation_and_accounting() {
        let (mut r, a, b, c, ab_a, ab_b, ac_a, ac_c) = rig3();
        let refs0 = r.ipc.objects().get_any(r.ipc.caps().peek(r.ipc.procs.get(a).unwrap().resolve(ac_a).unwrap()).unwrap().object).unwrap().refs;
        let m = with(a, MsgFlags::NONE, &[xfer(ac_a, Rights::SEND)]);
        r.ipc.send(&mut renv!(r), a, ab_a, m).unwrap();
        r.check();
        let got = r.ipc.recv(&mut renv!(r), b, ab_b).unwrap();
        let nh = Handle::from_raw(got.transfers()[0].handle);
        assert_eq!(got.transfers()[0].rights, Rights::SEND, "receiver learns what it got");
        // The receiver can send on the transferred end (to c) but nothing else.
        assert_eq!(r.ipc.rights_of(b, nh), Ok(Rights::SEND));
        r.ipc.send(&mut renv!(r), b, nh, msg(b, 9, b"via b")).unwrap();
        let m = r.ipc.recv(&mut renv!(r), c, ac_c).unwrap();
        assert_eq!(m.sender, b, "the new holder is identified as the sender");
        assert_eq!(r.ipc.recv(&mut renv!(r), b, nh).err(), Some(Error::RightsDenied));
        assert_eq!(
            r.ipc.duplicate_handle(b, nh, Rights::SEND).err(),
            Some(Error::RightsDenied),
            "no DERIVE was granted"
        );
        let refs1 = r.ipc.objects().get_any(r.ipc.caps().peek(r.ipc.procs.get(a).unwrap().resolve(ac_a).unwrap()).unwrap().object).unwrap().refs;
        assert_eq!(refs1, refs0 + 1, "exactly one reference for the delivered capability");
        // The sender keeps its own handle (copy semantics).
        assert!(r.ipc.rights_of(a, ac_a).is_ok());
        r.check();
        // Closing the receiver's handle releases the reference again.
        r.ipc.close_handle(&mut renv!(r), b, nh).unwrap();
        let refs2 = r.ipc.objects().get_any(r.ipc.caps().peek(r.ipc.procs.get(a).unwrap().resolve(ac_a).unwrap()).unwrap().object).unwrap().refs;
        assert_eq!(refs2, refs0);
        r.check();
    }

    #[test]
    fn unauthorized_transfers_are_refused_without_side_effects() {
        let (mut r, a, b, _c, ab_a, ab_b, ac_a, _ac_c) = rig3();
        let (caps0, objs0) = (r.ipc.caps().live(), r.ipc.objects().live());
        // Rights escalation: PEER_RIGHTS has no CONNECT.
        let m = with(a, MsgFlags::NONE, &[xfer(ac_a, Rights::SEND.union(Rights::CONNECT))]);
        assert_eq!(r.ipc.send(&mut renv!(r), a, ab_a, m), Err(Error::RightsEscalation));
        // Not holding the handle at all (b's handle value used by a; b's table differs).
        let m = with(a, MsgFlags::NONE, &[xfer(Handle::from_raw(0x0001_001F), Rights::SEND)]);
        assert!(r.ipc.send(&mut renv!(r), a, ab_a, m).is_err());
        // Without the TRANSFER right.
        let weak = r.ipc.duplicate_handle(a, ac_a, Rights::SEND).unwrap();
        let m = with(a, MsgFlags::NONE, &[xfer(weak, Rights::SEND)]);
        assert_eq!(r.ipc.send(&mut renv!(r), a, ab_a, m), Err(Error::RightsDenied));
        // One bad entry spoils the whole list: the good one must not stay staged.
        let m = with(
            a,
            MsgFlags::NONE,
            &[xfer(ac_a, Rights::SEND), xfer(weak, Rights::SEND)],
        );
        let before = r.ipc.caps().live();
        assert_eq!(r.ipc.send(&mut renv!(r), a, ab_a, m), Err(Error::RightsDenied));
        assert_eq!(r.ipc.caps().live(), before);
        assert_eq!(r.ipc.pending(b, ab_b), Ok(0), "nothing was queued");
        // A process cannot transfer a handle that belongs to another process.
        let mut b_last = ab_b;
        for _ in 0..6 {
            b_last = r.ipc.duplicate_handle(b, ab_b, Rights::SEND).unwrap();
        }
        let m = with(a, MsgFlags::NONE, &[xfer(b_last, Rights::SEND)]);
        let e = r.ipc.send(&mut renv!(r), a, ab_a, m).unwrap_err();
        assert!(matches!(e, Error::Stale | Error::BadHandle | Error::NotOwner), "{e:?}");
        // (A handle value only means something in the table it was issued in; a value that
        // happens to exist in a's own table would name a's own capability, never b's.)
        assert_eq!((r.ipc.caps().live(), r.ipc.objects().live()), (caps0 + 1 + 6, objs0));
        r.check();
    }

    #[test]
    fn duplicate_handles_in_one_list_are_refused() {
        // Refused at message construction time...
        let h = Handle::from_raw(0x0001_0001);
        let t = xfer(h, Rights::SEND);
        let m = Message::new(p(1), MsgKind::Data, MsgFlags::NONE, 0, b"").unwrap();
        assert_eq!(m.with_transfers(&[t, t]).err(), Some(Error::DuplicateTransfer));
        // ...and the staging pass would also catch two handles that resolve to one capability.
    }

    #[test]
    fn destination_exhaustion_keeps_the_message_and_changes_nothing() {
        let (mut r, a, b, _c, ab_a, ab_b, ac_a, _ac_c) = rig3();
        // Fill b's handle table completely.
        loop {
            match r.ipc.duplicate_handle(b, ab_b, Rights::SEND) {
                Ok(_) => {}
                Err(Error::HandlesExhausted) => break,
                Err(e) => panic!("{e:?}"),
            }
        }
        let m = with(a, MsgFlags::NONE, &[xfer(ac_a, Rights::SEND)]);
        r.ipc.send(&mut renv!(r), a, ab_a, m).unwrap();
        let snapshot = (r.ipc.caps().live(), r.ipc.objects().live(), r.ipc.stats());
        for _ in 0..3 {
            assert_eq!(r.ipc.recv(&mut renv!(r), b, ab_b).err(), Some(Error::DestinationFull));
        }
        assert_eq!((r.ipc.caps().live(), r.ipc.objects().live()), (snapshot.0, snapshot.1));
        assert_eq!(r.ipc.stats().received, snapshot.2.received);
        assert_eq!(r.ipc.pending(b, ab_b), Ok(1), "still queued");
        r.check();
        // Make room: the same message is now delivered.
        let victim = r.ipc.procs.get(b).unwrap().stored_caps().count();
        assert!(victim > 0);
        let any = (1..=40u32)
            .map(|s| Handle::from_raw(0x0001_0000 | s))
            .find(|h| r.ipc.rights_of(b, *h) == Ok(Rights::SEND))
            .unwrap();
        r.ipc.close_handle(&mut renv!(r), b, any).unwrap();
        let got = r.ipc.recv(&mut renv!(r), b, ab_b).unwrap();
        assert_eq!(got.transfers().len(), 1);
        r.check();
    }

    #[test]
    fn capability_table_exhaustion_rolls_back_staging() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        let (xa, _xb) = r.pair(a, a);
        let (ya, _yb) = r.pair(a, a);
        // Fill the capability table with duplicates held by other processes until exactly one
        // slot is left; a two-entry transfer needs two.
        let capacity = r.ipc.caps().capacity();
        let mut n = 100u64;
        while r.ipc.caps().live() + 1 < capacity {
            let o = r.proc(n);
            n += 1;
            let (base, _other) = r.pair(o, o);
            while r.ipc.caps().live() + 1 < capacity {
                if r.ipc.duplicate_handle(o, base, Rights::SEND).is_err() {
                    break;
                }
            }
        }
        assert_eq!(r.ipc.caps().capacity() - r.ipc.caps().live(), 1);
        let m = with(a, MsgFlags::NONE, &[xfer(xa, Rights::SEND), xfer(ya, Rights::SEND)]);
        let before = (r.ipc.caps().live(), r.ipc.objects().live());
        assert_eq!(r.ipc.send(&mut renv!(r), a, ha, m), Err(Error::CapsExhausted));
        assert_eq!((r.ipc.caps().live(), r.ipc.objects().live()), before, "rolled back");
        assert_eq!(r.ipc.pending(b, hb), Ok(0), "nothing queued");
        assert_eq!(r.ipc.stats().transfers_rolled_back, 1);
        r.check();
        // A single-entry transfer still fits.
        let m = with(a, MsgFlags::NONE, &[xfer(xa, Rights::SEND)]);
        r.ipc.send(&mut renv!(r), a, ha, m).unwrap();
        r.check();
    }

    #[test]
    fn revoking_a_capability_in_flight_makes_delivery_fail_cleanly() {
        let mut r = Rig::new();
        let (srv, cli) = (r.proc(1), r.proc(2));
        let (hs, hc) = r.pair(srv, cli);
        let hl = r.ipc.create_listener(&mut renv!(r), srv).unwrap();
        let connect = r
            .ipc
            .duplicate_handle(srv, hl, Rights::CONNECT.union(Rights::TRANSFER))
            .unwrap();
        let m = with(srv, MsgFlags::NONE, &[xfer(connect, Rights::CONNECT)]);
        r.ipc.send(&mut renv!(r), srv, hs, m).unwrap();
        r.ipc.send(&mut renv!(r), srv, hs, msg(srv, 2, b"after")).unwrap();
        // The server revokes everything derived from its listener capability.
        assert_eq!(r.ipc.revoke_derived(&mut renv!(r), srv, hl), Ok(2), "the duplicate and the in-flight cap");
        r.check();
        let before = r.ipc.caps().live();
        assert_eq!(r.ipc.recv(&mut renv!(r), cli, hc).err(), Some(Error::TransferRevoked));
        assert_eq!(r.ipc.caps().live(), before, "no authority was installed");
        assert_eq!(r.ipc.stats().transfers_revoked, 1);
        // The queue is not jammed: the next message is deliverable.
        assert_eq!(r.ipc.recv(&mut renv!(r), cli, hc).unwrap().payload(), b"after");
        r.check();
    }

    #[test]
    fn revoking_before_sending_is_refused() {
        let mut r = Rig::new();
        let (srv, cli) = (r.proc(1), r.proc(2));
        let (hs, _hc) = r.pair(srv, cli);
        let hl = r.ipc.create_listener(&mut renv!(r), srv).unwrap();
        let connect = r
            .ipc
            .duplicate_handle(srv, hl, Rights::CONNECT.union(Rights::TRANSFER))
            .unwrap();
        r.ipc.revoke_derived(&mut renv!(r), srv, hl).unwrap();
        let m = with(srv, MsgFlags::NONE, &[xfer(connect, Rights::CONNECT)]);
        assert_eq!(r.ipc.send(&mut renv!(r), srv, hs, m), Err(Error::Stale));
        r.check();
    }

    #[test]
    fn closing_the_object_while_its_capability_is_in_flight_revokes_the_transfer() {
        let (mut r, a, b, _c, ab_a, ab_b, ac_a, _ac_c) = rig3();
        let m = with(a, MsgFlags::NONE, &[xfer(ac_a, Rights::SEND)]);
        r.ipc.send(&mut renv!(r), a, ab_a, m).unwrap();
        // a closes the endpoint for everybody while the capability is queued.
        r.ipc.close_endpoint(&mut renv!(r), a, ac_a).unwrap();
        r.check();
        assert_eq!(r.ipc.recv(&mut renv!(r), b, ab_b).err(), Some(Error::TransferRevoked));
        r.check();
    }

    #[test]
    fn object_survives_while_only_the_message_references_it() {
        let (mut r, a, b, c, ab_a, ab_b, ac_a, ac_c) = rig3();
        let m = with(a, MsgFlags::NONE, &[xfer(ac_a, Rights::SEND.union(Rights::RECV))]);
        r.ipc.send(&mut renv!(r), a, ab_a, m).unwrap();
        // The sender drops its own handle; the in-flight capability keeps the endpoint alive.
        r.ipc.close_handle(&mut renv!(r), a, ac_a).unwrap();
        assert_eq!(r.ipc.peer_open(c, ac_c), Ok(true));
        r.check();
        let got = r.ipc.recv(&mut renv!(r), b, ab_b).unwrap();
        let nh = Handle::from_raw(got.transfers()[0].handle);
        r.ipc.send(&mut renv!(r), c, ac_c, msg(c, 1, b"still here")).unwrap();
        assert_eq!(r.ipc.recv(&mut renv!(r), b, nh).unwrap().payload(), b"still here");
        r.check();
    }

    #[test]
    fn move_closes_the_senders_handle_only_after_queueing() {
        let (mut r, a, b, _c, ab_a, ab_b, ac_a, _ac_c) = rig3();
        let m = with(a, MsgFlags::MOVE_CAPS, &[xfer(ac_a, Rights::SEND)]);
        r.ipc.send(&mut renv!(r), a, ab_a, m).unwrap();
        assert_eq!(r.ipc.rights_of(a, ac_a).err(), Some(Error::Stale), "moved: sender lost it");
        let got = r.ipc.recv(&mut renv!(r), b, ab_b).unwrap();
        assert!(r.ipc.rights_of(b, Handle::from_raw(got.transfers()[0].handle)).is_ok());
        r.check();
        // A failing send does not close anything.
        let (ha2, _hb2) = r.pair(a, b);
        for i in 0..crate::d6::ipc::QUEUE_CAPACITY {
            r.ipc.send(&mut renv!(r), a, ha2, msg(a, i as u64, b"")).unwrap();
        }
        let (xa, _xb) = r.pair(a, a);
        let m = with(a, MsgFlags::MOVE_CAPS, &[xfer(xa, Rights::SEND)]);
        assert_eq!(r.ipc.send(&mut renv!(r), a, ha2, m), Err(Error::QueueFull));
        assert!(r.ipc.rights_of(a, xa).is_ok(), "full queue: the handle stays open");
        r.check();
    }

    #[test]
    fn transfer_of_an_endpoint_through_itself_and_to_oneself_is_consistent() {
        let mut r = Rig::new();
        let a = r.proc(1);
        let (h1, h2) = r.create_channel_for(a);
        // Send h1 through h1 to the peer end h2 (loopback); receive it back into a's own table.
        let m = with(a, MsgFlags::NONE, &[xfer(h1, Rights::SEND)]);
        r.ipc.send(&mut renv!(r), a, h1, m).unwrap();
        let got = r.ipc.recv(&mut renv!(r), a, h2).unwrap();
        assert_eq!(got.transfers().len(), 1);
        r.check();
    }

    impl Rig {
        fn create_channel_for(&mut self, a: Owner) -> (Handle, Handle) {
            let mut env = crate::d6::ipc_wait::Env { sched: &mut self.sched, sync: &mut self.sync };
            self.ipc.create_channel(&mut env, a).unwrap()
        }
    }
}
