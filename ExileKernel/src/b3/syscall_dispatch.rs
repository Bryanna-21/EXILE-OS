//! System-call routing into kernel IPC (Mission B4). SPDX-License-Identifier: Apache-2.0
//!
//! [`dispatch`] is the IPC-aware replacement for `usermode::dispatch` on the kernel's syscall
//! path, and [`handle_frame`] the replacement for `usermode::handle_syscall`:
//!
//! * numbers outside `0x40..=0x48` are handed to D5's own `dispatch` **unchanged**, so every D5
//!   behaviour (including `ENOSYS` for unknown numbers) is preserved by construction;
//! * `0x40..=0x48` are run through the B3 ABI (`ipc_abi::handle`), which validates arguments and
//!   user pointers before it touches any state, against the one authoritative
//!   [`KernelDomain`](super::kernel_domain::KernelDomain);
//! * the return frame is built with D5's `apply_outcome`, so privilege checks on the frame and
//!   the choice between `sysret` and `iretq` are D5's.
//!
//! # Flow of one IPC system call
//! 1. *Resolve*: the calling thread is the scheduler's current non-idle thread; its owner is the
//!    process that thread belongs to (`owner_of`, a closure so the process table is not held
//!    across a context switch). No thread or no process: `ESRCH`. Domain not initialised:
//!    `ENOSYS`, nothing touched.
//! 2. *Execute* (domain lock, then scheduler lock): `PendingTable::begin`, the `ipc_abi::handle`
//!    call through a [`KernelIpcService`], and -- if the call blocked the thread -- the pending
//!    record is armed **in the same critical section**. An operation that finishes immediately
//!    returns its value here (direct hand-off, queued delivery, errors).
//! 3. *Park* (no lock held): `scheduler_bridge::park` gives the CPU to another thread and
//!    returns only when the scheduler has selected this thread again.
//! 4. *Resume* (locks again): `resume::finish_blocked` takes this thread's pending record, asks
//!    the domain how the operation ended, writes the original user buffer through the updated
//!    `ipc_abi::complete` and yields the value for RAX.
//!
//! Results after a blocked call: the operation's own result (`0`, `EPIPE`, `ECANCELED`,
//! `EFAULT`, ...); `EINTR` when the thread was interrupted or when its pending record is gone
//! (process teardown, subsystem shutdown); `EIO` for a protocol violation. If the context switch
//! itself fails (`park` error) the kernel's switching machinery is broken -- the same condition
//! D5's `dispatch::reschedule` treats as fatal -- and the thread's IPC records are dropped
//! before `EIO` is returned.
//!
//! `mem` **must** be the address space of the calling process (see `SpaceMemory::for_process`);
//! the dispatcher cannot check that a foreign memory object was not passed in.
#![allow(dead_code)]

use super::ipc_abi::{self, Abi, EINTR, EIO, ESRCH};
use super::ipc_service::KernelIpcService;
use super::kernel_domain::{DomainError, IpcKernel};
use super::resume::{ResumeError, finish_blocked};
use super::scheduler_bridge::{SchedPort, current_thread, park};
use super::thread_lifecycle::cleanup_thread;
use crate::frame::TrapFrame;
use crate::thread::Handle;
use crate::usermode::{
    FrameDisposition, SyscallArgs, SyscallError, SyscallHost, SyscallOutcome, UserMemory,
    apply_outcome, validate_return_frame,
};

const ENOSYS: i64 = SyscallError::NoSys as i64;

/// Routes one system call. See the module documentation.
pub fn dispatch<
    H: SyscallHost,
    M: UserMemory,
    const N: usize,
    const O: usize,
    const C: usize,
    const Q: usize,
    P: SchedPort<N>,
>(
    host: &mut H,
    mem: &mut M,
    args: &SyscallArgs,
    ipc: &IpcKernel<'_, N, O, C, Q, P>,
    owner_of: impl FnOnce(Handle) -> Option<u64>,
) -> SyscallOutcome {
    if !ipc_abi::owns(args.nr) {
        return crate::usermode::dispatch(host, &*mem, args);
    }
    SyscallOutcome::Return(ipc_syscall(mem, args, ipc, owner_of))
}

/// `usermode::handle_syscall` with IPC: decode the frame, dispatch, store RAX, re-validate.
pub fn handle_frame<
    H: SyscallHost,
    M: UserMemory,
    const N: usize,
    const O: usize,
    const C: usize,
    const Q: usize,
    P: SchedPort<N>,
>(
    frame: &mut TrapFrame,
    host: &mut H,
    mem: &mut M,
    ipc: &IpcKernel<'_, N, O, C, Q, P>,
    owner_of: impl FnOnce(Handle) -> Option<u64>,
) -> FrameDisposition {
    if let Err(e) = validate_return_frame(frame) {
        return FrameDisposition::Kill(e);
    }
    let outcome = dispatch(host, mem, &SyscallArgs::from_frame(frame), ipc, owner_of);
    apply_outcome(frame, outcome)
}

enum Step {
    Done(i64),
    Parked,
}

/// The IPC half of [`dispatch`]: returns the RAX value of a `0x40..=0x48` call.
fn ipc_syscall<
    M: UserMemory,
    const N: usize,
    const O: usize,
    const C: usize,
    const Q: usize,
    P: SchedPort<N>,
