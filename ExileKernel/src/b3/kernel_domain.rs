//! The kernel-owned IPC domain (Mission B4). SPDX-License-Identifier: Apache-2.0
//!
//! [`KernelDomain`] is the single authoritative place where capability and IPC state lives in a
//! running kernel. It is meant to be a `static`: constructing it is `const`, the state inside
//! only appears when [`initialization::init`](super::initialization::init) succeeds, and every
//! access goes through a [`SpinLock`], so there is no unprotected global mutable IPC domain.
//!
//! # Phases
//! `Uninit` (static initial state) -> `Active` (after `init`) -> `Down` (after `shutdown`).
//! Every operation on a domain that is not `Active` fails with [`DomainError::NotReady`] without
//! touching anything, which is how syscalls are kept away from an uninitialised subsystem.
//! `init` may be called again after `shutdown`; it never replaces an `Active` domain.
//!
//! # Locking
//! There are two locks: this domain's and the scheduler's (`dispatch::KernelSlot`). The only
//! allowed order is **domain first, scheduler second**, and [`IpcKernel::locked`] is the only
//! way to take both, so the order cannot be inverted by accident. Neither lock is held while a
//! thread is switched away (`scheduler_bridge::park` runs with both released). Closures passed
//! to `with`/`locked` must do bookkeeping only: they must not block, switch contexts or call
//! back into the same `KernelDomain` (the locks are not recursive).
//!
//! The spin locks do not mask interrupts. EUK runs system calls with interrupts disabled; an
//! interrupt handler must therefore use [`KernelDomain::try_with`], which reports
//! [`DomainError::Busy`] instead of spinning against the interrupted holder.
#![allow(dead_code)]

use super::ipc::IpcDomain;
use super::resume::PendingTable;
use super::scheduler_bridge::SchedPort;
use crate::sched::Scheduler;
use crate::spin::SpinLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainError {
    /// The domain is not initialised (or has been shut down).
    NotReady,
    /// `init` found an active domain.
    AlreadyActive,
    /// The lock is held (only `try_with` reports this).
    Busy,
}

/// Everything protected by the domain lock: the IPC domain and the table of syscalls that are
/// blocked inside it, so both are always updated together.
pub struct IpcState<const N: usize, const O: usize, const C: usize, const Q: usize> {
    pub domain: IpcDomain<N, O, C, Q>,
    pub pending: PendingTable<N>,
}

impl<const N: usize, const O: usize, const C: usize, const Q: usize> IpcState<N, O, C, Q> {
    pub fn new() -> Self {
        Self {
            domain: IpcDomain::new(),
            pending: PendingTable::new(),
        }
    }
}

impl<const N: usize, const O: usize, const C: usize, const Q: usize> Default
    for IpcState<N, O, C, Q>
{
    fn default() -> Self {
        Self::new()
    }
}

enum Phase<S> {
    Uninit,
    Active(S),
    Down,
}

pub struct KernelDomain<const N: usize, const O: usize, const C: usize, const Q: usize> {
    slot: SpinLock<Phase<IpcState<N, O, C, Q>>>,
}

impl<const N: usize, const O: usize, const C: usize, const Q: usize> Default
    for KernelDomain<N, O, C, Q>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize, const O: usize, const C: usize, const Q: usize> KernelDomain<N, O, C, Q> {
    pub const fn new() -> Self {
        Self {
            slot: SpinLock::new(Phase::Uninit),
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(*self.slot.lock(), Phase::Active(_))
    }

    /// Installs a fresh state. Fails (leaving the existing one untouched) if already active.
    pub(super) fn install(&self, st: IpcState<N, O, C, Q>) -> Result<(), DomainError> {
        let mut g = self.slot.lock();
        if matches!(*g, Phase::Active(_)) {
            return Err(DomainError::AlreadyActive);
        }
        *g = Phase::Active(st);
        Ok(())
    }

    /// Runs `f` on the state under the domain lock.
    pub fn with<R>(
        &self,
        f: impl FnOnce(&mut IpcState<N, O, C, Q>) -> R,
    ) -> Result<R, DomainError> {
        match &mut *self.slot.lock() {
            Phase::Active(st) => Ok(f(st)),
            _ => Err(DomainError::NotReady),
        }
    }

    /// Like [`with`](Self::with) for interrupt context: never spins.
    pub fn try_with<R>(
        &self,
        f: impl FnOnce(&mut IpcState<N, O, C, Q>) -> R,
    ) -> Result<R, DomainError> {
        let mut g = self.slot.try_lock().ok_or(DomainError::Busy)?;
        match &mut *g {
            Phase::Active(st) => Ok(f(st)),
            _ => Err(DomainError::NotReady),
        }
    }

    /// Moves an active domain to `Down`, handing its state to `f` first. The phase changes even
    /// if `f` reports a problem, so a half-shut-down domain can never serve requests.
    pub(super) fn take_down<R>(
        &self,
        f: impl FnOnce(&mut IpcState<N, O, C, Q>) -> R,
    ) -> Result<R, DomainError> {
        let mut g = self.slot.lock();
        let Phase::Active(st) = &mut *g else {
            return Err(DomainError::NotReady);
        };
        let r = f(st);
        *g = Phase::Down;
        Ok(r)
    }
}

/// A domain together with the scheduler it serves: the handle the syscall, lifecycle and
/// initialisation code work with.
pub struct IpcKernel<'a, const N: usize, const O: usize, const C: usize, const Q: usize, P> {
    pub domain: &'a KernelDomain<N, O, C, Q>,
    pub port: &'a P,
}

