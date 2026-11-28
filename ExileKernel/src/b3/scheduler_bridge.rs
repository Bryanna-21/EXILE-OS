//! Scheduler bridge for blocking IPC (Mission B4). SPDX-License-Identifier: Apache-2.0
//!
//! B3's [`IpcDomain`](super::ipc::IpcDomain) only does scheduler *bookkeeping*: a blocking
//! operation marks the calling thread `Blocked` (`Scheduler::block_current`) and a completing
//! operation marks the waiter `Ready` (`Scheduler::wake`). Nothing in B3 gives up the CPU. This
//! module supplies the missing half in terms of the existing D1-D3 scheduler:
//!
//! * [`SchedPort`] is the kernel's view of "the scheduler plus the ability to switch away". The
//!   real implementation, [`DispatchPort`], is a thin adapter over `dispatch::KernelSlot`: the
//!   scheduler lock for bookkeeping, `dispatch::reschedule` for the actual context switch.
//! * [`park`] is called after an operation reported `Blocked`. It switches away, and on return
//!   verifies that this thread really is the running thread again; a switch that fails or comes
//!   back with the thread still blocked is an error, never silently ignored.
//! * [`interrupt_blocked`] wakes a thread that is blocked **in IPC** (event ids from
//!   `IPC_EVENT_BASE`) and refuses every other thread, so an interruption can neither disturb a
//!   thread blocked elsewhere (`d5::sync`, sleep, join) nor wake the same waiter twice.
//!
//! Priority and FIFO order are untouched: wake-up order is decided by the domain (highest
//! effective level, then FIFO) and the scheduler's own run queues.
//!
//! # The only `unsafe`
//! `DispatchPort::new` is `unsafe` because `dispatch::reschedule` has a contract the type system
//! cannot check (interrupts disabled, static kernel slot, no lock held, single CPU). The single
//! `unsafe` block in `switch_away` relies on exactly that contract and nothing else.
#![allow(dead_code)]

use super::ipc::IPC_EVENT_BASE;
use crate::sched::{Scheduler, WakeOutcome};
use crate::thread::{self, BlockReason, Handle, State};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeError {
    /// No thread is running.
    NoCurrent,
    /// The running thread is an idle thread, which cannot make IPC calls.
    IdleThread,
    /// `interrupt_blocked` found a thread that is not blocked in IPC.
    NotIpcBlocked,
    /// The context switch itself failed.
    SwitchFailed,
    /// The switch returned but this thread is not the running thread (or is still blocked).
    NotResumed,
    Sched(thread::Error),
}

impl From<thread::Error> for BridgeError {
    fn from(e: thread::Error) -> Self {
        BridgeError::Sched(e)
    }
}

/// The scheduler as the IPC integration sees it.
pub trait SchedPort<const N: usize> {
    /// Runs `f` with the scheduler lock held. `f` does bookkeeping only: it must not switch
    /// contexts or block.
    fn with_sched<R>(&self, f: impl FnOnce(&mut Scheduler<N>) -> R) -> R;

    /// Whether the scheduler exists and has its idle thread, i.e. threads can be blocked and
    /// switched. Never panics and never blocks; `with_sched` may only be used when this holds.
    fn is_ready(&self) -> bool;

    /// Gives up the CPU until the scheduler selects this thread again, then returns. Must be
    /// called with no spin lock held. A thread that was marked `Blocked` is only selected
    /// again after a wake-up.
    fn switch_away(&self) -> Result<(), BridgeError>;
}

/// The running, non-idle thread.
pub fn current_thread<const N: usize, P: SchedPort<N>>(port: &P) -> Result<Handle, BridgeError> {
    port.with_sched(|s| {
        let cur = s.current().ok_or(BridgeError::NoCurrent)?;
        if s.thread(cur)?.is_idle() {
            return Err(BridgeError::IdleThread);
        }
        Ok(cur)
    })
}