>(
    mem: &mut M,
    args: &SyscallArgs,
    ipc: &IpcKernel<'_, N, O, C, Q, P>,
    owner_of: impl FnOnce(Handle) -> Option<u64>,
) -> i64 {
    // Cheap early refusal for an uninitialised subsystem: no scheduler access at all (the
    // scheduler itself might not exist yet). The check under the lock below is authoritative.
    if !ipc.domain.is_active() {
        return ENOSYS;
    }
    let Ok(me) = current_thread(ipc.port) else {
        return ESRCH;
    };
    let Some(owner) = owner_of(me) else {
        return ESRCH;
    };

    let step = ipc.locked(|st, s| {
        let Ok(ticket) = st.pending.begin(me) else {
            // A live pending record of the running thread: impossible unless state is corrupt.
            return Step::Done(EIO);
        };
        let mut svc = KernelIpcService::new(&mut st.domain, s, me, owner);
        match ipc_abi::handle(&mut svc, mem, args) {
            Abi::Done(v) => Step::Done(v),
            Abi::Blocked(p) => {
                st.pending.arm(ticket, owner, p);
                Step::Parked
            }
        }
    });
    match step {
        Ok(Step::Done(v)) => return v,
        Ok(Step::Parked) => {}
        Err(DomainError::NotReady) => return ENOSYS,
        Err(_) => return EIO,
    }

    if park(ipc.port, me).is_err() {
        let _ = cleanup_thread(ipc, me);
        return EIO;
    }

    match ipc.locked(|st, s| finish_blocked(&mut st.domain, &mut st.pending, s, mem, me, owner)) {
        Ok(Ok(v)) => v,
        // The record was removed while the thread slept: its process was torn down.
        Ok(Err(ResumeError::NoPending)) => EINTR,
        Ok(Err(_)) => EIO,
        // Shut down while blocked.
        Err(DomainError::NotReady) => EINTR,
        Err(_) => EIO,
    }
}

#[cfg(test)]
mod tests {
    use super::super::cap_rights::Rights;
    use super::super::ipc::RecvOutcome;
    use super::super::ipc_abi::*;
    use super::super::ipc_message::{Delivery, UserMessage, WIRE_OUT};
    use super::super::kernel_domain::{IpcState, KernelDomain};
    use super::super::scheduler_bridge::testkit::ModelPort;
    use super::super::user_memory::SpaceMemory;
    use super::super::user_memory::tests::{Flaky, RO, RW, fixture};
    use super::*;
    use crate::usermode::{ReturnPath, Sysno, build_user_frame};

    type D = KernelDomain<4, 8, 32, 2>;
    const A: u64 = 1;
    const B: u64 = 2;

    #[derive(Default)]
    struct Host {
        exited: Option<i32>,
        yields: u32,
    }
    impl SyscallHost for Host {
        fn exit(&mut self, s: i32) {
            self.exited = Some(s);
        }
        fn yield_now(&mut self) -> Result<(), SyscallError> {
            self.yields += 1;
            Ok(())
        }
        fn sleep_ticks(&mut self, _: u64) -> Result<(), SyscallError> {
            Ok(())
        }
        fn getpid(&mut self) -> u64 {
            42
        }
        fn debug_write(&mut self, d: &[u8]) -> Result<usize, SyscallError> {
            Ok(d.len())
        }
    }

    fn call(
        port: &ModelPort<4>,
        d: &D,
        mem: &mut impl UserMemory,
        owner: Option<u64>,
        nr: u64,
        a: [u64; 3],
    ) -> i64 {
        let ipc = IpcKernel::new(d, port);
        let args = SyscallArgs {
            nr,
            a: [a[0], a[1], a[2], 0, 0, 0],
        };
        match dispatch(&mut Host::default(), mem, &args, &ipc, |_| owner) {
            SyscallOutcome::Return(v) => v,
            SyscallOutcome::Exit(_) => panic!("unexpected exit"),
        }
    }

    fn active() -> D {
        let d = D::new();
        d.install(IpcState::new()).unwrap();
        d
    }

    fn running() -> ModelPort<4> {
        let p = ModelPort::<4>::new(&[4, 4]);
        p.run_until(p.thread(0));
        p
    }

    #[test]
    fn numbers_outside_the_ipc_range_are_d5s_business_and_stay_unchanged() {
        let port = running();
        let d = active();
        let (mut fx, sp) = fixture();
        let mut mem = SpaceMemory::new(&sp, &mut fx.mem);
        let ipc = IpcKernel::new(&d, &port);
        let mut host = Host::default();
        let a = |nr| SyscallArgs { nr, a: [0; 6] };
        assert_eq!(
            dispatch(&mut host, &mut mem, &a(Sysno::Yield as u64), &ipc, |_| None),
            SyscallOutcome::Return(0)
        );
        assert_eq!(host.yields, 1);
        assert_eq!(
            dispatch(&mut host, &mut mem, &a(Sysno::GetPid as u64), &ipc, |_| {
                None
            }),
            SyscallOutcome::Return(42)
        );
        assert_eq!(
            dispatch(&mut host, &mut mem, &a(Sysno::Exit as u64), &ipc, |_| None),
            SyscallOutcome::Exit(0)
        );
        for nr in [0x3F, 0x49, 0x4A, 0x80, 0x100, u64::MAX, 5, 0x40 - 1] {
            assert_eq!(
                dispatch(&mut host, &mut mem, &a(nr), &ipc, |_| Some(A)),
                SyscallOutcome::Return(ENOSYS),
                "{nr:#x}"
            );
        }
    }

