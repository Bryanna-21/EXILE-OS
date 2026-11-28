//! Resuming blocked IPC syscalls (Mission B4). SPDX-License-Identifier: Apache-2.0
//!
//! When `ipc_abi::handle` answers [`Abi::Blocked`], the syscall is not finished: the user buffer
//! of a receive/wait still has to be written and RAX still has to be produced. The facts needed
//! for that (which operation, which user buffer, which process) are recorded in a
//! [`PendingTable`] -- one slot per scheduler thread slot, protected by the same lock as the
//! IPC domain (see `kernel_domain::IpcState`), and armed in the same critical section that
//! blocked the thread, so there is no window where a thread is blocked with nothing recorded.
//!
//! [`finish_blocked`] runs on the woken thread. It
//! 1. takes the pending record of *this* thread (never anyone else's),
//! 2. asks the domain what the operation ended with (`IpcDomain::resume`),
//! 3. calls the updated `ipc_abi::complete`, which writes the *original* user buffer and
//!    returns the value for RAX.
//!
//! A pending record is removed exactly once: by `finish_blocked` (completion), or by
//! `cancel`/`cancel_owner` (thread or process teardown). Every removal is reported through
//! counters or an error; nothing is dropped silently. A protocol failure (no record, record of
//! another process) never leaves the domain's own wait/result record behind: the result is
//! collected and any capability it carries is closed again.
#![allow(dead_code)]

use super::ipc::IpcDomain;
use super::ipc_abi::{self, Pending};
use super::ipc_service::KernelIpcService;
use crate::sched::Scheduler;
use crate::thread::Handle;
use crate::usermode::UserMemory;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeError {
    /// The thread has no pending operation (already completed or cancelled).
    NoPending,
    /// The thread already has a pending operation; a second one is never armed.
    AlreadyPending,
    /// The pending operation was recorded for a different process.
    OwnerMismatch,
}

/// Permission to arm the pending record of one thread; see [`PendingTable::begin`].
#[must_use = "a ticket must be used to arm the pending record"]
pub struct Ticket {
    h: Handle,
}

#[derive(Clone, Copy)]
struct Entry {
    handle: Handle,
    owner: u64,
    pending: Pending,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PendingStats {
    pub armed: u64,
    pub completed: u64,
    pub cancelled: u64,
    /// Records of dead thread incarnations found (and removed) in a reused slot.
    pub stale_dropped: u64,
}

pub struct PendingTable<const N: usize> {
    slots: [Option<Entry>; N],
    stats: PendingStats,
}

impl<const N: usize> Default for PendingTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> PendingTable<N> {
    pub const fn new() -> Self {
        Self {
            slots: [None; N],
            stats: PendingStats {
                armed: 0,
                completed: 0,
                cancelled: 0,
                stale_dropped: 0,
            },
        }
    }

    /// Number of pending operations.
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn stats(&self) -> PendingStats {
        self.stats
    }

    pub fn is_pending(&self, h: Handle) -> bool {
        self.slots[h.index()].is_some_and(|e| e.handle == h)
    }

    pub fn pending_of(&self, h: Handle) -> Option<Pending> {
        self.slots[h.index()]
            .filter(|e| e.handle == h)
            .map(|e| e.pending)
    }

    /// To be called before a thread starts a syscall that may block. A record of a *previous*
    /// thread in the same slot is dropped (counted); a live record of `h` itself is refused.
    /// The returned [`Ticket`] proves the slot is free and is the only way to [`arm`](Self::arm),
    /// so arming cannot fail after the thread has already been blocked.
    pub fn begin(&mut self, h: Handle) -> Result<Ticket, ResumeError> {
        match self.slots[h.index()] {
            Some(e) if e.handle == h => return Err(ResumeError::AlreadyPending),
            Some(_) => {
                self.slots[h.index()] = None;
                self.stats.stale_dropped += 1;
            }
            None => {}
        }
        Ok(Ticket { h })
    }