/// Switches away after an operation blocked `me`, and checks that `me` is running again when
/// control comes back.
pub fn park<const N: usize, P: SchedPort<N>>(port: &P, me: Handle) -> Result<(), BridgeError> {
    port.switch_away()?;
    port.with_sched(|s| {
        let running = s.current() == Some(me) && s.thread(me)?.state() == State::Running;
        if running {
            Ok(())
        } else {
            Err(BridgeError::NotResumed)
        }
    })
}

/// Whether a block reason is one of the IPC domain's event ids.
pub const fn is_ipc_event(reason: BlockReason) -> bool {
    match reason {
        BlockReason::Event(e) => e.wrapping_sub(IPC_EVENT_BASE) < 0x1_0000,
        _ => false,
    }
}

/// Wakes `h` if (and only if) it is currently blocked in IPC. The IPC call it was blocked in
/// then completes as `Interrupted` (see `IpcDomain::resume`). Any other state, including a
/// thread that was already woken, is refused.
pub fn interrupt_blocked<const N: usize, P: SchedPort<N>>(
    port: &P,
    h: Handle,
) -> Result<WakeOutcome, BridgeError> {
    port.with_sched(|s| {
        let t = s.thread(h)?;
        if t.state() != State::Blocked || !is_ipc_event(t.block_reason()) {
            return Err(BridgeError::NotIpcBlocked);
        }
        Ok(s.wake(h)?)
    })
}

#[cfg(target_arch = "x86_64")]
#[allow(unused_imports)]
pub use real::DispatchPort;

#[cfg(target_arch = "x86_64")]
mod real {
    use super::*;
    use crate::dispatch::{self, KernelSlot};
    use crate::kthread::StackMemory;

    /// [`SchedPort`] over the kernel's scheduler slot.
    pub struct DispatchPort<'a, const N: usize, M: StackMemory> {
        slot: &'a KernelSlot<N, M>,
    }

    impl<'a, const N: usize, M: StackMemory> DispatchPort<'a, N, M> {
        /// # Safety
        /// The contract of `dispatch::reschedule` must hold for every `switch_away` call made
        /// through this port: interrupts are disabled until it returns, `slot` is a `static`
        /// that is not moved while any thread is switched out and through which every thread
        /// was created, the context-switch hooks are installed, no spin lock is held, and there
        /// is only one CPU.
        pub unsafe fn new(slot: &'a KernelSlot<N, M>) -> Self {
            Self { slot }
        }

        pub fn slot(&self) -> &'a KernelSlot<N, M> {
            self.slot
        }

        /// Whether the scheduler has been installed (used by initialisation).
        pub fn is_installed(&self) -> bool {
            self.slot.is_installed()
        }
    }

    impl<const N: usize, M: StackMemory> SchedPort<N> for DispatchPort<'_, N, M> {
        fn with_sched<R>(&self, f: impl FnOnce(&mut Scheduler<N>) -> R) -> R {
            self.slot.with(|k| f(k.sched_mut()))
        }

        fn is_ready(&self) -> bool {
            self.slot.is_installed() && self.slot.with(|k| k.sched().idle().is_some())
        }

        fn switch_away(&self) -> Result<(), BridgeError> {
            // SAFETY: `DispatchPort::new` made its caller promise the `reschedule` contract
            // for all uses of this port; this call adds no further requirement.
            unsafe { dispatch::reschedule(self.slot) }.map_err(|_| BridgeError::SwitchFailed)
        }
    }
}

/// Test fixtures shared by the B4 modules.
#[cfg(test)]
pub mod testkit {
    use super::*;
    use crate::thread::Priority;
    use std::cell::{Cell, RefCell};
    use std::vec::Vec;

    /// A [`SchedPort`] over a bare `Scheduler` for single-threaded tests. `switch_away` models a
    /// context switch (`schedule` + `switch_complete`) and returns at once, so a thread that was
    /// blocked is *not* running afterwards: tests drive "other threads" by hand and call the
    /// completion half themselves. The genuine switching tests are in `real` below.
    pub struct ModelPort<const N: usize> {
        s: RefCell<Scheduler<N>>,
        threads: Vec<Handle>,
        pub fail_switch: Cell<bool>,
        pub switches: Cell<usize>,
    }