    #[test]
    fn an_uninitialised_or_shut_down_subsystem_answers_enosys_and_touches_nothing() {
        let port = running();
        let d = D::new();
        let (mut fx, sp) = fixture();
        let mut mem = SpaceMemory::new(&sp, &mut fx.mem);
        for nr in FIRST_SYSCALL..=LAST_SYSCALL {
            assert_eq!(call(&port, &d, &mut mem, Some(A), nr, [RW, RW, 0]), ENOSYS);
        }
        assert!(!d.is_active());
        // after shutdown too
        d.install(IpcState::new()).unwrap();
        d.take_down(|_| ()).unwrap();
        assert_eq!(
            call(&port, &d, &mut mem, Some(A), SYS_EP_CREATE, [0; 3]),
            ENOSYS
        );
    }

    /// A scheduler port that must never be used: models a kernel whose scheduler does not exist
    /// yet (IPC is not initialised, so a system call must not even look for the current thread).
    struct NoScheduler;
    impl SchedPort<4> for NoScheduler {
        fn with_sched<R>(&self, _: impl FnOnce(&mut crate::sched::Scheduler<4>) -> R) -> R {
            panic!("the scheduler was touched on behalf of an uninitialised IPC subsystem");
        }
        fn is_ready(&self) -> bool {
            false
        }
        fn switch_away(&self) -> Result<(), super::super::scheduler_bridge::BridgeError> {
            panic!("switch on behalf of an uninitialised IPC subsystem");
        }
    }

    #[test]
    fn ipc_calls_never_reach_the_scheduler_before_initialisation() {
        let d = D::new();
        let (mut fx, sp) = fixture();
        let mut mem = SpaceMemory::new(&sp, &mut fx.mem);
        let ipc = IpcKernel::new(&d, &NoScheduler);
        for nr in FIRST_SYSCALL..=LAST_SYSCALL {
            let args = SyscallArgs { nr, a: [RW; 6] };
            assert_eq!(
                dispatch(&mut Host::default(), &mut mem, &args, &ipc, |_| Some(A)),
                SyscallOutcome::Return(ENOSYS)
            );
        }
    }

    #[test]
    fn calls_without_a_current_thread_or_a_process_are_esrch() {
        let d = active();
        let (mut fx, sp) = fixture();
        let mut mem = SpaceMemory::new(&sp, &mut fx.mem);
        let idle_only = ModelPort::<4>::new(&[4]);
        assert_eq!(
            call(&idle_only, &d, &mut mem, Some(A), SYS_EP_CREATE, [0; 3]),
            ESRCH
        );
        let port = running();
        assert_eq!(
            call(&port, &d, &mut mem, None, SYS_EP_CREATE, [0; 3]),
            ESRCH
        );
        assert_eq!(
            d.with(|st| st.domain.live_objects()),
            Ok(0),
            "nothing was created"
        );
    }

    #[test]
    fn create_send_recv_close_round_trip_through_user_memory() {
        let port = running();
        let d = active();
        let (mut fx, sp) = fixture();
        let mut mem = SpaceMemory::new(&sp, &mut fx.mem);
        let ep = call(&port, &d, &mut mem, Some(A), SYS_EP_CREATE, [0; 3]);
        assert!(ep > 0);
        let msg = UserMessage::new(0x77, &[1, 2, 3], None).unwrap();
        mem.copy_to_user(RW, &msg.encode()).unwrap();
        assert_eq!(
            call(&port, &d, &mut mem, Some(A), SYS_SEND, [ep as u64, RW, 0]),
            0
        );
        // delivered into a different buffer, byte for byte
        let out = RW + 0x100;
        assert_eq!(
            call(&port, &d, &mut mem, Some(A), SYS_RECV, [ep as u64, out, 0]),
            0
        );
        let mut raw = [0u8; WIRE_OUT];
        mem.copy_from_user(out, &mut raw).unwrap();
        let dl = Delivery::decode(&raw).unwrap();
        assert_eq!((dl.label, dl.data()), (0x77, &[1, 2, 3][..]));
        assert_eq!(
            call(
                &port,
                &d,
                &mut mem,
                Some(A),
                SYS_RECV,
                [ep as u64, out, FLAG_NONBLOCK]
            ),
            EAGAIN
        );
        assert_eq!(
            call(&port, &d, &mut mem, Some(A), SYS_CLOSE, [ep as u64, 0, 0]),
            0
        );
        assert_eq!(
            call(&port, &d, &mut mem, Some(A), SYS_SEND, [ep as u64, RW, 0]),
            EBADF,
            "closed handle is stale"
        );
        assert_eq!(d.with(|st| st.domain.live_objects()), Ok(0));
    }

