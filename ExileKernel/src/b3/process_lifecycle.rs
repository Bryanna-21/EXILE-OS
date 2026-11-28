//! IPC cleanup when a process ends (Mission B4). SPDX-License-Identifier: Apache-2.0
//!
//! The process table (`d5::process`) knows nothing about IPC, and B3 knows nothing about
//! processes, so the link is made here: thin wrappers around the `ProcessTable` operations that
//! can end a process or one of its threads -- `exit`, `fault_exit`, `deliver` (the unmaskable
//! kill event), `thread_exited` and `collect` -- which run the process operation exactly as D5
//! defines it and then reconcile IPC with the outcome:
//!
//! * every thread the operation kills goes through
//!   [`IpcCleanupEnv`](super::thread_lifecycle::IpcCleanupEnv) (pending record and wait record
//!   removed, no re-wake);
//! * as soon as the process is `Exiting` or `Zombie` (or its handle is already stale) the IPC
//!   *owner* is torn down: all capabilities of the process are closed in every table
//!   (`IpcDomain::owner_exit`, which wakes threads of other processes that waited on objects
//!   that died with it), and any pending operation recorded for it is removed.
//!
//! The owner id is the process id, which D5 never reuses, so a handle issued to the dead
//! process cannot be valid for a later process even if the capability slots are reused: it is
//! rejected both by the generation check and by the owner check.
//!
//! The reconcile step runs even when the wrapped D5 operation returned an error (a half-done
//! exit leaves the process `Exiting`, which already forbids new IPC). The D5 error is the one
//! reported; a cleanup error is reported only if D5 succeeded. Cleanup is idempotent, so the
//! retry the D5 layer expects simply repeats it.
//!
//! None of these functions may be called with the domain or scheduler lock held.
#![allow(dead_code)]

use super::ipc::IpcError;
use super::kernel_domain::{DomainError, IpcKernel};
use super::scheduler_bridge::SchedPort;
use super::thread_lifecycle::{IpcCleanupEnv, cleanup_thread};
use crate::process::{self, ExitStatus, ProcHandle, ProcState, ProcessEnv, ProcessTable};
use crate::thread::Handle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleError {
    /// The process layer refused or failed the operation.
    Process(process::Error),
    /// The process layer succeeded but IPC cleanup failed.
    Ipc(IpcError),
}

/// What [`cleanup_process`] removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProcessCleanup {
    /// The IPC domain is not initialised, so nothing existed.
    pub inactive: bool,
    /// Capabilities closed (all tables, including ones in flight to the process).
    pub caps_closed: usize,
    /// Pending blocked-syscall records removed.
    pub pending_cancelled: usize,
}

/// Tears down everything IPC holds for process `owner`. Idempotent.
pub fn cleanup_process<
    const N: usize,
    const O: usize,
    const C: usize,
    const Q: usize,
    P: SchedPort<N>,
>(
    ipc: &IpcKernel<'_, N, O, C, Q, P>,
    owner: u64,
) -> Result<ProcessCleanup, IpcError> {
    match ipc.locked(|st, s| {
        let pending_cancelled = st.pending.cancel_owner(owner);
        let caps_closed = st.domain.owner_exit(s, owner)?;
        Ok(ProcessCleanup {
            inactive: false,
            caps_closed,
            pending_cancelled,
        })
    }) {
        Ok(r) => r,
        Err(DomainError::NotReady) => Ok(ProcessCleanup {
            inactive: true,
            ..ProcessCleanup::default()
        }),
        Err(_) => Err(IpcError::Corrupt("ipc domain unavailable")),
    }
}

fn reconcile<
    T,
    const PN: usize,
    const N: usize,
    const O: usize,
    const C: usize,
    const Q: usize,
    P: SchedPort<N>,