    /// Records the pending operation of the thread that was just blocked. Infallible: the
    /// ticket guarantees the slot is free (the table is only reachable under the domain lock,
    /// and `begin` and `arm` run in the same critical section).
    pub fn arm(&mut self, t: Ticket, owner: u64, pending: Pending) {
        let i = t.h.index();
        debug_assert!(self.slots[i].is_none(), "ticket for an occupied slot");
        if self.slots[i].is_some() {
            self.stats.stale_dropped += 1;
        }
        self.slots[i] = Some(Entry {
            handle: t.h,
            owner,
            pending,
        });
        self.stats.armed += 1;
    }

    /// Removes and returns the pending operation of `h`, which must belong to `owner`.
    pub fn take(&mut self, h: Handle, owner: u64) -> Result<Pending, ResumeError> {
        match self.slots[h.index()] {
            Some(e) if e.handle == h => {
                self.slots[h.index()] = None;
                if e.owner != owner {
                    self.stats.cancelled += 1;
                    return Err(ResumeError::OwnerMismatch);
                }
                self.stats.completed += 1;
                Ok(e.pending)
            }
            _ => Err(ResumeError::NoPending),
        }
    }

    /// Removes the pending operation of `h` (thread teardown). Returns it if there was one.
    pub fn cancel(&mut self, h: Handle) -> Option<Pending> {
        let e = self.slots[h.index()].filter(|e| e.handle == h)?;
        self.slots[h.index()] = None;
        self.stats.cancelled += 1;
        Some(e.pending)
    }

    /// Removes every pending operation recorded for `owner` (process teardown).
    pub fn cancel_owner(&mut self, owner: u64) -> usize {
        let mut n = 0;
        for s in self.slots.iter_mut() {
            if s.is_some_and(|e| e.owner == owner) {
                *s = None;
                n += 1;
            }
        }
        self.stats.cancelled += n as u64;
        n
    }

    /// Removes everything (subsystem shutdown).
    pub fn clear(&mut self) -> usize {
        let n = self.len();
        self.slots = [None; N];
        self.stats.cancelled += n as u64;
        n
    }
}

/// Finishes a blocked syscall of thread `me` (process `owner`), which has been scheduled again.
/// Returns the RAX value, or a protocol error if no matching pending operation exists.
pub fn finish_blocked<
    const N: usize,
    const O: usize,
    const C: usize,
    const Q: usize,
    M: UserMemory,
>(
    domain: &mut IpcDomain<N, O, C, Q>,
    pending: &mut PendingTable<N>,
    sched: &mut Scheduler<N>,
    mem: &mut M,
    me: Handle,
    owner: u64,
) -> Result<i64, ResumeError> {
    let p = match pending.take(me, owner) {
        Ok(p) => p,
        Err(e) => {
            // Collect whatever the domain holds for this thread so nothing is left behind, and
            // give back a capability that was delivered to nobody.
            if let Ok(super::ipc::Resume::Received(d)) = domain.resume(me)
                && let Some(c) = d.cap
            {
                let _ = domain.close(sched, owner, c);
            }
            return Err(e);
        }
    };
    let r = domain.resume(me);
    let mut svc = KernelIpcService::new(domain, sched, me, owner);
    Ok(ipc_abi::complete(&mut svc, mem, p, r))
}

#[cfg(test)]
mod tests {
    use super::super::cap_rights::Rights;
    use super::super::ipc::{IpcError, RecvOutcome, SendOutcome, WaitOutcome};
    use super::super::ipc_message::{Delivery, UserMessage, WIRE_OUT};
    use super::*;
    use crate::thread::Priority;
    use crate::usermode::SyscallError;
    extern crate std;
    use std::vec::Vec;

    type S = Scheduler<8>;
    type D = IpcDomain<8, 8, 32, 2>;
    const A: u64 = 1;
    const B: u64 = 2;
    const OUT: u64 = 0x10000;

    struct Mem {
        bytes: Vec<u8>,
        fail_copy: bool,
    }

    impl Mem {
        fn new() -> Self {
            Mem {
                bytes: std::vec![0xEE; 0x1000],
                fail_copy: false,
            }
        }
    }