impl<'a, const N: usize, const O: usize, const C: usize, const Q: usize, P: SchedPort<N>>
    IpcKernel<'a, N, O, C, Q, P>
{
    pub fn new(domain: &'a KernelDomain<N, O, C, Q>, port: &'a P) -> Self {
        Self { domain, port }
    }

    /// Runs `f` with the domain lock **and then** the scheduler lock held.
    pub fn locked<R>(
        &self,
        f: impl FnOnce(&mut IpcState<N, O, C, Q>, &mut Scheduler<N>) -> R,
    ) -> Result<R, DomainError> {
        self.domain.with(|st| self.port.with_sched(|s| f(st, s)))
    }

    /// `locked` for interrupt context: the domain lock is only tried.
    pub fn try_locked<R>(
        &self,
        f: impl FnOnce(&mut IpcState<N, O, C, Q>, &mut Scheduler<N>) -> R,
    ) -> Result<R, DomainError> {
        self.domain
            .try_with(|st| self.port.with_sched(|s| f(st, s)))
    }
}

#[cfg(test)]
mod tests {
    use super::super::scheduler_bridge::testkit::ModelPort;
    use super::*;

    type D = KernelDomain<4, 4, 8, 2>;

    #[test]
    fn uninitialised_domain_refuses_everything_without_side_effects() {
        let d = D::new();
        assert!(!d.is_active());
        assert_eq!(d.with(|_| ()), Err(DomainError::NotReady));
        assert_eq!(d.try_with(|_| ()), Err(DomainError::NotReady));
        assert_eq!(d.take_down(|_| ()), Err(DomainError::NotReady));
        assert!(!d.is_active());
    }

    #[test]
    fn install_activates_once_and_never_replaces_live_state() {
        let d = D::new();
        d.install(IpcState::new()).unwrap();
        assert!(d.is_active());
        let h = d.with(|st| st.domain.create_endpoint(1)).unwrap().unwrap();
        assert_eq!(d.install(IpcState::new()), Err(DomainError::AlreadyActive));
        // the original state (and its endpoint) survived the refused install
        assert_eq!(d.with(|st| st.domain.caps().owner_count(1)), Ok(1));
        let _ = h;
    }

    #[test]
    fn take_down_hands_over_the_state_then_closes_the_domain() {
        let d = D::new();
        d.install(IpcState::new()).unwrap();
        d.with(|st| st.domain.create_notification(2).unwrap())
            .unwrap();
        let live = d.take_down(|st| st.domain.live_objects()).unwrap();
        assert_eq!(live, 1);
        assert!(!d.is_active());
        assert_eq!(d.with(|_| ()), Err(DomainError::NotReady));
        assert_eq!(d.take_down(|_| ()), Err(DomainError::NotReady), "only once");
        // a fresh init after shutdown starts from empty state
        d.install(IpcState::new()).unwrap();
        assert_eq!(d.with(|st| st.domain.live_objects()), Ok(0));
    }

    #[test]
    fn try_with_reports_busy_instead_of_spinning() {
        let d = D::new();
        d.install(IpcState::new()).unwrap();
        let inner = d.with(|_| d.try_with(|_| ())).unwrap();
        assert_eq!(inner, Err(DomainError::Busy));
        assert_eq!(d.try_with(|_| 7), Ok(7), "free again after the outer call");
    }

    #[test]
    fn locked_takes_domain_then_scheduler_and_reports_not_ready() {
        let d = D::new();
        let port = ModelPort::<4>::new(&[4]);
        let k = IpcKernel::new(&d, &port);
        assert_eq!(k.locked(|_, _| ()), Err(DomainError::NotReady));
        d.install(IpcState::new()).unwrap();
        let cur = k.locked(|_, s| s.current()).unwrap();
        assert_eq!(cur, None, "nothing scheduled yet");
        port.run_next();
        let cur = k.locked(|st, s| (st.pending.len(), s.current())).unwrap();
        assert_eq!(cur.0, 0);
        assert!(cur.1.is_some());
        // the scheduler lock is only held inside `locked`
        assert!(!port.sched_locked());
    }
}