>(
    table: &ProcessTable<PN>,
    ipc: &IpcKernel<'_, N, O, C, Q, P>,
    h: ProcHandle,
    pid: u64,
    r: Result<T, process::Error>,
) -> Result<T, LifecycleError> {
    let ending = matches!(
        table.state(h),
        Ok(ProcState::Exiting) | Ok(ProcState::Zombie) | Err(process::Error::Stale)
    );
    let cleanup = if ending {
        Some(cleanup_process(ipc, pid))
    } else {
        None
    };
    match (r, cleanup) {
        (Err(e), _) => Err(LifecycleError::Process(e)),
        (Ok(_), Some(Err(e))) => Err(LifecycleError::Ipc(e)),
        (Ok(v), _) => Ok(v),
    }
}

fn pid_of<const PN: usize>(table: &ProcessTable<PN>, h: ProcHandle) -> Result<u64, LifecycleError> {
    table.pid(h).map(|p| p.0).map_err(LifecycleError::Process)
}

/// `ProcessTable::exit` plus IPC cleanup.
pub fn exit<
    E: ProcessEnv,
    const PN: usize,
    const N: usize,
    const O: usize,
    const C: usize,
    const Q: usize,
    P: SchedPort<N>,
>(
    table: &mut ProcessTable<PN>,
    env: &mut E,
    ipc: &IpcKernel<'_, N, O, C, Q, P>,
    h: ProcHandle,
    status: ExitStatus,
) -> Result<(), LifecycleError> {
    let pid = pid_of(table, h)?;
    let r = table.exit(&mut IpcCleanupEnv::new(env, ipc), h, status);
    reconcile(table, ipc, h, pid, r)
}

/// `ProcessTable::fault_exit` plus IPC cleanup.
pub fn fault_exit<
    E: ProcessEnv,
    const PN: usize,
    const N: usize,
    const O: usize,
    const C: usize,
    const Q: usize,
    P: SchedPort<N>,
>(
    table: &mut ProcessTable<PN>,
    env: &mut E,
    ipc: &IpcKernel<'_, N, O, C, Q, P>,
    h: ProcHandle,
    reason: u32,
) -> Result<(), LifecycleError> {
    let pid = pid_of(table, h)?;
    let r = table.fault_exit(&mut IpcCleanupEnv::new(env, ipc), h, reason);
    reconcile(table, ipc, h, pid, r)
}

/// `ProcessTable::deliver` (act on a pending kill event) plus IPC cleanup. `Ok(true)` if the
/// process was terminated.
pub fn deliver<
    E: ProcessEnv,
    const PN: usize,
    const N: usize,
    const O: usize,
    const C: usize,
    const Q: usize,
    P: SchedPort<N>,
>(
    table: &mut ProcessTable<PN>,
    env: &mut E,
    ipc: &IpcKernel<'_, N, O, C, Q, P>,
    h: ProcHandle,
) -> Result<bool, LifecycleError> {
    let pid = pid_of(table, h)?;
    let r = table.deliver(&mut IpcCleanupEnv::new(env, ipc), h);
    reconcile(table, ipc, h, pid, r)
}

/// A thread of process `h` has terminated itself: its IPC state is removed first (it must not
/// be woken or served any more), then D5 records the exit, which ends the process when this
/// was its last live thread.
pub fn thread_exited<
    E: ProcessEnv,
    const PN: usize,
    const N: usize,
    const O: usize,
    const C: usize,
    const Q: usize,
    P: SchedPort<N>,
>(
    table: &mut ProcessTable<PN>,
    env: &mut E,
    ipc: &IpcKernel<'_, N, O, C, Q, P>,
    h: ProcHandle,
    thread: Handle,
    status: i32,
) -> Result<(), LifecycleError> {
    let pid = pid_of(table, h)?;
    let t = cleanup_thread(ipc, thread).map_err(LifecycleError::Ipc);
    let r = table.thread_exited(&mut IpcCleanupEnv::new(env, ipc), h, thread, status);
    let r = reconcile(table, ipc, h, pid, r);
    match (r, t) {
        (Err(e), _) => Err(e),
        (Ok(()), Err(e)) => Err(e),
        (Ok(()), Ok(_)) => Ok(()),
    }
}