    impl UserMemory for Mem {
        fn check(&self, s: u64, l: u64, _w: bool) -> Result<(), SyscallError> {
            if s >= OUT && s + l <= OUT + 0x1000 {
                Ok(())
            } else {
                Err(SyscallError::Fault)
            }
        }
        fn copy_from_user(&self, s: u64, d: &mut [u8]) -> Result<(), SyscallError> {
            self.check(s, d.len() as u64, false)?;
            let o = (s - OUT) as usize;
            d.copy_from_slice(&self.bytes[o..o + d.len()]);
            Ok(())
        }
        fn copy_to_user(&mut self, d: u64, s: &[u8]) -> Result<(), SyscallError> {
            self.check(d, s.len() as u64, true)?;
            if self.fail_copy {
                return Err(SyscallError::Fault);
            }
            let o = (d - OUT) as usize;
            self.bytes[o..o + s.len()].copy_from_slice(s);
            Ok(())
        }
    }

    struct W {
        s: S,
        d: D,
        p: PendingTable<8>,
        t: [Handle; 2],
    }

    fn world() -> W {
        let mut s = S::new();
        s.init_idle(0, 0).unwrap();
        let t = [
            s.spawn(Priority::new(4).unwrap(), 0, 0).unwrap(),
            s.spawn(Priority::new(4).unwrap(), 0, 0).unwrap(),
        ];
        W {
            s,
            d: D::new(),
            p: PendingTable::new(),
            t,
        }
    }

    impl W {
        fn run_until(&mut self, h: Handle) {
            for _ in 0..30 {
                if self.s.current() == Some(h) {
                    return;
                }
                let sw = self.s.schedule().unwrap();
                if sw.is_switch()
                    && let Some(p) = sw.prev
                {
                    self.s.switch_complete(p).unwrap();
                }
            }
            panic!("never current");
        }

        /// Thread `i` of owner `o` blocks receiving on `ep` and records its pending op.
        fn block_recv(&mut self, i: usize, o: u64, ep: u64) {
            self.run_until(self.t[i]);
            let tk = self.p.begin(self.t[i]).unwrap();
            assert_eq!(
                self.d.recv(&mut self.s, o, ep, true),
                Ok(RecvOutcome::Blocked)
            );
            self.p.arm(tk, o, Pending::Recv { out: OUT });
        }

        fn finish(&mut self, mem: &mut Mem, i: usize, o: u64) -> Result<i64, ResumeError> {
            self.run_until(self.t[i]);
            finish_blocked(&mut self.d, &mut self.p, &mut self.s, mem, self.t[i], o)
        }
    }

    fn out_of(m: &Mem) -> Delivery {
        let raw: [u8; WIRE_OUT] = m.bytes[..WIRE_OUT].try_into().unwrap();
        Delivery::decode(&raw).unwrap()
    }

    #[test]
    fn table_arm_take_cancel_and_counters() {
        let w = world();
        let mut p = PendingTable::<8>::new();
        let (a, b) = (w.t[0], w.t[1]);
        assert!(p.is_empty());
        let t = p.begin(a).unwrap();
        p.arm(t, A, Pending::Send);
        assert!(matches!(p.begin(a), Err(ResumeError::AlreadyPending)));
        assert!(p.is_pending(a) && !p.is_pending(b));
        assert_eq!(p.pending_of(a), Some(Pending::Send));
        assert_eq!(p.take(b, A), Err(ResumeError::NoPending));
        assert_eq!(p.take(a, A), Ok(Pending::Send));
        assert_eq!(p.take(a, A), Err(ResumeError::NoPending), "only once");
        let t = p.begin(a).unwrap();
        p.arm(t, A, Pending::Wait { out: 8 });
        let t = p.begin(b).unwrap();
        p.arm(t, B, Pending::Send);
        assert_eq!(p.cancel_owner(A), 1);
        assert!(!p.is_pending(a) && p.is_pending(b));
        assert_eq!(p.cancel(b), Some(Pending::Send));
        assert_eq!(p.cancel(b), None);
        let t = p.begin(a).unwrap();
        p.arm(t, A, Pending::Send);
        assert_eq!(p.clear(), 1);
        let st = p.stats();
        assert_eq!((st.armed, st.completed, st.cancelled), (4, 1, 3));
    }

    #[test]
    fn take_refuses_another_process_and_still_removes_the_record() {
        let w = world();
        let mut p = PendingTable::<8>::new();
        let t = p.begin(w.t[0]).unwrap();
        p.arm(t, A, Pending::Send);
        assert_eq!(p.take(w.t[0], B), Err(ResumeError::OwnerMismatch));
        assert!(p.is_empty(), "an unusable record must not linger");
    }