    #[test]
    fn bad_user_pointers_fault_before_any_state_changes() {
        let port = running();
        let d = active();
        let (mut fx, sp) = fixture();
        let mut mem = SpaceMemory::new(&sp, &mut fx.mem);
        let ep = call(&port, &d, &mut mem, Some(A), SYS_EP_CREATE, [0; 3]) as u64;
        let msg = UserMessage::new(1, &[], None).unwrap();
        mem.copy_to_user(RW, &msg.encode()).unwrap();
        assert_eq!(call(&port, &d, &mut mem, Some(A), SYS_SEND, [ep, RW, 0]), 0);
        let queued = |d: &D| d.with(|st| st.domain.queued(A, ep)).unwrap();
        assert_eq!(queued(&d), Ok(1));
        for bad in [
            0u64,
            8,
            0xFFFF_8000_0000_1000,
            u64::MAX - 7,
            0x60_0000,                               // unmapped
            RO,                                      // read-only: cannot be written
            RW + 2 * crate::address_space::PAGE - 8, // runs off the mapping
        ] {
            assert_eq!(
                call(&port, &d, &mut mem, Some(A), SYS_RECV, [ep, bad, 0]),
                EFAULT,
                "recv out={bad:#x}"
            );
            assert_eq!(
                queued(&d),
                Ok(1),
                "the message must not be consumed ({bad:#x})"
            );
        }
        for bad in [
            0u64,
            8,
            0xFFFF_8000_0000_1000,
            0x60_0000,
            RW + 2 * crate::address_space::PAGE - 8,
        ] {
            assert_eq!(
                call(&port, &d, &mut mem, Some(A), SYS_SEND, [ep, bad, 0]),
                EFAULT,
                "send msg={bad:#x}"
            );
        }
        assert_eq!(queued(&d), Ok(1), "failed sends enqueued nothing");
        let nf = call(&port, &d, &mut mem, Some(A), SYS_NF_CREATE, [0; 3]) as u64;
        assert_eq!(
            call(&port, &d, &mut mem, Some(A), SYS_SIGNAL, [nf, 0b1, 0]),
            0
        );
        assert_eq!(
            call(&port, &d, &mut mem, Some(A), SYS_WAIT, [nf, RO, 0]),
            EFAULT
        );
        // the signal word survived the refused wait
        assert_eq!(call(&port, &d, &mut mem, Some(A), SYS_WAIT, [nf, RW, 0]), 0);
        let mut w = [0u8; 8];
        mem.copy_from_user(RW, &mut w).unwrap();
        assert_eq!(u64::from_le_bytes(w), 1);
    }

    #[test]
    fn handles_of_another_process_and_malformed_arguments_are_rejected() {
        let port = running();
        let d = active();
        let (mut fx, sp) = fixture();
        let mut mem = SpaceMemory::new(&sp, &mut fx.mem);
        let ep = call(&port, &d, &mut mem, Some(A), SYS_EP_CREATE, [0; 3]) as u64;
        let msg = UserMessage::new(1, &[], None).unwrap();
        mem.copy_to_user(RW, &msg.encode()).unwrap();
        for nr in [SYS_SEND, SYS_RECV] {
            assert_eq!(call(&port, &d, &mut mem, Some(B), nr, [ep, RW, 0]), EBADF);
        }
        assert_eq!(
            call(&port, &d, &mut mem, Some(B), SYS_CLOSE, [ep, 0, 0]),
            EBADF
        );
        assert_eq!(
            call(&port, &d, &mut mem, Some(B), SYS_REVOKE, [ep, 0, 0]),
            EBADF
        );
        assert_eq!(
            call(&port, &d, &mut mem, Some(A), SYS_SEND, [ep, RW, 1 << 5]),
            EINVAL
        );
        assert_eq!(
            call(&port, &d, &mut mem, Some(A), SYS_SEND, [0, RW, 0]),
            EBADF
        );
        assert_eq!(
            call(&port, &d, &mut mem, Some(A), SYS_SEND, [u64::MAX, RW, 0]),
            EBADF
        );
        assert_eq!(d.with(|st| st.domain.queued(A, ep)), Ok(Ok(0)));
    }

    #[test]
    fn capability_transfer_and_failed_copy_out_through_dispatch() {
        let port = running();
        let d = active();
        let (fx, sp) = fixture();
        let mut phys = Flaky {
            inner: fx.mem,
            fail_writes: false,
            written: 0,
        };
        let mut mem = SpaceMemory::new(&sp, &mut phys);
        let ep = call(&port, &d, &mut mem, Some(A), SYS_EP_CREATE, [0; 3]) as u64;
        let nf = call(&port, &d, &mut mem, Some(A), SYS_NF_CREATE, [0; 3]) as u64;
        let narrow = call(
            &port,
            &d,
            &mut mem,
            Some(A),
            SYS_DERIVE,
            [nf, u64::from(Rights::SIGNAL.union(Rights::GRANT).bits()), 0],
        );
        assert!(narrow > 0);
        let msg = UserMessage::new(1, &[], Some(narrow as u64)).unwrap();
        mem.copy_to_user(RW, &msg.encode()).unwrap();
        assert_eq!(call(&port, &d, &mut mem, Some(A), SYS_SEND, [ep, RW, 0]), 0);
        let held = d.with(|st| st.domain.caps().owner_count(A)).unwrap();
        drop(mem);
        phys.fail_writes = true;
        let mut mem = SpaceMemory::new(&sp, &mut phys);
        assert_eq!(
            call(&port, &d, &mut mem, Some(A), SYS_RECV, [ep, RW + 0x100, 0]),
            EFAULT
        );
        d.with(|st| {
            assert_eq!(
                st.domain.caps().owner_count(A),
                held,
                "no capability leaked"
            );
            assert_eq!(st.domain.caps().limbo_count(), 0);
        })
        .unwrap();
        let _ = RecvOutcome::Blocked;
    }