/// `ProcessTable::collect` plus IPC reconcile (a zombie process holds no IPC state).
pub fn collect<
    E: ProcessEnv,
    const PN: usize,
    const N: usize,
    const O: usize,
    const C: usize,
    const Q: usize,
    P: SchedPort<N>,
>(
    table: &mut ProcessTable<PN>,
    env: &mut E,
    ipc: &IpcKernel<'_, N, O, C, Q, P>,
    h: ProcHandle,
) -> Result<(), LifecycleError> {
    let pid = pid_of(table, h)?;
    let r = table.collect(&mut IpcCleanupEnv::new(env, ipc), h);
    reconcile(table, ipc, h, pid, r)
}

#[cfg(test)]
mod tests {
    use super::super::cap_rights::Rights;
    use super::super::ipc::{RecvOutcome, SendOutcome};
    use super::super::ipc_abi::Pending;
    use super::super::ipc_message::UserMessage;
    use super::super::kernel_domain::{IpcState, KernelDomain};
    use super::super::scheduler_bridge::testkit::ModelPort;
    use super::super::thread_lifecycle::testenv::Env;
    use super::*;
    use crate::process::{EVENT_KILL, Limits};
    use crate::tls::TlsContext;

    type D = KernelDomain<4, 8, 32, 2>;
    type T = ProcessTable<4>;

