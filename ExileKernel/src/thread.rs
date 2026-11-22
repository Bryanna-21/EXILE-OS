//! Architecture-independent kernel thread control blocks and lifecycle.
//!
//! This module owns thread identity, the lifecycle state machine and slot
//! storage. It deliberately contains NO saved CPU context, kernel stack or
//! FPU/TLS state: those depend on the context-switch contract of Mission C,
//! which does not exist in this repository yet (see docs/SCHEDULER.md and docs/THREADS.md).
//!
//! Safety properties enforced here:
//! - Handles carry a generation, so a stale handle can never reach a reused slot.
//! - State changes go through `State::can_become`; invalid transitions fail.
//! - A terminated thread is only reapable once `on_cpu` is false, i.e. after
//!   the context switch away from it has completed, so its stack could never
//!   be released while still in use.
use crate::runqueue::QueueError;

pub const PRIORITY_LEVELS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    NoSlots,
    IdExhausted,
    Stale,
    InvalidTransition { from: State, to: State },
    BadPriority,
    IsIdle,
    NoCurrent,
    NotCurrent,
    NotTerminated,
    StillOnCpu,
    IsRunning,
    DeadlineOverflow,
    NoRunnable,
    IdleExists,
    Corrupt,
    Queue(QueueError),
}

impl From<QueueError> for Error {
    fn from(e: QueueError) -> Self {
        Error::Queue(e)
    }
}

/// Scheduling priority. Level 0 is reserved for the idle thread; ordinary
/// threads use 1..=7. Higher value means more urgent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Priority(u8);

impl Priority {
    pub const IDLE: Priority = Priority(0);
    pub const MIN: Priority = Priority(1);
    pub const DEFAULT: Priority = Priority(4);
    pub const MAX: Priority = Priority(PRIORITY_LEVELS as u8 - 1);

    pub const fn new(level: u8) -> Result<Priority, Error> {
        if level >= 1 && (level as usize) < PRIORITY_LEVELS {
            Ok(Priority(level))
        } else {
            Err(Error::BadPriority)
        }
    }