    impl<const N: usize> ModelPort<N> {
        /// Idle thread plus one thread per priority, nothing running yet.
        pub fn new(prios: &[u8]) -> Self {
            let mut s = Scheduler::<N>::new();
            s.init_idle(0, 0).unwrap();
            let threads = prios
                .iter()
                .map(|&p| s.spawn(Priority::new(p).unwrap(), 0, 0).unwrap())
                .collect();
            Self {
                s: RefCell::new(s),
                threads,
                fail_switch: Cell::new(false),
                switches: Cell::new(0),
            }
        }

        pub fn thread(&self, i: usize) -> Handle {
            self.threads[i]
        }

        pub fn sched<R>(&self, f: impl FnOnce(&mut Scheduler<N>) -> R) -> R {
            f(&mut self.s.borrow_mut())
        }

        pub fn sched_locked(&self) -> bool {
            self.s.try_borrow_mut().is_err()
        }

        pub fn run_next(&self) -> Handle {
            let mut s = self.s.borrow_mut();
            let sw = s.schedule().unwrap();
            if sw.is_switch()
                && let Some(prev) = sw.prev
            {
                s.switch_complete(prev).unwrap();
            }
            sw.next
        }

        pub fn run_until(&self, h: Handle) {
            for _ in 0..50 {
                if self.sched(|s| s.current()) == Some(h) {
                    return;
                }
                self.run_next();
            }
            panic!("thread never became current");
        }
    }

    impl<const N: usize> SchedPort<N> for ModelPort<N> {
        fn with_sched<R>(&self, f: impl FnOnce(&mut Scheduler<N>) -> R) -> R {
            f(&mut self.s.borrow_mut())
        }

        fn is_ready(&self) -> bool {
            self.s.borrow().idle().is_some()
        }

        fn switch_away(&self) -> Result<(), BridgeError> {
            if self.fail_switch.get() {
                return Err(BridgeError::SwitchFailed);
            }
            self.switches.set(self.switches.get() + 1);
            self.run_next();
            Ok(())
        }
    }

    /// Real context switches on the x86-64 host, the same technique as `dispatch::real_tests`:
    /// every thread runs on its own host-allocated stack and the scheduler switches between them
    /// with the real assembly. Cooperative, single OS thread, serialised by `HOST_SWITCH_LOCK`.
    /// This is not a bare-metal, emulator or user-mode test.
    #[cfg(target_arch = "x86_64")]
    pub mod real {
        use super::super::DispatchPort;
        use crate::dispatch::{self, Kernel, KernelSlot};
        use crate::kstack::{GUARD_BYTES, StackPool};
        use crate::kthread::{Config, MemError, StackMemory, ThreadParams};
        use crate::switch::{self, HOST_SWITCH_LOCK};
        use crate::thread::{Handle, Priority};
        use std::boxed::Box;
        use std::string::String;
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::{Mutex, MutexGuard};
        use std::vec;
        use std::vec::Vec;

        pub const SLOTS: u64 = 6;
        pub const STACK: u64 = 64 * 1024;
        pub const THREADS: usize = 8;

        /// Host stack memory: addresses are real host addresses inside a leaked buffer.
        pub struct HostMem {
            lo: usize,
            hi: usize,
        }

        impl HostMem {
            fn span(&self, addr: u64, len: usize) -> Result<usize, MemError> {
                let a = addr as usize;
                let e = a.checked_add(len).ok_or(MemError)?;
                if a < self.lo || e > self.hi {
                    return Err(MemError);
                }
                Ok(a)
            }
        }