    struct World<'a> {
        port: &'a ModelPort<4>,
        d: &'a D,
        t: T,
        env: Env<'a, 4>,
    }

    impl<'a> World<'a> {
        fn new(port: &'a ModelPort<4>, d: &'a D) -> Self {
            d.install(IpcState::new()).unwrap();
            let mut env = Env::new(port, 32);
            let mut t = T::new();
            // process 0 is the root (D5 allows exactly one parentless process); it has no threads
            let sp = env.fx.space();
            t.create(None, Limits::DEFAULT, sp)
                .map_err(|(e, _)| e)
                .unwrap();
            World { port, d, t, env }
        }
        /// A process with the given scheduler threads; returns (handle, pid).
        fn proc(&mut self, threads: &[usize]) -> (ProcHandle, u64) {
            let sp = self.env.fx.space();
            let h = self
                .t
                .create(self.t.root(), Limits::DEFAULT, sp)
                .map_err(|(e, _)| e)
                .unwrap();
            for &i in threads {
                self.t
                    .add_thread(h, self.port.thread(i), None, TlsContext::default())
                    .unwrap();
            }
            (h, self.t.pid(h).unwrap().0)
        }
        fn ipc(&self) -> IpcKernel<'a, 4, 8, 32, 2, ModelPort<4>> {
            IpcKernel::new(self.d, self.port)
        }
        fn block_recv(&self, i: usize, owner: u64, ep: u64) {
            let h = self.port.thread(i);
            self.port.run_until(h);
            self.ipc()
                .locked(|st, s| {
                    let tk = st.pending.begin(h).unwrap();
                    assert_eq!(st.domain.recv(s, owner, ep, true), Ok(RecvOutcome::Blocked));
                    st.pending.arm(tk, owner, Pending::Recv { out: 0x2000 });
                })
                .unwrap();
        }
    }

    #[test]
    fn exit_closes_every_capability_kills_blocked_threads_and_invalidates_handles() {
        let port = ModelPort::<4>::new(&[4, 4, 4]);
        let d = D::new();
        let mut w = World::new(&port, &d);
        let (p1, pid1) = w.proc(&[0]);
        let (_p2, pid2) = w.proc(&[2]);
        let (ep, nf) = d
            .with(|st| {
                (
                    st.domain.create_endpoint(pid1).unwrap(),
                    st.domain.create_notification(pid1).unwrap(),
                )
            })
            .unwrap();
        let other = d
            .with(|st| st.domain.create_endpoint(pid2).unwrap())
            .unwrap();
        w.block_recv(0, pid1, ep);
        port.run_until(port.thread(1));
        let ipc = w.ipc();
        exit(&mut w.t, &mut w.env, &ipc, p1, ExitStatus::Code(3)).unwrap();
        d.with(|st| {
            assert!(st.pending.is_empty(), "pending operation removed");
            assert_eq!(st.domain.waiting_threads(), 0);
            assert_eq!(
                st.domain.caps().owner_count(pid1),
                0,
                "all capabilities closed"
            );
            assert_eq!(
                st.domain.live_objects(),
                1,
                "only the other process's endpoint"
            );
            // stale handles are rejected, they do not resurrect
            let m = UserMessage::new(1, &[], None).unwrap();
            let sc = ModelPort::<4>::new(&[4]); // a scheduler is needed for the call only
            sc.run_next();
            sc.sched(|s| {
                assert!(st.domain.send(s, pid1, ep, &m, false).is_err());
                assert!(st.domain.signal(s, pid1, nf, 1).is_err());
            });
            assert_eq!(st.domain.caps().owner_count(pid2), 1);
        })
        .unwrap();
        let _ = other;
        assert!(matches!(
            w.t.state(p1),
            Ok(ProcState::Exiting) | Ok(ProcState::Zombie)
        ));
        // idempotent: a second exit and an explicit cleanup find nothing
        exit(&mut w.t, &mut w.env, &ipc, p1, ExitStatus::Code(3)).unwrap();
        assert_eq!(
            cleanup_process(&ipc, pid1),
            Ok(ProcessCleanup::default()),
            "nothing left"
        );
        ipc.locked(|st, s| st.domain.check_invariants(s))
            .unwrap()
            .unwrap();
    }

    #[test]
    fn shared_objects_survive_one_owner_and_die_with_the_last() {
        let port = ModelPort::<4>::new(&[4, 4, 4]);
        let d = D::new();
        let mut w = World::new(&port, &d);
        let (p1, pid1) = w.proc(&[0]);
        let (p2, pid2) = w.proc(&[1]);
        let ep1 = d
            .with(|st| st.domain.create_endpoint(pid1).unwrap())
            .unwrap();
        let ep2 = d
            .with(|st| st.domain.grant(pid1, ep1, pid2, Rights::RECV).unwrap())
            .unwrap();
        w.block_recv(1, pid2, ep2);
        port.run_until(port.thread(2));
        let ipc = w.ipc();
        exit(&mut w.t, &mut w.env, &ipc, p1, ExitStatus::Code(0)).unwrap();
        d.with(|st| {
            assert_eq!(st.domain.live_objects(), 1, "p2 still holds the endpoint");
            assert!(
                st.pending.is_pending(port.thread(1)),
                "p2's receiver still blocked"
            );
        })
        .unwrap();
        // the last owner goes: p2's blocked receiver is killed and its pending record removed
        port.run_until(port.thread(2));
        exit(&mut w.t, &mut w.env, &ipc, p2, ExitStatus::Code(0)).unwrap();
        d.with(|st| {
            assert_eq!(st.domain.live_objects(), 0);
            assert!(st.pending.is_empty());
        })
        .unwrap();
    }

    #[test]
    fn fault_exit_and_kill_event_clean_up_too() {
        let port = ModelPort::<4>::new(&[4, 4, 4]);
        let d = D::new();
        let mut w = World::new(&port, &d);
        let (p1, pid1) = w.proc(&[0]);
        let (p2, pid2) = w.proc(&[1]);
        for pid in [pid1, pid2] {
            d.with(|st| st.domain.create_endpoint(pid).unwrap())
                .unwrap();
        }
        port.run_until(port.thread(2));
        let ipc = w.ipc();
        fault_exit(&mut w.t, &mut w.env, &ipc, p1, 14).unwrap();
        assert_eq!(d.with(|st| st.domain.caps().owner_count(pid1)), Ok(0));
        // a kill event that has not been delivered yet changes nothing
        w.t.post_event(p2, EVENT_KILL).unwrap();
        assert_eq!(d.with(|st| st.domain.caps().owner_count(pid2)), Ok(1));
        assert_eq!(deliver(&mut w.t, &mut w.env, &ipc, p2), Ok(true));
        assert_eq!(d.with(|st| st.domain.caps().owner_count(pid2)), Ok(0));
        assert_eq!(deliver(&mut w.t, &mut w.env, &ipc, p2), Ok(false));
    }

    #[test]
    fn a_thread_exit_keeps_the_process_capabilities_until_the_last_thread_goes() {
        let port = ModelPort::<4>::new(&[4, 4, 4]);
        let d = D::new();
        let mut w = World::new(&port, &d);
        let (p1, pid1) = w.proc(&[0, 1]);
        let ep = d
            .with(|st| st.domain.create_endpoint(pid1).unwrap())
            .unwrap();
        w.block_recv(0, pid1, ep);
        port.run_until(port.thread(1));
        let ipc = w.ipc();
        // thread 0 is killed from outside (it is blocked), then reported as exited
        port.sched(|s| s.kill(port.thread(0), -1)).unwrap();
        thread_exited(&mut w.t, &mut w.env, &ipc, p1, port.thread(0), -1).unwrap();
        d.with(|st| {
            assert!(st.pending.is_empty());
            assert_eq!(st.domain.waiting_threads(), 0);
            assert_eq!(
                st.domain.caps().owner_count(pid1),
                1,
                "process still owns the endpoint"
            );
        })
        .unwrap();
        assert_eq!(w.t.state(p1), Ok(ProcState::Running));
        // the last live thread terminates itself: the process ends and IPC is torn down
        port.sched(|s| s.terminate_current(0)).unwrap();
        thread_exited(&mut w.t, &mut w.env, &ipc, p1, port.thread(1), 0).unwrap();
        assert_eq!(d.with(|st| st.domain.caps().owner_count(pid1)), Ok(0));
    }

    #[test]
    fn a_failed_process_operation_is_reported_and_cleanup_is_retried_by_the_retry() {
        let port = ModelPort::<4>::new(&[4, 4, 4]);
        let d = D::new();
        let mut w = World::new(&port, &d);
        let (p1, pid1) = w.proc(&[0]);
        let ep = d
            .with(|st| st.domain.create_endpoint(pid1).unwrap())
            .unwrap();
        w.block_recv(0, pid1, ep);
        port.run_until(port.thread(1));
        let ipc = w.ipc();
        w.env.fail_kill = true;
        let e = exit(&mut w.t, &mut w.env, &ipc, p1, ExitStatus::Code(0)).unwrap_err();
        assert!(matches!(e, LifecycleError::Process(_)));
        // the process is already Exiting: no more IPC for it, and its capabilities are gone
        assert_eq!(w.t.state(p1), Ok(ProcState::Exiting));
        assert_eq!(d.with(|st| st.domain.caps().owner_count(pid1)), Ok(0));
        assert!(d.with(|st| st.pending.is_empty()).unwrap());
        // retry succeeds once the kill works
        w.env.fail_kill = false;
        exit(&mut w.t, &mut w.env, &ipc, p1, ExitStatus::Code(0)).unwrap();
        assert_eq!(d.with(|st| st.domain.waiting_threads()), Ok(0));
    }

    #[test]
    fn unknown_process_handles_are_refused_before_anything_happens() {
        let port = ModelPort::<4>::new(&[4]);
        let d = D::new();
        let mut w = World::new(&port, &d);
        let (p1, _) = w.proc(&[0]);
        let ipc = w.ipc();
        let mut other = T::new();
        assert!(matches!(
            exit(&mut other, &mut w.env, &ipc, p1, ExitStatus::Code(0)),
            Err(LifecycleError::Process(_))
        ));
        assert_eq!(w.t.state(p1), Ok(ProcState::Running));
        let _ = SendOutcome::Sent;
    }
}