    #[test]
    fn begin_drops_a_record_left_by_a_dead_thread_in_a_reused_slot() {
        let mut w = world();
        let old = w.t[0];
        let t = w.p.begin(old).unwrap();
        w.p.arm(t, A, Pending::Send);
        w.s.kill(old, 0).unwrap(); // never ran: killed while Ready
        w.run_until(w.t[1]);
        w.s.reap(old).unwrap();
        let new = w.s.spawn(Priority::new(4).unwrap(), 0, 0).unwrap();
        assert_eq!(new.index(), old.index(), "slot reused");
        assert_ne!(new, old);
        assert!(w.p.begin(new).is_ok());
        assert_eq!(w.p.stats().stale_dropped, 1);
        assert!(w.p.is_empty());
        assert_eq!(w.p.take(new, A), Err(ResumeError::NoPending));
    }

    #[test]
    fn blocked_receive_is_completed_into_the_original_buffer() {
        let mut w = world();
        let mut mem = Mem::new();
        let ep = w.d.create_endpoint(A).unwrap();
        w.block_recv(0, A, ep);
        w.run_until(w.t[1]);
        let m = UserMessage::new(7, &[1, 2], None).unwrap();
        // the sender (owner A too, for brevity) hands the message straight to the waiter
        assert_eq!(w.d.send(&mut w.s, A, ep, &m, false), Ok(SendOutcome::Sent));
        assert_eq!(w.finish(&mut mem, 0, A), Ok(0));
        let d = out_of(&mem);
        assert_eq!((d.label, d.data()), (7, &[1, 2][..]));
        assert!(w.p.is_empty());
        assert_eq!(w.d.stats().handoffs, 1);
        assert_eq!(w.d.waiting_threads(), 0);
    }

    #[test]
    fn blocked_send_and_wait_complete_with_zero() {
        let mut w = world();
        let mut mem = Mem::new();
        let ep = w.d.create_endpoint(A).unwrap();
        let m = UserMessage::new(1, &[], None).unwrap();
        w.run_until(w.t[0]);
        for _ in 0..2 {
            assert_eq!(w.d.send(&mut w.s, A, ep, &m, false), Ok(SendOutcome::Sent));
        }
        let tk = w.p.begin(w.t[0]).unwrap();
        assert_eq!(
            w.d.send(&mut w.s, A, ep, &m, true),
            Ok(SendOutcome::Blocked)
        );
        w.p.arm(tk, A, Pending::Send);
        w.run_until(w.t[1]);
        assert!(matches!(
            w.d.recv(&mut w.s, A, ep, false),
            Ok(RecvOutcome::Received(_))
        ));
        assert_eq!(w.finish(&mut mem, 0, A), Ok(0));
        // wait: blocks, then a signal completes it and the word lands in the user buffer
        let nf = w.d.create_notification(A).unwrap();
        w.run_until(w.t[0]);
        let tk = w.p.begin(w.t[0]).unwrap();
        assert_eq!(w.d.wait(&mut w.s, A, nf, true), Ok(WaitOutcome::Blocked));
        w.p.arm(tk, A, Pending::Wait { out: OUT + 0x100 });
        w.run_until(w.t[1]);
        w.d.signal(&mut w.s, A, nf, 0b101).unwrap();
        assert_eq!(w.finish(&mut mem, 0, A), Ok(0));
        assert_eq!(&mem.bytes[0x100..0x108], &5u64.to_le_bytes());
    }

    #[test]
    fn interruption_reports_eintr_and_leaves_the_buffer_alone() {
        let mut w = world();
        let mut mem = Mem::new();
        let ep = w.d.create_endpoint(A).unwrap();
        w.block_recv(0, A, ep);
        w.run_until(w.t[1]);
        w.s.wake(w.t[0]).unwrap(); // woken by something that is not IPC
        assert_eq!(w.finish(&mut mem, 0, A), Ok(super::ipc_abi::EINTR));
        assert!(mem.bytes.iter().all(|&b| b == 0xEE), "buffer untouched");
        assert_eq!(w.d.waiting_threads(), 0, "wait record removed");
        assert!(w.p.is_empty());
        w.d.check_invariants(&w.s).unwrap();
    }

