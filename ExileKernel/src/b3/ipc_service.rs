//! Kernel implementation of `IpcService` (Mission B4). SPDX-License-Identifier: Apache-2.0
//!
//! [`KernelIpcService`] is the `ipc_abi::IpcService` the syscall layer talks to. It is a thin,
//! *checked* view of the one authoritative [`IpcDomain`]: it borrows the domain and the
//! scheduler for the duration of one critical section and binds every call to one calling
//! thread and the process (capability owner) that thread belongs to.
//!
//! # Ownership rules enforced here
//! * The owner id is never taken from user space. [`KernelIpcService::for_thread`] derives it
//!   from the process table ([`OwnerResolver`]): the thread must belong to a live process whose
//!   id is a valid capability owner.
//! * The bound thread must be the scheduler's current, non-idle thread. Any call made on behalf
//!   of a thread that is not running (a stale service value, a woken-but-not-yet-running
//!   thread) fails with `NoCurrent`/`IsIdle` before the domain is touched.
//! * Every handle goes through the domain with this owner, so a handle of another process fails
//!   exactly like a forged one (`BadHandle`); a capability sent in a message must be held with
//!   `GRANT` and is attenuated into limbo by the domain (see `ipc.rs`).
#![allow(dead_code)]

use super::cap_rights::Rights;
use super::cap_space::LIMBO;
use super::ipc::{IpcDomain, IpcError, RecvOutcome, SendOutcome, WaitOutcome};
use super::ipc_abi::IpcService;
use super::ipc_message::UserMessage;
use crate::process::{ProcState, ProcessTable};
use crate::sched::Scheduler;
use crate::thread::Handle;

/// Maps a scheduler thread to the capability owner (process id) it runs for.
pub trait OwnerResolver {
    /// `None` if the thread belongs to no process, or its process is exiting.
    fn owner_of_thread(&self, t: Handle) -> Option<u64>;
}

impl<const P: usize> OwnerResolver for ProcessTable<P> {
    fn owner_of_thread(&self, t: Handle) -> Option<u64> {
        let h = self.owner_of(t)?;
        if !matches!(
            self.state(h),
            Ok(ProcState::Creating) | Ok(ProcState::Running)
        ) {
            return None;
        }
        let pid = self.pid(h).ok()?.0;
        (pid != LIMBO && pid != 0).then_some(pid)
    }
}

pub struct KernelIpcService<'a, const N: usize, const O: usize, const C: usize, const Q: usize> {
    dom: &'a mut IpcDomain<N, O, C, Q>,
    sched: &'a mut Scheduler<N>,
    thread: Handle,
    owner: u64,
}

impl<'a, const N: usize, const O: usize, const C: usize, const Q: usize>
    KernelIpcService<'a, N, O, C, Q>
{
    /// Binds the service to `thread` acting for `owner`. `owner` must already be trusted (it
    /// came from [`OwnerResolver`] or from kernel code); user space never supplies it.
    pub fn new(
        dom: &'a mut IpcDomain<N, O, C, Q>,
        sched: &'a mut Scheduler<N>,
        thread: Handle,
        owner: u64,
    ) -> Self {
        Self {
            dom,
            sched,
            thread,
            owner,
        }
    }

    /// Resolves the owner of `thread` through the process table. `thread` must be the running,
    /// non-idle thread.
    pub fn for_thread<R: OwnerResolver>(
        dom: &'a mut IpcDomain<N, O, C, Q>,
        sched: &'a mut Scheduler<N>,
        resolver: &R,
        thread: Handle,
    ) -> Result<Self, IpcError> {
        Self::check_running(sched, thread)?;
        let owner = resolver.owner_of_thread(thread).ok_or(IpcError::BadOwner)?;
        Ok(Self::new(dom, sched, thread, owner))
    }

    pub fn owner(&self) -> u64 {
        self.owner
    }

    pub fn thread(&self) -> Handle {
        self.thread
    }

    fn check_running(sched: &Scheduler<N>, thread: Handle) -> Result<(), IpcError> {
        if sched.current() != Some(thread) {
            return Err(IpcError::NoCurrent);
        }
        if sched.thread(thread)?.is_idle() {
            return Err(IpcError::IsIdle);
        }
        Ok(())
    }

    fn guard(&self) -> Result<(), IpcError> {
        if self.owner == LIMBO {
            return Err(IpcError::BadOwner);
        }
        Self::check_running(self.sched, self.thread)
    }
}