    #[test]
    fn many_create_use_close_cycles_leak_nothing() {
        let port = running();
        let d = active();
        let (mut fx, sp) = fixture();
        let mut mem = SpaceMemory::new(&sp, &mut fx.mem);
        let msg = UserMessage::new(9, &[4], None).unwrap();
        mem.copy_to_user(RW, &msg.encode()).unwrap();
        for i in 0..300u64 {
            let ep = call(&port, &d, &mut mem, Some(A), SYS_EP_CREATE, [0; 3]) as u64;
            let nf = call(&port, &d, &mut mem, Some(A), SYS_NF_CREATE, [0; 3]) as u64;
            assert_eq!(call(&port, &d, &mut mem, Some(A), SYS_SEND, [ep, RW, 0]), 0);
            assert_eq!(
                call(
                    &port,
                    &d,
                    &mut mem,
                    Some(A),
                    SYS_SIGNAL,
                    [nf, 1 << (i % 60), 0]
                ),
                0
            );
            // some iterations fail on purpose
            if i % 3 == 0 {
                assert_eq!(
                    call(&port, &d, &mut mem, Some(A), SYS_RECV, [ep, 0, 0]),
                    EFAULT
                );
            }
            assert_eq!(call(&port, &d, &mut mem, Some(A), SYS_CLOSE, [ep, 0, 0]), 0);
            assert_eq!(call(&port, &d, &mut mem, Some(A), SYS_CLOSE, [nf, 0, 0]), 0);
        }
        d.with(|st| {
            assert_eq!(st.domain.live_objects(), 0);
            assert_eq!(st.domain.caps().owner_count(A), 0);
            assert!(st.pending.is_empty());
        })
        .unwrap();
        port.with_sched(|s| d.with(|st| st.domain.check_invariants(s)).unwrap().unwrap());
    }

    #[test]
    fn handle_frame_sets_rax_and_returns_by_d5s_rules() {
        let port = running();
        let d = active();
        let ipc = IpcKernel::new(&d, &port);
        let (mut fx, sp) = fixture();
        let mut mem = SpaceMemory::new(&sp, &mut fx.mem);
        let mut f = build_user_frame(0x40_0000, 0x7FFF_0000, 0).unwrap();
        f.rax = SYS_EP_CREATE;
        let r = handle_frame(&mut f, &mut Host::default(), &mut mem, &ipc, |_| Some(A));
        assert_eq!(r, FrameDisposition::Return(ReturnPath::Sysret));
        assert!(f.rax as i64 > 0, "an endpoint handle");
        let mut f2 = build_user_frame(0x40_0000, 0x7FFF_0000, 0).unwrap();
        f2.rax = SYS_NF_CREATE;
        f2.rflags |= crate::usermode::RFLAGS_TF;
        let r = handle_frame(&mut f2, &mut Host::default(), &mut mem, &ipc, |_| Some(A));
        assert_eq!(r, FrameDisposition::Return(ReturnPath::Iret));
        // a corrupt frame is never returned to, and nothing is executed for it
        let mut bad = build_user_frame(0x40_0000, 0x7FFF_0000, 0).unwrap();
        bad.rax = SYS_EP_CREATE;
        bad.cs = 0x08;
        let before = d.with(|st| st.domain.live_objects()).unwrap();
        assert!(matches!(
            handle_frame(&mut bad, &mut Host::default(), &mut mem, &ipc, |_| Some(A)),
            FrameDisposition::Kill(_)
        ));
        assert_eq!(d.with(|st| st.domain.live_objects()), Ok(before));
        // D5's exit still exits
        let mut fe = build_user_frame(0x40_0000, 0x7FFF_0000, 0).unwrap();
        fe.rax = Sysno::Exit as u64;
        fe.rdi = 7;
        let mut h = Host::default();
        assert_eq!(
            handle_frame(&mut fe, &mut h, &mut mem, &ipc, |_| Some(A)),
            FrameDisposition::Exit(7)
        );
    }
}

/// End-to-end tests with real context switches: every blocking system call below genuinely
/// parks its thread (`dispatch::reschedule`, the real stack switch) and another thread runs
/// before it resumes. They run against the real `Scheduler`, the real `KernelDomain`, `dispatch`
/// and `resume`; only the user-memory object and the (host) kernel stacks are test doubles.
#[cfg(all(test, target_arch = "x86_64"))]
mod real_switch {
    extern crate std;
    use super::super::cap_rights::Rights;
    use super::super::initialization::shutdown;
    use super::super::ipc_abi::*;
    use super::super::ipc_message::{Delivery, UserMessage, WIRE_OUT};
    use super::super::kernel_domain::{IpcState, KernelDomain};
    use super::super::process_lifecycle::cleanup_process;
    use super::super::scheduler_bridge::testkit::real::*;
    use super::super::thread_lifecycle::{cleanup_thread, interrupt_thread};
    use super::*;
    use std::format;
    use std::string::String;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::vec::Vec;

