//! IPC cleanup when a thread ends (Mission B4). SPDX-License-Identifier: Apache-2.0
//!
//! A thread can end while it is blocked in an IPC call, while a message or a capability is on
//! its way to it, or while a syscall of it is half done. [`cleanup_thread`] makes sure none of
//! that outlives the thread:
//!
//! * its [`PendingTable`](super::resume::PendingTable) record is removed, so nothing can ever
//!   "complete" a syscall for a dead thread;
//! * its wait record in the domain is removed (a blocked sender's in-flight capability goes
//!   with it) and a capability delivered to it but never collected is released
//!   (`IpcDomain::thread_exit`), and the domain sweeps afterwards, so nobody is left waiting
//!   for it;
//! * it is not woken again: the wait record is gone before anything can select it as a waiter.
//!
//! Capabilities are owned by the *process*, so a thread ending does **not** close anything its
//! process holds; that is `process_lifecycle`'s job when the last thread (or the process
//! itself) goes.
//!
//! [`IpcCleanupEnv`] wraps the kernel's `ProcessEnv` so the cleanup runs on every path that
//! kills a thread (`ProcessTable::exit`, fault exits, kill events). A thread that terminates
//! itself goes through `process_lifecycle::thread_exited`, which calls [`cleanup_thread`] too.
//! [`interrupt_thread`] is the asynchronous interruption: it wakes a thread that is blocked in
//! IPC (it then completes with `EINTR`), and refuses every other thread.
#![allow(dead_code)]

use super::ipc::IpcError;
use super::kernel_domain::{DomainError, IpcKernel};
use super::scheduler_bridge::{self, BridgeError, SchedPort};
use crate::address_space::{FrameAlloc, PhysMem};
use crate::process::{EnvError, ProcessEnv};
use crate::sched::WakeOutcome;
use crate::thread::Handle;

/// What [`cleanup_thread`] found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ThreadCleanup {
    /// The IPC domain is not initialised (nothing can exist to clean up).
    pub inactive: bool,
    /// A pending (blocked) syscall record was removed.
    pub pending_cancelled: bool,
    /// The domain still had a wait record for the thread, now removed.
    pub wait_removed: bool,
}

fn map_domain(e: DomainError) -> IpcError {
    match e {
        DomainError::NotReady | DomainError::AlreadyActive | DomainError::Busy => {
            IpcError::Corrupt("ipc domain unavailable")
        }
    }
}

/// Removes every IPC trace of the (terminated or about to be terminated) thread `h`.
/// Idempotent: a second call finds nothing and reports an empty [`ThreadCleanup`].
pub fn cleanup_thread<
    const N: usize,
    const O: usize,
    const C: usize,
    const Q: usize,
    P: SchedPort<N>,
>(
    ipc: &IpcKernel<'_, N, O, C, Q, P>,
    h: Handle,
) -> Result<ThreadCleanup, IpcError> {
    match ipc.locked(|st, s| {
        let pending_cancelled = st.pending.cancel(h).is_some();
        let wait_removed = st.domain.is_waiting(h);
        st.domain.thread_exit(s, h)?;
        Ok(ThreadCleanup {
            inactive: false,
            pending_cancelled,
            wait_removed,
        })
    }) {
        Ok(r) => r,
        Err(DomainError::NotReady) => Ok(ThreadCleanup {
            inactive: true,
            ..ThreadCleanup::default()
        }),
        Err(e) => Err(map_domain(e)),
    }
}

/// Wakes `h` if it is blocked in IPC; its call then returns `EINTR`. Any other thread (not
/// blocked, blocked elsewhere, already woken) is refused, so a wake-up can neither be
/// duplicated nor disturb a different wait.
pub fn interrupt_thread<
    const N: usize,
    const O: usize,
    const C: usize,
    const Q: usize,
    P: SchedPort<N>,
>(
    ipc: &IpcKernel<'_, N, O, C, Q, P>,
    h: Handle,
) -> Result<WakeOutcome, BridgeError> {
    scheduler_bridge::interrupt_blocked(ipc.port, h)
}