impl<const N: usize, const O: usize, const C: usize, const Q: usize> IpcService
    for KernelIpcService<'_, N, O, C, Q>
{
    fn create_endpoint(&mut self) -> Result<u64, IpcError> {
        self.guard()?;
        self.dom.create_endpoint(self.owner)
    }

    fn create_notification(&mut self) -> Result<u64, IpcError> {
        self.guard()?;
        self.dom.create_notification(self.owner)
    }

    fn send(&mut self, ep: u64, m: &UserMessage, block: bool) -> Result<SendOutcome, IpcError> {
        self.guard()?;
        self.dom.send(self.sched, self.owner, ep, m, block)
    }

    fn recv(&mut self, ep: u64, block: bool) -> Result<RecvOutcome, IpcError> {
        self.guard()?;
        self.dom.recv(self.sched, self.owner, ep, block)
    }

    fn signal(&mut self, nf: u64, bits: u64) -> Result<(), IpcError> {
        self.guard()?;
        self.dom.signal(self.sched, self.owner, nf, bits)
    }

    fn wait(&mut self, nf: u64, block: bool) -> Result<WaitOutcome, IpcError> {
        self.guard()?;
        self.dom.wait(self.sched, self.owner, nf, block)
    }

    fn derive(&mut self, h: u64, mask: Rights) -> Result<u64, IpcError> {
        self.guard()?;
        self.dom.derive(self.owner, h, mask)
    }

    fn close(&mut self, h: u64) -> Result<(), IpcError> {
        self.guard()?;
        self.dom.close(self.sched, self.owner, h)
    }

    fn revoke(&mut self, h: u64) -> Result<u64, IpcError> {
        self.guard()?;
        self.dom.revoke(self.sched, self.owner, h).map(|n| n as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread::Priority;

    type S = Scheduler<4>;
    type D = IpcDomain<4, 4, 16, 2>;

    struct Fixed(Handle, u64);
    impl OwnerResolver for Fixed {
        fn owner_of_thread(&self, t: Handle) -> Option<u64> {
            (t == self.0).then_some(self.1)
        }
    }

    fn setup() -> (S, D, [Handle; 2]) {
        let mut s = S::new();
        s.init_idle(0, 0).unwrap();
        let a = s.spawn(Priority::new(4).unwrap(), 0, 0).unwrap();
        let b = s.spawn(Priority::new(4).unwrap(), 0, 0).unwrap();
        (s, D::new(), [a, b])
    }

    fn run_until(s: &mut S, h: Handle) {
        for _ in 0..20 {
            if s.current() == Some(h) {
                return;
            }
            let sw = s.schedule().unwrap();
            if sw.is_switch()
                && let Some(p) = sw.prev
            {
                s.switch_complete(p).unwrap();
            }
        }
        panic!("never current");
    }

    #[test]
    fn a_service_bound_to_a_thread_that_is_not_running_touches_nothing() {
        let (mut s, mut d, hs) = setup();
        run_until(&mut s, hs[0]);
        let mut svc = KernelIpcService::new(&mut d, &mut s, hs[1], 1);
        assert_eq!(svc.create_endpoint(), Err(IpcError::NoCurrent));
        assert_eq!(svc.create_notification(), Err(IpcError::NoCurrent));
        assert_eq!(svc.close(1), Err(IpcError::NoCurrent));
        assert_eq!(d.caps().owner_count(1), 0, "no capability was created");
        assert_eq!(d.live_objects(), 0);
    }

    #[test]
    fn the_idle_thread_cannot_make_ipc_calls() {
        let mut s = S::new();
        s.init_idle(0, 0).unwrap();
        let mut d = D::new();
        let idle = s.idle().unwrap();
        let sw = s.schedule().unwrap();
        assert_eq!(sw.next, idle, "nothing else is runnable");
        let mut svc = KernelIpcService::new(&mut d, &mut s, idle, 1);
        assert_eq!(svc.create_endpoint(), Err(IpcError::IsIdle));
    }

    #[test]
    fn nothing_runs_for_the_limbo_owner() {
        let (mut s, mut d, hs) = setup();
        run_until(&mut s, hs[0]);
        let mut svc = KernelIpcService::new(&mut d, &mut s, hs[0], LIMBO);
        assert_eq!(svc.create_endpoint(), Err(IpcError::BadOwner));
        assert_eq!(d.live_objects(), 0);
    }

    #[test]
    fn handles_never_cross_process_boundaries() {
        let (mut s, mut d, hs) = setup();
        run_until(&mut s, hs[0]);
        let ep = KernelIpcService::new(&mut d, &mut s, hs[0], 1)
            .create_endpoint()
            .unwrap();
        let mut other = KernelIpcService::new(&mut d, &mut s, hs[0], 2);
        let m = UserMessage::new(1, &[], None).unwrap();
        assert_eq!(other.send(ep, &m, false), Err(IpcError::BadHandle));
        assert_eq!(other.recv(ep, false), Err(IpcError::BadHandle));
        assert_eq!(other.close(ep), Err(IpcError::BadHandle));
        assert_eq!(other.revoke(ep), Err(IpcError::BadHandle));
        assert_eq!(
            other.derive(ep, Rights::SEND),
            Err(IpcError::BadHandle),
            "cannot derive from another process's capability"
        );
        // the owner still has it, untouched
        let mut mine = KernelIpcService::new(&mut d, &mut s, hs[0], 1);
        assert_eq!(mine.send(ep, &m, false), Ok(SendOutcome::Sent));
    }

    #[test]
    fn for_thread_resolves_the_owner_and_refuses_unknown_threads() {
        let (mut s, mut d, hs) = setup();
        run_until(&mut s, hs[0]);
        assert_eq!(
            KernelIpcService::for_thread(&mut d, &mut s, &Fixed(hs[1], 9), hs[0]).err(),
            Some(IpcError::BadOwner),
            "the running thread belongs to no process"
        );
        let svc = KernelIpcService::for_thread(&mut d, &mut s, &Fixed(hs[0], 9), hs[0]).unwrap();
        assert_eq!((svc.owner(), svc.thread()), (9, hs[0]));
        assert_eq!(
            KernelIpcService::for_thread(&mut d, &mut s, &Fixed(hs[1], 9), hs[1]).err(),
            Some(IpcError::NoCurrent),
            "not the running thread"
        );
    }

    #[test]
    fn capability_transfer_attenuates_and_requires_grant() {
        let (mut s, mut d, hs) = setup();
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(1).unwrap();
        let ep_for_2 = d.grant(1, ep, 2, Rights::RECV).unwrap();
        let nf = d.create_notification(1).unwrap();
        let no_grant = d.derive(1, nf, Rights::SIGNAL).unwrap();
        let narrow = d
            .derive(1, nf, Rights::SIGNAL.union(Rights::GRANT))
            .unwrap();
        let mut svc = KernelIpcService::new(&mut d, &mut s, hs[0], 1);
        // a capability held without GRANT cannot be sent
        let bad = UserMessage::new(1, &[], Some(no_grant)).unwrap();
        assert_eq!(svc.send(ep, &bad, false), Err(IpcError::Perm));
        // with GRANT it travels; the receiver gets a derived (not the same) capability
        let good = UserMessage::new(2, &[], Some(narrow)).unwrap();
        assert_eq!(svc.send(ep, &good, false), Ok(SendOutcome::Sent));
        let _ = svc;
        run_until(&mut s, hs[1]);
        let mut rcv = KernelIpcService::new(&mut d, &mut s, hs[1], 2);
        let RecvOutcome::Received(dl) = rcv.recv(ep_for_2, false).unwrap() else {
            panic!("expected a message");
        };
        let got = dl.cap.expect("capability delivered");
        assert!(!dl.cap_lost);
        // the receiver may use it, but owner 1's handle value is not valid in owner 2's space
        // unless it is the very one delivered
        assert_eq!(rcv.signal(got, 1), Ok(()));
        assert_eq!(
            rcv.wait(got, false),
            Err(IpcError::Perm),
            "WAIT was never granted"
        );
        assert_eq!(rcv.revoke(got), Err(IpcError::Perm), "no REVOKE right");
        assert_eq!(d.caps().limbo_count(), 0);
    }

    #[test]
    fn the_process_table_resolver_maps_live_processes_and_refuses_exiting_ones() {
        use super::super::scheduler_bridge::testkit::ModelPort;
        use super::super::thread_lifecycle::testenv::Env;
        use crate::process::{ExitStatus, Limits};
        use crate::tls::TlsContext;

        let port = ModelPort::<4>::new(&[4, 4, 4]);
        let mut env = Env::new(&port, 32);
        let mut t = ProcessTable::<4>::new();
        let sp = env.fx.space();
        let root = t
            .create(None, Limits::DEFAULT, sp)
            .map_err(|(e, _)| e)
            .unwrap();
        let sp = env.fx.space();
        let p1 = t
            .create(Some(root), Limits::DEFAULT, sp)
            .map_err(|(e, _)| e)
            .unwrap();
        t.add_thread(p1, port.thread(0), None, TlsContext::default())
            .unwrap();
        let pid = t.pid(p1).unwrap().0;
        assert_eq!(t.owner_of_thread(port.thread(0)), Some(pid));
        assert_eq!(
            t.owner_of_thread(port.thread(1)),
            None,
            "a thread that belongs to no process has no owner"
        );
        assert_ne!(pid, LIMBO);
        // the thread is the running one, so exit leaves it (and its record) alive
        port.run_until(port.thread(0));
        t.exit(&mut env, p1, ExitStatus::Code(0)).unwrap();
        assert_eq!(t.state(p1), Ok(ProcState::Exiting));
        assert_eq!(
            t.owner_of(port.thread(0)),
            Some(p1),
            "the thread is still recorded"
        );
        assert_eq!(
            t.owner_of_thread(port.thread(0)),
            None,
            "an exiting process gets no new IPC"
        );
    }
}