    pub const fn level(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThreadId(pub u64);

/// Generation-checked reference to a thread slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handle {
    index: u32,
    generation: u32,
}

impl Handle {
    pub const fn index(self) -> usize {
        self.index as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    New,
    Ready,
    Running,
    Blocked,
    Sleeping,
    Terminated,
}

impl State {
    /// The complete set of legal lifecycle transitions.
    pub const fn can_become(self, to: State) -> bool {
        use State::*;
        matches!(
            (self, to),
            (New, Ready)
                | (New, Terminated)
                | (Ready, Running)
                | (Ready, Terminated)
                | (Running, Ready)
                | (Running, Blocked)
                | (Running, Sleeping)
                | (Running, Terminated)
                | (Blocked, Ready)
                | (Blocked, Terminated)
                | (Sleeping, Ready)
                | (Sleeping, Terminated)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    None,
    /// Waiting on an opaque wait-object id (wait queues, mutexes: Phase 4).
    Event(u32),
    Join(Handle),
    Sleep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeReason {
    None,
    Event,
    Timeout,
    /// A sleeping thread was woken before its deadline.
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct Thread {
    pub(crate) id: ThreadId,
    state: State,
    pub(crate) priority: Priority,
    /// Anti-starvation boost added to `priority` while waiting in Ready.
    pub(crate) boost: u8,
    pub(crate) block_reason: BlockReason,
    pub(crate) wake_reason: WakeReason,
    /// A wake-up arrived while Running; the next `block` must not sleep.
    pub(crate) wake_pending: bool,
    pub(crate) deadline: u64,
    pub(crate) entry: usize,
    pub(crate) arg: usize,
    pub(crate) exit_status: Option<i32>,
    /// True from dispatch until the switch away from this thread completes.
    pub(crate) on_cpu: bool,
    pub(crate) ready_since: u64,
    pub(crate) slice_left: u32,
    pub(crate) cpu_ticks: u64,
    pub(crate) dispatches: u64,
}

impl Thread {
    fn new(id: ThreadId, priority: Priority, entry: usize, arg: usize) -> Self {
        Self {
            id,
            state: State::New,
            priority,
            boost: 0,
            block_reason: BlockReason::None,
            wake_reason: WakeReason::None,
            wake_pending: false,
            deadline: 0,
            entry,
            arg,
            exit_status: None,
            on_cpu: false,
            ready_since: 0,
            slice_left: 0,
            cpu_ticks: 0,
            dispatches: 0,
        }
    }

    pub fn id(&self) -> ThreadId {
        self.id
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn priority(&self) -> Priority {
        self.priority
    }
    pub fn is_idle(&self) -> bool {
        self.priority == Priority::IDLE
    }
    pub fn on_cpu(&self) -> bool {
        self.on_cpu
    }
    pub fn exit_status(&self) -> Option<i32> {
        self.exit_status
    }
    pub fn block_reason(&self) -> BlockReason {
        self.block_reason
    }
    pub fn wake_reason(&self) -> WakeReason {
        self.wake_reason
    }
    pub fn cpu_ticks(&self) -> u64 {
        self.cpu_ticks
    }
    pub fn dispatches(&self) -> u64 {
        self.dispatches
    }
    /// Entry point and argument requested at creation; consumed by the future
    /// architecture trampoline, never executed by this module.
    pub fn entry(&self) -> (usize, usize) {
        (self.entry, self.arg)
    }

    /// Queue level currently used for this thread (base priority plus boost).
    pub fn effective_level(&self) -> usize {
        if self.is_idle() {
            0
        } else {
            (self.priority.level() + self.boost as usize).min(PRIORITY_LEVELS - 1)
        }
    }

    pub(crate) fn set_state(&mut self, to: State) -> Result<(), Error> {
        if !self.state.can_become(to) {
            return Err(Error::InvalidTransition {
                from: self.state,
                to,
            });
        }
        self.state = to;
        Ok(())
    }
}

struct Slot {
    generation: u32,
    /// Generation counter exhausted: slot is never reused (prevents ABA).
    retired: bool,
    thread: Option<Thread>,
}

/// Fixed-capacity thread storage; no heap allocation.
pub struct ThreadTable<const N: usize> {
    slots: [Slot; N],
    next_id: u64,
    live: usize,
}

impl<const N: usize> Default for ThreadTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> ThreadTable<N> {
    pub fn new() -> Self {
        assert!(N < u32::MAX as usize);
        Self {
            slots: core::array::from_fn(|_| Slot {
                generation: 1,
                retired: false,
                thread: None,
            }),
            next_id: 1,
            live: 0,
        }
    }

    pub fn live(&self) -> usize {
        self.live
    }

    pub fn create(
        &mut self,
        priority: Priority,
        entry: usize,
        arg: usize,
    ) -> Result<Handle, Error> {
        let id = self.next_id;
        let next = id.checked_add(1).ok_or(Error::IdExhausted)?;
        let index = self
            .slots
            .iter()
            .position(|s| s.thread.is_none() && !s.retired)
            .ok_or(Error::NoSlots)?;
        let slot = &mut self.slots[index];
        slot.thread = Some(Thread::new(ThreadId(id), priority, entry, arg));
        self.next_id = next;
        self.live += 1;
        Ok(Handle {
            index: index as u32,
            generation: slot.generation,
        })
    }

    fn slot(&self, h: Handle) -> Result<&Slot, Error> {
        match self.slots.get(h.index()) {
            Some(s) if s.generation == h.generation && s.thread.is_some() => Ok(s),
            _ => Err(Error::Stale),
        }
    }

    pub fn get(&self, h: Handle) -> Result<&Thread, Error> {
        self.slot(h)?.thread.as_ref().ok_or(Error::Stale)
    }

    pub fn get_mut(&mut self, h: Handle) -> Result<&mut Thread, Error> {
        self.slot(h)?;
        self.slots[h.index()].thread.as_mut().ok_or(Error::Stale)
    }

    /// Handle for the live thread stored at `index`, if any.
    pub fn handle_at(&self, index: usize) -> Option<Handle> {
        let s = self.slots.get(index)?;
        s.thread.as_ref()?;
        Some(Handle {
            index: index as u32,
            generation: s.generation,
        })
    }

    fn free(&mut self, index: usize) {
        let slot = &mut self.slots[index];
        slot.thread = None;
        match slot.generation.checked_add(1) {
            Some(g) => slot.generation = g,
            None => slot.retired = true,
        }
        self.live -= 1;
    }

    /// Frees a thread that never left `New` (creation rollback).
    pub fn discard_new(&mut self, h: Handle) -> Result<(), Error> {
        let state = self.get(h)?.state;
        if state != State::New {
            return Err(Error::InvalidTransition {
                from: state,
                to: State::Terminated,
            });
        }
        self.free(h.index());
        Ok(())
    }

    /// Reclaims a terminated thread and returns its exit status.
    pub fn reap(&mut self, h: Handle) -> Result<i32, Error> {
        let t = self.get(h)?;
        if t.state != State::Terminated {
            return Err(Error::NotTerminated);
        }
        if t.on_cpu {
            return Err(Error::StillOnCpu);
        }
        let status = t.exit_status.ok_or(Error::Corrupt)?;
        self.free(h.index());
        Ok(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [State; 6] = [
        State::New,
        State::Ready,
        State::Running,
        State::Blocked,
        State::Sleeping,
        State::Terminated,
    ];

    #[test]
    fn transition_table_is_exactly_the_documented_set() {
        use State::*;
        let allowed = [
            (New, Ready),
            (New, Terminated),
            (Ready, Running),
            (Ready, Terminated),
            (Running, Ready),
            (Running, Blocked),
            (Running, Sleeping),
            (Running, Terminated),
            (Blocked, Ready),
            (Blocked, Terminated),
            (Sleeping, Ready),
            (Sleeping, Terminated),
        ];
        for from in ALL {
            for to in ALL {
                assert_eq!(
                    from.can_become(to),
                    allowed.contains(&(from, to)),
                    "{from:?}->{to:?}"
                );
            }
        }
    }

    #[test]
    fn terminated_is_final_and_blocked_cannot_run_directly() {
        for to in ALL {
            assert!(!State::Terminated.can_become(to));
        }
        assert!(!State::Blocked.can_become(State::Running));
        assert!(!State::Sleeping.can_become(State::Running));
    }

    #[test]
    fn priority_bounds() {
        assert_eq!(Priority::new(0), Err(Error::BadPriority));
        assert_eq!(
            Priority::new(PRIORITY_LEVELS as u8),
            Err(Error::BadPriority)
        );
        assert_eq!(Priority::new(1), Ok(Priority::MIN));
        assert_eq!(Priority::new(7), Ok(Priority::MAX));
    }

    #[test]
    fn set_state_rejects_invalid() {
        let mut t = ThreadTable::<2>::new();
        let h = t.create(Priority::DEFAULT, 0, 0).unwrap();
        let th = t.get_mut(h).unwrap();
        assert_eq!(
            th.set_state(State::Running),
            Err(Error::InvalidTransition {
                from: State::New,
                to: State::Running
            })
        );
        th.set_state(State::Ready).unwrap();
        assert_eq!(th.state(), State::Ready);
    }

    #[test]
    fn ids_are_unique_and_never_reused() {
        let mut t = ThreadTable::<2>::new();
        let a = t.create(Priority::DEFAULT, 0, 0).unwrap();
        let id_a = t.get(a).unwrap().id();
        t.discard_new(a).unwrap();
        let b = t.create(Priority::DEFAULT, 0, 0).unwrap();
        assert_ne!(t.get(b).unwrap().id(), id_a);
    }

    #[test]
    fn stale_handle_rejected_after_slot_reuse() {
        let mut t = ThreadTable::<1>::new();
        let a = t.create(Priority::DEFAULT, 0, 0).unwrap();
        t.discard_new(a).unwrap();
        assert_eq!(t.get(a).err(), Some(Error::Stale));
        let b = t.create(Priority::DEFAULT, 0, 0).unwrap();
        assert_eq!(a.index(), b.index());
        assert_eq!(t.get(a).err(), Some(Error::Stale));
        assert!(t.get(b).is_ok());
    }

    #[test]
    fn capacity_exhaustion_is_an_error_not_a_panic() {
        let mut t = ThreadTable::<2>::new();
        t.create(Priority::DEFAULT, 0, 0).unwrap();
        t.create(Priority::DEFAULT, 0, 0).unwrap();
        assert_eq!(
            t.create(Priority::DEFAULT, 0, 0).err(),
            Some(Error::NoSlots)
        );
        assert_eq!(t.live(), 2);
    }

    #[test]
    fn id_exhaustion_is_reported_and_does_not_leak_a_slot() {
        let mut t = ThreadTable::<2>::new();
        t.next_id = u64::MAX;
        assert_eq!(
            t.create(Priority::DEFAULT, 0, 0).err(),
            Some(Error::IdExhausted)
        );
        assert_eq!(t.live(), 0);
    }

    #[test]
    fn slot_with_exhausted_generation_is_retired() {
        let mut t = ThreadTable::<2>::new();
        t.slots[0].generation = u32::MAX;
        let a = t.create(Priority::DEFAULT, 0, 0).unwrap();
        assert_eq!(a.index(), 0);
        t.discard_new(a).unwrap();
        let b = t.create(Priority::DEFAULT, 0, 0).unwrap();
        assert_eq!(b.index(), 1, "retired slot must not be reused");
    }

    #[test]
    fn reap_requires_terminated_and_off_cpu() {
        let mut t = ThreadTable::<2>::new();
        let h = t.create(Priority::DEFAULT, 0, 0).unwrap();
        assert_eq!(t.reap(h), Err(Error::NotTerminated));
        {
            let th = t.get_mut(h).unwrap();
            th.set_state(State::Ready).unwrap();
            th.set_state(State::Running).unwrap();
            th.on_cpu = true;
            th.exit_status = Some(7);
            th.set_state(State::Terminated).unwrap();
        }
        assert_eq!(t.reap(h), Err(Error::StillOnCpu));
        t.get_mut(h).unwrap().on_cpu = false;
        assert_eq!(t.reap(h), Ok(7));
        assert_eq!(t.get(h).err(), Some(Error::Stale));
        assert_eq!(t.reap(h), Err(Error::Stale));
    }

    #[test]
    fn effective_level_is_capped_and_idle_is_fixed() {
        let mut t = ThreadTable::<2>::new();
        let h = t.create(Priority::MAX, 0, 0).unwrap();
        t.get_mut(h).unwrap().boost = 5;
        assert_eq!(t.get(h).unwrap().effective_level(), PRIORITY_LEVELS - 1);
        let i = t.create(Priority::IDLE, 0, 0).unwrap();
        t.get_mut(i).unwrap().boost = 5;
        assert_eq!(t.get(i).unwrap().effective_level(), 0);
    }
}