/// A `ProcessEnv` that adds IPC cleanup to `kill_thread`.
///
/// If the cleanup itself fails the kill is reported as `EnvError::Failed`. The process layer
/// then keeps the thread marked live and retries, and the retry finds the thread `Gone`, so the
/// cleanup runs again: nothing is skipped silently.
pub struct IpcCleanupEnv<
    'a,
    E: ProcessEnv,
    const N: usize,
    const O: usize,
    const C: usize,
    const Q: usize,
    P: SchedPort<N>,
> {
    pub inner: &'a mut E,
    pub ipc: &'a IpcKernel<'a, N, O, C, Q, P>,
    /// Number of failed cleanups (diagnostics).
    pub cleanup_failures: u32,
}

impl<
    'a,
    E: ProcessEnv,
    const N: usize,
    const O: usize,
    const C: usize,
    const Q: usize,
    P: SchedPort<N>,
> IpcCleanupEnv<'a, E, N, O, C, Q, P>
{
    pub fn new(inner: &'a mut E, ipc: &'a IpcKernel<'a, N, O, C, Q, P>) -> Self {
        Self {
            inner,
            ipc,
            cleanup_failures: 0,
        }
    }
}

impl<E: ProcessEnv, const N: usize, const O: usize, const C: usize, const Q: usize, P: SchedPort<N>>
    ProcessEnv for IpcCleanupEnv<'_, E, N, O, C, Q, P>
{
    fn phys(&mut self) -> (&mut dyn PhysMem, &mut dyn FrameAlloc) {
        self.inner.phys()
    }
    fn current_thread(&self) -> Option<Handle> {
        self.inner.current_thread()
    }
    fn active_cr3(&self) -> u64 {
        self.inner.active_cr3()
    }
    fn kill_thread(&mut self, h: Handle, status: i32) -> Result<(), EnvError> {
        let r = self.inner.kill_thread(h, status);
        // Gone: the scheduler slot was already reclaimed; its IPC state may still be there.
        if matches!(r, Ok(()) | Err(EnvError::Gone)) && cleanup_thread(self.ipc, h).is_err() {
            self.cleanup_failures += 1;
            return Err(EnvError::Failed);
        }
        r
    }
    fn reap_thread(&mut self, h: Handle) -> Result<(), EnvError> {
        self.inner.reap_thread(h)
    }
    fn free_tls(&mut self, key: u64) -> Result<(), EnvError> {
        self.inner.free_tls(key)
    }
}

#[cfg(test)]
pub(crate) mod testenv {
    //! A `ProcessEnv` over the `ModelPort` scheduler and the address-space fakes, shared by the
    //! lifecycle tests.
    use super::super::scheduler_bridge::testkit::ModelPort;
    use crate::address_space::fakes::Fx;
    use crate::address_space::{FrameAlloc, PhysMem};
    use crate::process::{EnvError, ProcessEnv};
    use crate::thread::{self, Handle};