    #[test]
    fn destroyed_object_reports_epipe() {
        let mut w = world();
        let mut mem = Mem::new();
        let ep = w.d.create_endpoint(A).unwrap();
        w.block_recv(0, A, ep);
        w.run_until(w.t[1]);
        w.d.close(&mut w.s, A, ep).unwrap();
        assert_eq!(w.finish(&mut mem, 0, A), Ok(super::ipc_abi::EPIPE));
    }

    #[test]
    fn failed_copy_out_closes_the_delivered_capability() {
        let mut w = world();
        let mut mem = Mem::new();
        let ep = w.d.create_endpoint(A).unwrap();
        let ep_b = w.d.grant(A, ep, B, Rights::RECV).unwrap();
        let nf = w.d.create_notification(A).unwrap();
        let before = w.d.caps().owner_count(B);
        w.block_recv(0, B, ep_b);
        w.run_until(w.t[1]);
        let m = UserMessage::new(1, &[], Some(nf)).unwrap();
        w.d.send(&mut w.s, A, ep, &m, false).unwrap();
        assert_eq!(
            w.d.caps().owner_count(B),
            before + 1,
            "handed over at send time"
        );
        mem.fail_copy = true;
        assert_eq!(w.finish(&mut mem, 0, B), Ok(SyscallError::Fault as i64));
        assert_eq!(
            w.d.caps().owner_count(B),
            before,
            "the capability that could not be handed over was closed again"
        );
        assert_eq!(w.d.caps().limbo_count(), 0);
        w.d.check_invariants(&w.s).unwrap();
    }

    #[test]
    fn no_pending_record_is_a_protocol_error_that_cleans_up() {
        let mut w = world();
        let mut mem = Mem::new();
        let ep = w.d.create_endpoint(A).unwrap();
        let ep_b = w.d.grant(A, ep, B, Rights::RECV).unwrap();
        let nf = w.d.create_notification(A).unwrap();
        // blocked, but nobody armed a pending record
        w.run_until(w.t[0]);
        assert_eq!(w.d.recv(&mut w.s, B, ep_b, true), Ok(RecvOutcome::Blocked));
        w.run_until(w.t[1]);
        let m = UserMessage::new(1, &[], Some(nf)).unwrap();
        w.d.send(&mut w.s, A, ep, &m, false).unwrap();
        let before = w.d.caps().owner_count(B);
        assert_eq!(w.finish(&mut mem, 0, B), Err(ResumeError::NoPending));
        assert_eq!(
            w.d.caps().owner_count(B),
            before - 1,
            "delivered cap closed"
        );
        assert!(mem.bytes.iter().all(|&b| b == 0xEE));
        w.d.check_invariants(&w.s).unwrap();
    }

    #[test]
    fn a_pending_record_of_another_process_is_never_completed() {
        let mut w = world();
        let mut mem = Mem::new();
        let ep = w.d.create_endpoint(A).unwrap();
        w.block_recv(0, A, ep);
        w.run_until(w.t[1]);
        let m = UserMessage::new(1, &[], None).unwrap();
        w.d.send(&mut w.s, A, ep, &m, false).unwrap();
        assert_eq!(w.finish(&mut mem, 0, B), Err(ResumeError::OwnerMismatch));
        assert!(mem.bytes.iter().all(|&b| b == 0xEE));
        assert!(w.p.is_empty());
    }

    #[test]
    fn a_mismatched_wake_result_is_refused_not_guessed() {
        let mut w = world();
        let mut mem = Mem::new();
        // a Send record whose domain result is a delivery can only come from corrupt state
        let ep = w.d.create_endpoint(A).unwrap();
        w.block_recv(0, A, ep);
        w.p.cancel(w.t[0]);
        let tk = w.p.begin(w.t[0]).unwrap();
        w.p.arm(tk, A, Pending::Send);
        w.run_until(w.t[1]);
        let m = UserMessage::new(1, &[], None).unwrap();
        w.d.send(&mut w.s, A, ep, &m, false).unwrap();
        assert_eq!(w.finish(&mut mem, 0, A), Ok(super::ipc_abi::EIO));
        let _ = IpcError::Closed;
    }
}
