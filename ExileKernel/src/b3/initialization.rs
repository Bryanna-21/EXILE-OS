//! Ordered start-up and shut-down of kernel IPC (Mission B4). SPDX-License-Identifier: Apache-2.0
//!
//! # Order
//! 1. the scheduler must exist and own its idle thread (`SchedPort::is_ready`): blocking needs
//!    it;
//! 2. the user-memory layer (page tables, frame allocator, process table) must be ready, as
//!    reported by the caller;
//! 3. only then is a fresh [`IpcState`] installed in the [`KernelDomain`], which is what makes
//!    the `0x40..=0x48` system calls answer. Until that moment (and after shutdown) the
//!    dispatcher answers `ENOSYS` without touching any state, so a system call cannot reach an
//!    uninitialised subsystem.
//!
//! `init` does its checks first and installs last, so a failure leaves the domain exactly as it
//! was (uninitialised); the same call can simply be repeated. It never replaces a live domain
//! (`AlreadyInitialized`). It is called once during start-up, never per system call.
//!
//! # Shutdown
//! [`shutdown`] moves the domain to its terminal `Down` phase under the domain lock (so no
//! system call can start afterwards), removes every pending operation, destroys every object
//! and capability and wakes every blocked thread (they complete with `EINTR`: their pending
//! record is gone). `init` may be run again afterwards and starts from empty state.
#![allow(dead_code)]