        impl StackMemory for HostMem {
            fn write(&mut self, addr: u64, d: &[u8]) -> Result<(), MemError> {
                let a = self.span(addr, d.len())?;
                // SAFETY: [a, a+len) lies inside the leaked host buffer [lo, hi).
                unsafe { core::ptr::copy_nonoverlapping(d.as_ptr(), a as *mut u8, d.len()) };
                Ok(())
            }
            fn zero(&mut self, addr: u64, len: u64) -> Result<(), MemError> {
                let a = self.span(addr, len as usize)?;
                // SAFETY: as above.
                unsafe { core::ptr::write_bytes(a as *mut u8, 0, len as usize) };
                Ok(())
            }
        }

        pub static K: KernelSlot<THREADS, HostMem> = KernelSlot::new();
        static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
        pub static IDLE_ITERS: AtomicU64 = AtomicU64::new(0);

        extern "C" fn first_run() {
            K.with(|k| k.complete_switch().expect("complete_switch"));
        }

        extern "C" fn exit_hook(status: i32) -> ! {
            // SAFETY: single-OS-thread cooperative scenario under HOST_SWITCH_LOCK; hooks
            // installed; K is a static.
            unsafe { dispatch::exit_current(&K, status) }
        }

        /// Installs a fresh kernel and the switch hooks; hold the guard for the whole test.
        pub fn setup() -> MutexGuard<'static, ()> {
            let g = HOST_SWITCH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let total = (SLOTS * (STACK + GUARD_BYTES)) as usize;
            let buf: &'static mut [u8] = Box::leak(vec![0u8; total + 4096].into_boxed_slice());
            let base = (buf.as_ptr() as usize + 4095) & !4095;
            let pool = StackPool::new(base as u64, total as u64, STACK, 1 << 47).unwrap();
            let cfg = Config {
                text_start: 0x1000,
                text_end: 0x7fff_ffff_f000,
                trampoline: switch::trampoline_address(),
            };
            let mem = HostMem {
                lo: base,
                hi: base + total,
            };
            K.install(Kernel::new(cfg, pool, mem).unwrap());
            LOG.lock().unwrap().clear();
            IDLE_ITERS.store(0, Ordering::SeqCst);
            switch::install_hooks(first_run, exit_hook);
            g
        }

        pub fn log(s: impl Into<String>) {
            LOG.lock().unwrap().push(s.into());
        }

        pub fn logged() -> Vec<String> {
            LOG.lock().unwrap().clone()
        }

        fn fp(f: extern "C" fn(u64) -> i32) -> u64 {
            f as *const () as usize as u64
        }

        pub fn spawn(f: extern "C" fn(u64) -> i32, arg: u64, prio: u8) -> Handle {
            K.with(|k| {
                k.create(ThreadParams {
                    entry: fp(f),
                    arg,
                    priority: Priority::new(prio).unwrap(),
                })
                .unwrap()
            })
        }

        /// The default idle thread: reaps terminated threads, leaves when only idle remains,
        /// otherwise ticks and yields (so sleepers and need-resched are honoured).
        extern "C" fn idle(_: u64) -> i32 {
            loop {
                let live = K.with(|k| {
                    k.reap_terminated(|_, _| {});
                    k.sched().live_threads()
                });
                if live == 1 {
                    // SAFETY: idle context, the bootstrap context is suspended in `start`.
                    unsafe { dispatch::leave_to_boot(&K) };
                }
                if IDLE_ITERS.fetch_add(1, Ordering::SeqCst) > 200_000 {
                    // SAFETY: as above.
                    unsafe { dispatch::leave_to_boot(&K) };
                }
                dispatch::tick(&K);
                // SAFETY: cooperative single-CPU scenario.
                unsafe { dispatch::yield_now(&K) }.unwrap();
            }
        }