    static DOM: KernelDomain<8, 8, 32, 2> = KernelDomain::new();
    const A: u64 = 1;
    const B: u64 = 2;
    const BASE: u64 = 0x1_0000;
    const OUT: u64 = BASE + 0x100;
    static EP_A: AtomicU64 = AtomicU64::new(0);
    static EP_B: AtomicU64 = AtomicU64::new(0);
    static GOT: Mutex<Vec<(u64, i64, u64)>> = Mutex::new(Vec::new());
    static BLOCKED: Mutex<Vec<(u64, Handle)>> = Mutex::new(Vec::new());

    struct FlatMem(Vec<u8>);
    impl FlatMem {
        fn new() -> Self {
            FlatMem(std::vec![0xEE; 0x400])
        }
        fn delivery(&self) -> Delivery {
            let o = (OUT - BASE) as usize;
            Delivery::decode(self.0[o..o + WIRE_OUT].try_into().unwrap()).unwrap()
        }
        fn untouched(&self) -> bool {
            self.0[0x100..0x100 + WIRE_OUT].iter().all(|&b| b == 0xEE)
        }
    }
    impl UserMemory for FlatMem {
        fn check(&self, s: u64, l: u64, _w: bool) -> Result<(), SyscallError> {
            if s >= BASE && s.checked_add(l).is_some_and(|e| e <= BASE + 0x400) {
                Ok(())
            } else {
                Err(SyscallError::Fault)
            }
        }
        fn copy_from_user(&self, s: u64, d: &mut [u8]) -> Result<(), SyscallError> {
            self.check(s, d.len() as u64, false)?;
            let o = (s - BASE) as usize;
            d.copy_from_slice(&self.0[o..o + d.len()]);
            Ok(())
        }
        fn copy_to_user(&mut self, d: u64, s: &[u8]) -> Result<(), SyscallError> {
            self.check(d, s.len() as u64, true)?;
            let o = (d - BASE) as usize;
            self.0[o..o + s.len()].copy_from_slice(s);
            Ok(())
        }
    }

    struct H;
    impl SyscallHost for H {
        fn exit(&mut self, _: i32) {}
        fn yield_now(&mut self) -> Result<(), SyscallError> {
            Ok(())
        }
        fn sleep_ticks(&mut self, _: u64) -> Result<(), SyscallError> {
            Ok(())
        }
        fn getpid(&mut self) -> u64 {
            0
        }
        fn debug_write(&mut self, d: &[u8]) -> Result<usize, SyscallError> {
            Ok(d.len())
        }
    }

    fn sys(owner: u64, mem: &mut FlatMem, nr: u64, a0: u64, a1: u64, a2: u64) -> i64 {
        let p = port();
        let ipc = IpcKernel::new(&DOM, &p);
        let args = SyscallArgs {
            nr,
            a: [a0, a1, a2, 0, 0, 0],
        };
        match dispatch(&mut H, mem, &args, &ipc, |_| Some(owner)) {
            SyscallOutcome::Return(v) => v,
            SyscallOutcome::Exit(_) => panic!("unexpected exit"),
        }
    }

    fn put(mem: &mut FlatMem, m: &UserMessage) {
        mem.0[..m.encode().len()].copy_from_slice(&m.encode());
    }