use super::ipc::IpcError;
use super::kernel_domain::{DomainError, IpcKernel, IpcState};
use super::scheduler_bridge::SchedPort;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitConfig {
    /// The user-memory layer is ready (reported by the kernel's own start-up sequence).
    pub user_memory_ready: bool,
    /// Per-process capability quota; `None` keeps the domain default.
    pub owner_cap_limit: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitError {
    SchedulerNotReady,
    MemoryNotReady,
    AlreadyInitialized,
    /// The requested configuration was refused by the domain.
    BadConfig(IpcError),
    Domain(DomainError),
}

/// Initialises kernel IPC. See the module documentation for the order.
pub fn init<const N: usize, const O: usize, const C: usize, const Q: usize, P: SchedPort<N>>(
    ipc: &IpcKernel<'_, N, O, C, Q, P>,
    cfg: InitConfig,
) -> Result<(), InitError> {
    if ipc.domain.is_active() {
        return Err(InitError::AlreadyInitialized);
    }
    if !ipc.port.is_ready() {
        return Err(InitError::SchedulerNotReady);
    }
    if !cfg.user_memory_ready {
        return Err(InitError::MemoryNotReady);
    }
    let mut st = IpcState::new();
    if let Some(limit) = cfg.owner_cap_limit {
        st.domain
            .set_owner_cap_limit(limit)
            .map_err(InitError::BadConfig)?;
    }
    ipc.domain.install(st).map_err(|e| match e {
        DomainError::AlreadyActive => InitError::AlreadyInitialized,
        e => InitError::Domain(e),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownReport {
    /// Blocked syscalls that were abandoned.
    pub pending_cancelled: usize,
    /// Capabilities destroyed.
    pub caps_closed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownError {
    /// Not initialised (or already shut down).
    NotActive,
    /// The domain is `Down` now, but tearing the objects down reported a problem.
    Teardown(IpcError),
}

/// Shuts kernel IPC down. Must not be called with the domain or scheduler lock held.
pub fn shutdown<const N: usize, const O: usize, const C: usize, const Q: usize, P: SchedPort<N>>(
    ipc: &IpcKernel<'_, N, O, C, Q, P>,
) -> Result<ShutdownReport, ShutdownError> {
    let r = ipc
        .domain
        .take_down(|st| {
            ipc.port.with_sched(|s| {
                let pending_cancelled = st.pending.clear();
                st.domain.close_all(s).map(|caps_closed| ShutdownReport {
                    pending_cancelled,
                    caps_closed,
                })
            })
        })
        .map_err(|_| ShutdownError::NotActive)?;
    r.map_err(ShutdownError::Teardown)
}

#[cfg(test)]
mod tests {
    use super::super::ipc::RecvOutcome;
    use super::super::ipc_abi::Pending;
    use super::super::kernel_domain::KernelDomain;
    use super::super::scheduler_bridge::testkit::ModelPort;
    use super::*;

    type D = KernelDomain<4, 4, 16, 2>;
    const OK: InitConfig = InitConfig {
        user_memory_ready: true,
        owner_cap_limit: None,
    };

    /// A port whose scheduler has no idle thread yet.
    struct NoIdle(ModelPort<4>);
    impl SchedPort<4> for NoIdle {
        fn with_sched<R>(&self, f: impl FnOnce(&mut crate::sched::Scheduler<4>) -> R) -> R {
            self.0.with_sched(f)
        }
        fn is_ready(&self) -> bool {
            false
        }
        fn switch_away(&self) -> Result<(), super::super::scheduler_bridge::BridgeError> {
            self.0.switch_away()
        }
    }

    #[test]
    fn init_checks_the_scheduler_then_memory_and_installs_last() {
        let d = D::new();
        let np = NoIdle(ModelPort::<4>::new(&[4]));
        let k = IpcKernel::new(&d, &np);
        assert_eq!(init(&k, OK), Err(InitError::SchedulerNotReady));
        assert!(
            !d.is_active(),
            "failed init leaves the domain uninitialised"
        );
        let port = ModelPort::<4>::new(&[4]);
        let k = IpcKernel::new(&d, &port);
        let no_mem = InitConfig {
            user_memory_ready: false,
            ..OK
        };
        assert_eq!(init(&k, no_mem), Err(InitError::MemoryNotReady));
        assert!(!d.is_active());
        assert_eq!(init(&k, OK), Ok(()));
        assert!(d.is_active());
    }

    #[test]
    fn init_never_replaces_a_live_domain() {
        let d = D::new();
        let port = ModelPort::<4>::new(&[4]);
        let k = IpcKernel::new(&d, &port);
        init(&k, OK).unwrap();
        d.with(|st| st.domain.create_endpoint(1).unwrap()).unwrap();
        assert_eq!(init(&k, OK), Err(InitError::AlreadyInitialized));
        assert_eq!(
            d.with(|st| st.domain.live_objects()),
            Ok(1),
            "state untouched"
        );
    }

    #[test]
    fn a_refused_configuration_leaves_the_domain_uninitialised() {
        let d = D::new();
        let port = ModelPort::<4>::new(&[4]);
        let k = IpcKernel::new(&d, &port);
        let bad = InitConfig {
            owner_cap_limit: Some(0),
            ..OK
        };
        match init(&k, bad) {
            Err(InitError::BadConfig(_)) => assert!(!d.is_active()),
            Ok(()) => panic!("a zero quota must not be accepted"),
            Err(e) => panic!("unexpected {e:?}"),
        }
        let ok = InitConfig {
            owner_cap_limit: Some(2),
            ..OK
        };
        assert_eq!(init(&k, ok), Ok(()));
        let r = d.with(|st| {
            st.domain.create_endpoint(1).unwrap();
            st.domain.create_endpoint(1).unwrap();
            st.domain.create_endpoint(1)
        });
        assert!(r.unwrap().is_err(), "the configured quota is enforced");
    }

    #[test]
    fn shutdown_wakes_blocked_threads_and_allows_a_clean_reinit() {
        let d = D::new();
        let port = ModelPort::<4>::new(&[4, 4]);
        let k = IpcKernel::new(&d, &port);
        assert_eq!(shutdown(&k), Err(ShutdownError::NotActive));
        init(&k, OK).unwrap();
        let ep = d.with(|st| st.domain.create_endpoint(1).unwrap()).unwrap();
        let h = port.thread(0);
        port.run_until(h);
        k.locked(|st, s| {
            let tk = st.pending.begin(h).unwrap();
            assert_eq!(st.domain.recv(s, 1, ep, true), Ok(RecvOutcome::Blocked));
            st.pending.arm(tk, 1, Pending::Recv { out: 0x1000 });
        })
        .unwrap();
        port.run_until(port.thread(1));
        let rep = shutdown(&k).unwrap();
        assert_eq!(
            rep,
            ShutdownReport {
                pending_cancelled: 1,
                caps_closed: 1
            }
        );
        assert!(!d.is_active());
        assert_eq!(d.with(|_| ()), Err(DomainError::NotReady));
        // the blocked thread was woken, not lost
        assert!(
            port.sched(|s| s
                .thread(h)
                .map(|t| t.state() == crate::thread::State::Ready))
                .unwrap()
        );
        assert_eq!(shutdown(&k), Err(ShutdownError::NotActive), "only once");
        init(&k, OK).unwrap();
        assert_eq!(
            d.with(|st| (st.domain.live_objects(), st.pending.len())),
            Ok((0, 0))
        );
    }
}