    pub struct Env<'a, const N: usize> {
        pub port: &'a ModelPort<N>,
        pub fx: Fx,
        pub kills: usize,
        pub fail_kill: bool,
    }

    impl<'a, const N: usize> Env<'a, N> {
        pub fn new(port: &'a ModelPort<N>, pages: usize) -> Self {
            Self {
                port,
                fx: Fx::new(pages),
                kills: 0,
                fail_kill: false,
            }
        }
    }

    impl<const N: usize> ProcessEnv for Env<'_, N> {
        fn phys(&mut self) -> (&mut dyn PhysMem, &mut dyn FrameAlloc) {
            (&mut self.fx.mem, &mut self.fx.frames)
        }
        fn current_thread(&self) -> Option<Handle> {
            self.port.sched(|s| s.current())
        }
        fn active_cr3(&self) -> u64 {
            0
        }
        fn kill_thread(&mut self, h: Handle, status: i32) -> Result<(), EnvError> {
            if self.fail_kill {
                return Err(EnvError::Failed);
            }
            self.kills += 1;
            self.port.sched(|s| s.kill(h, status)).map_err(|e| match e {
                thread::Error::Stale => EnvError::Gone,
                _ => EnvError::Failed,
            })
        }
        fn reap_thread(&mut self, h: Handle) -> Result<(), EnvError> {
            self.port
                .sched(|s| s.reap(h).map(|_| ()))
                .map_err(|e| match e {
                    thread::Error::StillOnCpu => EnvError::StillOnCpu,
                    thread::Error::Stale => EnvError::Gone,
                    _ => EnvError::Failed,
                })
        }
        fn free_tls(&mut self, _key: u64) -> Result<(), EnvError> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::cap_rights::Rights;
    use super::super::ipc::{RecvOutcome, SendOutcome};
    use super::super::ipc_abi::Pending;
    use super::super::ipc_message::UserMessage;
    use super::super::kernel_domain::{IpcState, KernelDomain};
    use super::super::scheduler_bridge::testkit::ModelPort;
    use super::testenv::Env;
    use super::*;
    use crate::thread::State;

    type D = KernelDomain<4, 4, 16, 2>;
    const A: u64 = 1;

    fn active() -> D {
        let d = D::new();
        d.install(IpcState::new()).unwrap();
        d
    }

    /// Thread `i` blocks receiving on `ep` (with its pending record armed).
    fn block_receiver(port: &ModelPort<4>, d: &D, i: usize, owner: u64, ep: u64) -> Handle {
        let h = port.thread(i);
        port.run_until(h);
        let ipc = IpcKernel::new(d, port);
        ipc.locked(|st, s| {
            let tk = st.pending.begin(h).unwrap();
            assert_eq!(st.domain.recv(s, owner, ep, true), Ok(RecvOutcome::Blocked));
            st.pending.arm(tk, owner, Pending::Recv { out: 0x1000 });
        })
        .unwrap();
        h
    }

    #[test]
    fn cleanup_removes_wait_and_pending_but_keeps_process_capabilities() {
        let port = ModelPort::<4>::new(&[4, 4]);
        let d = active();
        let ipc = IpcKernel::new(&d, &port);
        let ep = d.with(|st| st.domain.create_endpoint(A)).unwrap().unwrap();
        let h = block_receiver(&port, &d, 0, A, ep);
        port.run_until(port.thread(1));
        port.sched(|s| s.kill(h, -9)).unwrap();
        let r = cleanup_thread(&ipc, h).unwrap();
        assert_eq!(
            r,
            ThreadCleanup {
                inactive: false,
                pending_cancelled: true,
                wait_removed: true
            }
        );
        d.with(|st| {
            assert!(st.pending.is_empty());
            assert_eq!(st.domain.waiting_threads(), 0);
            assert_eq!(
                st.domain.caps().owner_count(A),
                1,
                "the process still owns its endpoint"
            );
        })
        .unwrap();
        // idempotent
        assert_eq!(cleanup_thread(&ipc, h), Ok(ThreadCleanup::default()));
    }

    #[test]
    fn a_dead_thread_is_never_woken_or_handed_a_message() {
        let port = ModelPort::<4>::new(&[4, 4]);
        let d = active();
        let ipc = IpcKernel::new(&d, &port);
        let ep = d.with(|st| st.domain.create_endpoint(A)).unwrap().unwrap();
        let h = block_receiver(&port, &d, 0, A, ep);
        port.run_until(port.thread(1));
        port.sched(|s| s.kill(h, -9)).unwrap();
        cleanup_thread(&ipc, h).unwrap();
        // a later send is queued for the next receiver, not delivered to the dead thread
        let m = UserMessage::new(5, &[], None).unwrap();
        let out = ipc
            .locked(|st, s| st.domain.send(s, A, ep, &m, false))
            .unwrap();
        assert_eq!(out, Ok(SendOutcome::Sent));
        d.with(|st| {
            assert_eq!(st.domain.queued(A, ep), Ok(1));
            assert_eq!(st.domain.stats().handoffs, 0);
        })
        .unwrap();
        ipc.locked(|st, s| st.domain.check_invariants(s))
            .unwrap()
            .unwrap();
        assert_eq!(
            port.sched(|s| s.thread(h).map(|t| t.state())),
            Ok(State::Terminated)
        );
    }

    #[test]
    fn a_blocked_senders_capability_does_not_outlive_the_thread() {
        let port = ModelPort::<4>::new(&[4, 4]);
        let d = active();
        let ipc = IpcKernel::new(&d, &port);
        let (ep, nf) = d
            .with(|st| {
                (
                    st.domain.create_endpoint(A).unwrap(),
                    st.domain.create_notification(A).unwrap(),
                )
            })
            .unwrap();
        let h = port.thread(0);
        port.run_until(h);
        ipc.locked(|st, s| {
            let m = UserMessage::new(1, &[], None).unwrap();
            for _ in 0..2 {
                st.domain.send(s, A, ep, &m, false).unwrap();
            }
            let carry = UserMessage::new(9, &[], Some(nf)).unwrap();
            let tk = st.pending.begin(h).unwrap();
            assert_eq!(
                st.domain.send(s, A, ep, &carry, true),
                Ok(SendOutcome::Blocked)
            );
            st.pending.arm(tk, A, Pending::Send);
            assert_eq!(st.domain.caps().limbo_count(), 1, "in flight");
        })
        .unwrap();
        port.run_until(port.thread(1));
        port.sched(|s| s.kill(h, -9)).unwrap();
        let r = cleanup_thread(&ipc, h).unwrap();
        assert!(r.pending_cancelled && r.wait_removed);
        d.with(|st| assert_eq!(st.domain.caps().limbo_count(), 0, "no leaked capability"))
            .unwrap();
    }

    #[test]
    fn cleanup_before_init_or_after_shutdown_is_a_harmless_no_op() {
        let port = ModelPort::<4>::new(&[4]);
        let d = D::new();
        let ipc = IpcKernel::new(&d, &port);
        let r = cleanup_thread(&ipc, port.thread(0)).unwrap();
        assert!(r.inactive && !r.pending_cancelled && !r.wait_removed);
    }

    #[test]
    fn interrupt_wakes_an_ipc_waiter_once_and_it_completes_as_eintr() {
        let port = ModelPort::<4>::new(&[4, 4]);
        let d = active();
        let ipc = IpcKernel::new(&d, &port);
        let ep = d.with(|st| st.domain.create_endpoint(A)).unwrap().unwrap();
        let h = block_receiver(&port, &d, 0, A, ep);
        port.run_until(port.thread(1));
        assert_eq!(interrupt_thread(&ipc, h), Ok(WakeOutcome::Woken));
        assert_eq!(
            interrupt_thread(&ipc, h),
            Err(BridgeError::NotIpcBlocked),
            "no second wake-up"
        );
        assert_eq!(
            interrupt_thread(&ipc, port.thread(1)),
            Err(BridgeError::NotIpcBlocked),
            "a running thread is not interruptible"
        );
        port.run_until(h);
        let r = d.with(|st| st.domain.resume(h)).unwrap();
        assert_eq!(r, Err(IpcError::Interrupted));
    }

    #[test]
    fn the_cleanup_env_runs_cleanup_on_kill_even_if_the_slot_is_gone() {
        let port = ModelPort::<4>::new(&[4, 4]);
        let d = active();
        let ipc = IpcKernel::new(&d, &port);
        let ep = d.with(|st| st.domain.create_endpoint(A)).unwrap().unwrap();
        let h = block_receiver(&port, &d, 0, A, ep);
        port.run_until(port.thread(1));
        let mut env = Env::new(&port, 8);
        let mut wrapped = IpcCleanupEnv::new(&mut env, &ipc);
        assert_eq!(wrapped.kill_thread(h, -9), Ok(()));
        d.with(|st| assert!(st.pending.is_empty() && st.domain.waiting_threads() == 0))
            .unwrap();
        // reaped and killed again: Gone is passed on and cleanup still runs (idempotent)
        wrapped.reap_thread(h).unwrap();
        assert_eq!(wrapped.kill_thread(h, -9), Err(EnvError::Gone));
        assert_eq!(wrapped.cleanup_failures, 0);
        let _ = Rights::SEND;
    }

    #[test]
    fn a_failing_inner_kill_does_not_run_cleanup() {
        let port = ModelPort::<4>::new(&[4, 4]);
        let d = active();
        let ipc = IpcKernel::new(&d, &port);
        let ep = d.with(|st| st.domain.create_endpoint(A)).unwrap().unwrap();
        let h = block_receiver(&port, &d, 0, A, ep);
        port.run_until(port.thread(1));
        let mut env = Env::new(&port, 8);
        env.fail_kill = true;
        let mut wrapped = IpcCleanupEnv::new(&mut env, &ipc);
        assert_eq!(wrapped.kill_thread(h, -9), Err(EnvError::Failed));
        d.with(|st| {
            assert!(
                st.pending.is_pending(h),
                "thread is still alive and blocked"
            )
        })
        .unwrap();
    }
}