    /// Fresh kernel + fresh IPC domain with one endpoint: A owns it, B may receive from it.
    fn fresh() -> std::sync::MutexGuard<'static, ()> {
        let g = setup();
        let _ = DOM.take_down(|_| ());
        DOM.install(IpcState::new()).unwrap();
        DOM.with(|st| {
            let ep = st.domain.create_endpoint(A).unwrap();
            let epb = st.domain.grant(A, ep, B, Rights::RECV).unwrap();
            EP_A.store(ep, Ordering::SeqCst);
            EP_B.store(epb, Ordering::SeqCst);
        })
        .unwrap();
        GOT.lock().unwrap().clear();
        BLOCKED.lock().unwrap().clear();
        g
    }

    fn ep_for(owner: u64) -> u64 {
        if owner == A {
            EP_A.load(Ordering::SeqCst)
        } else {
            EP_B.load(Ordering::SeqCst)
        }
    }

    fn note_blocking(owner: u64) {
        let p = port();
        let me = current_thread(&p).unwrap();
        BLOCKED.lock().unwrap().push((owner, me));
    }

    /// Receives once (blocking), records `(arg, rc, label)`.
    extern "C" fn receiver(arg: u64) -> i32 {
        let owner = if arg >= 100 { B } else { arg };
        let mut m = FlatMem::new();
        note_blocking(arg);
        log(format!("R{arg}:block"));
        let rc = sys(owner, &mut m, SYS_RECV, ep_for(owner), OUT, 0);
        let d = if rc == 0 {
            m.delivery().label
        } else {
            u64::MAX
        };
        GOT.lock().unwrap().push((arg, rc, d));
        log(format!("R{arg}:rc={rc} label={}", d as i64));
        0
    }

    extern "C" fn sender_one(_: u64) -> i32 {
        let mut m = FlatMem::new();
        put(&mut m, &UserMessage::new(7, &[1, 2], None).unwrap());
        log("S:send");
        assert_eq!(sys(A, &mut m, SYS_SEND, ep_for(A), BASE, 0), 0);
        log("S:sent");
        0
    }

    #[test]
    fn a_blocked_receive_really_yields_and_is_completed_by_a_direct_handoff() {
        let _g = fresh();
        spawn(receiver, B, 4);
        spawn(sender_one, 0, 4);
        start();
        assert_eq!(
            logged(),
            ["R2:block", "S:send", "S:sent", "R2:rc=0 label=7"],
            "R blocked, S ran in between, R resumed after the send"
        );
        assert_eq!(*GOT.lock().unwrap(), [(B, 0, 7)]);
        DOM.with(|st| {
            assert_eq!(st.domain.stats().handoffs, 1, "direct hand-off, not queued");
            assert!(st.pending.is_empty());
            assert_eq!(st.domain.waiting_threads(), 0);
            assert_eq!(st.domain.queued(A, ep_for(A)), Ok(0));
        })
        .unwrap();
    }

    extern "C" fn sender_three(_: u64) -> i32 {
        let mut m = FlatMem::new();
        for label in 1..=3u64 {
            put(&mut m, &UserMessage::new(label, &[], None).unwrap());
            assert_eq!(sys(A, &mut m, SYS_SEND, ep_for(A), BASE, 0), 0);
        }
        0
    }

    #[test]
    fn waiters_are_served_by_priority_then_fifo() {
        let _g = fresh();
        // all three receive through owner B's capability; arg 100+i identifies the thread.
        // They block in the order their priorities let them run: 103 (p6), 101 (p4), 102 (p4).
        spawn(receiver, 101, 4);
        spawn(receiver, 102, 4);
        spawn(receiver, 103, 6);
        spawn(sender_three, 0, 2);
        start();
        let got = GOT.lock().unwrap().clone();
        let label_of = |arg| got.iter().find(|g| g.0 == arg).map(|g| g.2);
        assert_eq!(label_of(103), Some(1), "highest priority waiter first");
        assert_eq!(label_of(101), Some(2), "then FIFO among equals");
        assert_eq!(label_of(102), Some(3));
        assert!(got.iter().all(|g| g.1 == 0));
        let order: Vec<String> = logged()
            .into_iter()
            .filter(|l| l.contains(":block"))
            .collect();
        assert_eq!(order, ["R103:block", "R101:block", "R102:block"]);
    }

    extern "C" fn blocking_sender(_: u64) -> i32 {
        let mut m = FlatMem::new();
        for label in 1..=3u64 {
            put(&mut m, &UserMessage::new(label, &[], None).unwrap());
            log(format!("S:send{label}"));
            let rc = sys(A, &mut m, SYS_SEND, ep_for(A), BASE, 0);
            log(format!("S:send{label}={rc}"));
        }
        0
    }

    extern "C" fn drain(_: u64) -> i32 {
        let mut m = FlatMem::new();
        for _ in 0..3 {
            let rc = sys(B, &mut m, SYS_RECV, ep_for(B), OUT, 0);
            GOT.lock().unwrap().push((B, rc, m.delivery().label));
        }
        0
    }

    #[test]
    fn a_sender_blocked_on_a_full_queue_resumes_and_queue_order_is_send_order() {
        let _g = fresh();
        spawn(blocking_sender, 0, 4); // queue holds 2: the third send blocks
        spawn(drain, 0, 4);
        start();
        let log = logged();
        let pos = |s: &str| {
            log.iter()
                .position(|l| l == s)
                .unwrap_or_else(|| panic!("{s}"))
        };
        assert!(pos("S:send3") < pos("S:send3=0"));
        assert_eq!(
            *GOT.lock().unwrap(),
            [(B, 0, 1), (B, 0, 2), (B, 0, 3)],
            "send order is receive order, including the message of the blocked sender"
        );
        DOM.with(|st| {
            assert!(st.pending.is_empty());
            assert_eq!(st.domain.waiting_threads(), 0);
        })
        .unwrap();
    }

    extern "C" fn interrupter(_: u64) -> i32 {
        let p = port();
        let ipc = IpcKernel::new(&DOM, &p);
        let (_, r) = BLOCKED.lock().unwrap()[0];
        assert!(interrupt_thread(&ipc, r).is_ok());
        assert!(interrupt_thread(&ipc, r).is_err(), "no duplicate wake-up");
        // a message sent afterwards is queued, not handed to the interrupted thread
        let mut m = FlatMem::new();
        put(&mut m, &UserMessage::new(5, &[], None).unwrap());
        assert_eq!(sys(A, &mut m, SYS_SEND, ep_for(A), BASE, 0), 0);
        0
    }

    #[test]
    fn an_interrupted_receive_returns_eintr_leaves_the_buffer_and_loses_no_message() {
        let _g = fresh();
        spawn(receiver, B, 4);
        spawn(interrupter, 0, 4);
        start();
        assert_eq!(*GOT.lock().unwrap(), [(B, EINTR, u64::MAX)]);
        DOM.with(|st| {
            assert_eq!(
                st.domain.queued(A, ep_for(A)),
                Ok(1),
                "message waits for a receiver"
            );
            assert_eq!(st.domain.stats().handoffs, 0);
            assert!(st.pending.is_empty());
            assert_eq!(st.domain.waiting_threads(), 0);
        })
        .unwrap();
    }

    extern "C" fn killer(_: u64) -> i32 {
        let p = port();
        let ipc = IpcKernel::new(&DOM, &p);
        let (_, r) = BLOCKED.lock().unwrap()[0];
        // what IpcCleanupEnv::kill_thread + process_lifecycle do on process exit
        p.with_sched(|s| s.kill(r, -9)).unwrap();
        let t = cleanup_thread(&ipc, r).unwrap();
        assert!(t.pending_cancelled && t.wait_removed);
        let c = cleanup_process(&ipc, B).unwrap();
        assert_eq!(c.caps_closed, 1);
        log("K:killed");
        // the dead process's handle is stale for good
        let mut m = FlatMem::new();
        put(&mut m, &UserMessage::new(1, &[], None).unwrap());
        assert_eq!(
            sys(B, &mut m, SYS_RECV, ep_for(B), OUT, FLAG_NONBLOCK),
            EBADF
        );
        0
    }

    #[test]
    fn a_thread_killed_while_blocked_never_resumes_and_leaves_no_ipc_state() {
        let _g = fresh();
        spawn(receiver, B, 4);
        spawn(killer, 0, 4);
        start();
        assert_eq!(
            logged(),
            ["R2:block", "K:killed"],
            "the dead thread did not resume"
        );
        assert!(GOT.lock().unwrap().is_empty());
        DOM.with(|st| {
            assert!(st.pending.is_empty());
            assert_eq!(st.domain.waiting_threads(), 0);
            assert_eq!(st.domain.caps().owner_count(B), 0);
            assert_eq!(
                st.domain.live_objects(),
                1,
                "owner A still holds the endpoint"
            );
        })
        .unwrap();
    }

    extern "C" fn downer(_: u64) -> i32 {
        let p = port();
        let ipc = IpcKernel::new(&DOM, &p);
        let r = shutdown(&ipc).unwrap();
        assert_eq!((r.pending_cancelled, r.caps_closed), (1, 2));
        let mut m = FlatMem::new();
        assert_eq!(
            sys(A, &mut m, SYS_EP_CREATE, 0, 0, 0),
            -38,
            "ENOSYS after shutdown"
        );
        0
    }

    #[test]
    fn shutdown_while_blocked_completes_the_waiter_with_eintr() {
        let _g = fresh();
        spawn(receiver, B, 4);
        spawn(downer, 0, 4);
        start();
        assert_eq!(*GOT.lock().unwrap(), [(B, EINTR, u64::MAX)]);
        assert!(!DOM.is_active());
    }

    extern "C" fn cap_sender(_: u64) -> i32 {
        let mut m = FlatMem::new();
        let nf = sys(A, &mut m, SYS_NF_CREATE, 0, 0, 0);
        assert!(nf > 0);
        let narrow = sys(
            A,
            &mut m,
            SYS_DERIVE,
            nf as u64,
            u64::from(Rights::SIGNAL.union(Rights::GRANT).bits()),
            0,
        );
        put(
            &mut m,
            &UserMessage::new(3, &[], Some(narrow as u64)).unwrap(),
        );
        assert_eq!(sys(A, &mut m, SYS_SEND, ep_for(A), BASE, 0), 0);
        0
    }

    extern "C" fn cap_receiver(_: u64) -> i32 {
        let mut m = FlatMem::new();
        assert_eq!(sys(B, &mut m, SYS_RECV, ep_for(B), OUT, 0), 0);
        let d = m.delivery();
        let cap = d.cap.expect("capability delivered after the wake-up");
        assert!(!d.cap_lost);
        // usable by B for SIGNAL only: attenuated by the sender's derive
        assert_eq!(sys(B, &mut m, SYS_SIGNAL, cap, 1, 0), 0);
        assert_eq!(sys(B, &mut m, SYS_WAIT, cap, OUT, FLAG_NONBLOCK), EPERM);
        assert_eq!(sys(B, &mut m, SYS_REVOKE, cap, 0, 0), EPERM);
        // not valid in A's space
        assert_eq!(sys(A, &mut m, SYS_SIGNAL, cap, 1, 0), EBADF);
        log("R:cap ok");
        0
    }

    #[test]
    fn a_capability_travels_through_a_blocked_receive_attenuated_and_owner_bound() {
        let _g = fresh();
        spawn(cap_receiver, 0, 4);
        spawn(cap_sender, 0, 4);
        start();
        assert_eq!(logged(), ["R:cap ok"]);
        DOM.with(|st| assert_eq!(st.domain.caps().limbo_count(), 0))
            .unwrap();
    }
}