        /// Creates the idle thread and runs every thread to completion.
        pub fn start() {
            K.with(|k| k.create_idle(fp(idle), 0).unwrap());
            // SAFETY: K is a static, hooks are installed, nothing else runs.
            unsafe { dispatch::reschedule(&K) }.unwrap();
            assert!(
                IDLE_ITERS.load(Ordering::SeqCst) <= 200_000,
                "scenario did not finish"
            );
            K.with(|k| {
                assert_eq!(k.sched().live_threads(), 1, "only idle remains");
                k.check_accounting().unwrap();
                k.sched().check_invariants().unwrap();
            });
        }

        /// The port the scenarios use.
        pub fn port() -> DispatchPort<'static, THREADS, HostMem> {
            // SAFETY: the scenarios run cooperatively on one OS thread under HOST_SWITCH_LOCK
            // with the hooks installed by `setup`, K is a static, interrupts do not exist on
            // the host, and no spin lock is held across `switch_away`.
            unsafe { DispatchPort::new(&K) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::ModelPort;
    use super::*;

    const EV: BlockReason = BlockReason::Event(IPC_EVENT_BASE + 3);

    #[test]
    fn current_thread_requires_a_running_non_idle_thread() {
        let p = ModelPort::<4>::new(&[4]);
        assert_eq!(current_thread(&p), Err(BridgeError::NoCurrent));
        p.run_next(); // the only runnable thread is the spawned one, idle waits
        let t = p.thread(0);
        assert_eq!(current_thread(&p), Ok(t));
        // make the idle thread current: block the only thread, then schedule
        p.sched(|s| s.block_current(EV).unwrap());
        p.run_next();
        assert_eq!(current_thread(&p), Err(BridgeError::IdleThread));
    }

    #[test]
    fn park_reports_failed_switches_and_threads_that_did_not_resume() {
        let p = ModelPort::<4>::new(&[4, 4]);
        let (a, _b) = (p.thread(0), p.thread(1));
        p.run_until(a);
        p.sched(|s| s.block_current(EV).unwrap());
        // the model switch selects the other thread, so `a` is not running when it returns
        assert_eq!(park(&p, a), Err(BridgeError::NotResumed));
        assert_eq!(p.switches.get(), 1);
        p.fail_switch.set(true);
        assert_eq!(park(&p, a), Err(BridgeError::SwitchFailed));
        assert_eq!(p.switches.get(), 1, "a failed switch is not counted");
    }

    #[test]
    fn park_succeeds_when_the_thread_is_selected_again() {
        let p = ModelPort::<4>::new(&[4]);
        let a = p.thread(0);
        p.run_until(a);
        // a thread that did not block is simply selected again by the switch
        assert_eq!(park(&p, a), Ok(()));
    }

    #[test]
    fn a_wake_that_lands_before_the_switch_is_not_lost() {
        // alone: the wake makes the thread the best candidate, so the switch re-selects it
        let p = ModelPort::<4>::new(&[4]);
        let a = p.thread(0);
        p.run_until(a);
        p.sched(|s| s.block_current(EV).unwrap());
        p.sched(|s| s.wake(a).unwrap()); // e.g. an interrupt between "blocked" and "switched"
        assert_eq!(park(&p, a), Ok(()));
        // with an equal-priority thread queued first, that one runs first; the woken thread is
        // Ready (never stranded Blocked) and gets the CPU next. (The model's `switch_away`
        // returns immediately, so `park` reports NotResumed here; on the real stack switch the
        // call simply returns when the thread is scheduled again.)
        let p = ModelPort::<4>::new(&[4, 4]);
        let a = p.thread(0);
        p.run_until(a);
        p.sched(|s| s.block_current(EV).unwrap());
        p.sched(|s| s.wake(a).unwrap());
        assert_eq!(park(&p, a), Err(BridgeError::NotResumed));
        assert_eq!(
            p.sched(|s| s.thread(a).map(|t| t.state())),
            Ok(crate::thread::State::Ready)
        );
        p.run_until(a);
    }

    #[test]
    fn interrupt_wakes_only_threads_blocked_in_ipc_and_only_once() {
        let p = ModelPort::<8>::new(&[4, 4, 4, 4, 4]);
        let ts: [Handle; 5] = core::array::from_fn(|i| p.thread(i));
        let reasons = [
            EV,
            BlockReason::Event(7), // a different domain (d5::sync ids are bare slot numbers)
            BlockReason::Event(IPC_EVENT_BASE + 0x1_0000), // just outside the IPC range
            BlockReason::Sleep,
        ];
        for (i, r) in reasons.iter().enumerate() {
            p.run_until(ts[i]);
            p.sched(|s| match r {
                BlockReason::Sleep => {
                    s.sleep_current(100).unwrap();
                }
                r => {
                    s.block_current(*r).unwrap();
                }
            });
        }
        assert_eq!(interrupt_blocked(&p, ts[0]), Ok(WakeOutcome::Woken));
        assert_eq!(
            interrupt_blocked(&p, ts[0]),
            Err(BridgeError::NotIpcBlocked),
            "second wake-up refused: the thread is Ready now"
        );
        for t in &ts[1..4] {
            assert_eq!(interrupt_blocked(&p, *t), Err(BridgeError::NotIpcBlocked));
        }
        // the running thread is not blocked either
        p.run_until(ts[4]);
        assert_eq!(
            interrupt_blocked(&p, ts[4]),
            Err(BridgeError::NotIpcBlocked)
        );
        p.sched(|s| s.check_invariants().unwrap());
    }

    #[test]
    fn ipc_event_range_boundaries() {
        assert!(is_ipc_event(BlockReason::Event(IPC_EVENT_BASE)));
        assert!(is_ipc_event(BlockReason::Event(IPC_EVENT_BASE + 0xFFFF)));
        assert!(!is_ipc_event(BlockReason::Event(IPC_EVENT_BASE + 0x1_0000)));
        assert!(!is_ipc_event(BlockReason::Event(IPC_EVENT_BASE - 1)));
        assert!(!is_ipc_event(BlockReason::Event(0)));
        assert!(!is_ipc_event(BlockReason::None));
        assert!(!is_ipc_event(BlockReason::Sleep));
    }

    #[cfg(target_arch = "x86_64")]
    mod real_switch {
        use super::super::testkit::real::*;
        use super::super::*;
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicBool, Ordering};

        static WAITER: Mutex<Option<Handle>> = Mutex::new(None);
        static WOKEN: AtomicBool = AtomicBool::new(false);

        extern "C" fn waiter(_: u64) -> i32 {
            let p = port();
            let me = current_thread(&p).unwrap();
            *WAITER.lock().unwrap() = Some(me);
            log("W: blocking");
            p.with_sched(|s| s.block_current(super::EV)).unwrap();
            // the thread is Blocked: park really gives the CPU to someone else
            park(&p, me).unwrap();
            log("W: resumed");
            WOKEN.store(true, Ordering::SeqCst);
            0
        }

        extern "C" fn waker(_: u64) -> i32 {
            let p = port();
            log("K: running while W is blocked");
            let w = WAITER.lock().unwrap().expect("waiter registered first");
            assert_eq!(interrupt_blocked(&p, w), Ok(WakeOutcome::Woken));
            assert_eq!(
                interrupt_blocked(&p, w),
                Err(BridgeError::NotIpcBlocked),
                "no second wake-up"
            );
            log("K: woke W");
            0
        }

        #[test]
        fn blocked_thread_really_yields_and_resumes_after_a_wake() {
            let _g = setup();
            *WAITER.lock().unwrap() = None;
            WOKEN.store(false, Ordering::SeqCst);
            // equal priority and FIFO: the waiter is created (and so runs) first
            spawn(waiter, 0, 4);
            spawn(waker, 0, 4);
            start();
            assert!(WOKEN.load(Ordering::SeqCst));
            assert_eq!(
                logged(),
                [
                    "W: blocking",
                    "K: running while W is blocked",
                    "K: woke W",
                    "W: resumed"
                ]
            );
        }
    }
}
