//! User-space ABI for capabilities and IPC (Mission B3). SPDX-License-Identifier: Apache-2.0
//!
//! Syscalls `0x40..=0x48`, deliberately outside the D5 numbers (`Sysno` 0..=4) so the two sets
//! cannot collide; [`owns`] tells a dispatcher which set a number belongs to. Arguments follow
//! the D5 convention (`SyscallArgs`: `rdi, rsi, rdx, r10, r8, r9`), results are returned in RAX
//! with negative values as errors (see [`errno`]).
//!
//! | nr | name | a0 | a1 | a2 | returns |
//! |---|---|---|---|---|---|
//! | 0x40 | `EP_CREATE` | | | | endpoint handle |
//! | 0x41 | `NF_CREATE` | | | | notification handle |
//! | 0x42 | `SEND` | endpoint | message ptr (56 B) | flags | 0 |
//! | 0x43 | `RECV` | endpoint | out ptr (64 B) | flags | 0 |
//! | 0x44 | `SIGNAL` | notification | bits | | 0 |
//! | 0x45 | `WAIT` | notification | out ptr (8 B) | flags | 0 |
//! | 0x46 | `DERIVE` | handle | rights mask | | new handle |
//! | 0x47 | `CLOSE` | handle | | | 0 |
//! | 0x48 | `REVOKE` | handle | | | number of capabilities deleted |
//!
//! `flags` bit 0 is [`FLAG_NONBLOCK`]; any other bit is `EINVAL`. User pointers are validated
//! with D5's `user_range_ok` and `UserMemory::check` **before** the operation runs, so a bad
//! output pointer can never consume a message or signal word.
//!
//! # Blocking
//! [`handle`] does not sleep. When an operation blocks it returns [`Abi::Blocked`] with a
//! [`Pending`] token; the kernel glue runs the scheduler, calls `IpcDomain::resume`, and passes
//! the outcome to [`complete`], which writes the user output buffer and produces the RAX value.
//! That glue (and chaining this dispatcher after D5's `dispatch`) is integration work that this
//! mission does not activate; see `docs/IPC_CAPABILITIES.md`.
#![allow(dead_code)]

use crate::cap_rights::Rights;
use crate::ipc::{IpcDomain, IpcError, RecvOutcome, Resume, SendOutcome, WaitOutcome};
use crate::ipc_message::{UserMessage, WIRE_IN, WIRE_OUT};
use crate::sched::Scheduler;
use crate::usermode::{SyscallArgs, UserMemory, user_range_ok};

pub const SYS_EP_CREATE: u64 = 0x40;
pub const SYS_NF_CREATE: u64 = 0x41;
pub const SYS_SEND: u64 = 0x42;
pub const SYS_RECV: u64 = 0x43;
pub const SYS_SIGNAL: u64 = 0x44;
pub const SYS_WAIT: u64 = 0x45;
pub const SYS_DERIVE: u64 = 0x46;
pub const SYS_CLOSE: u64 = 0x47;
pub const SYS_REVOKE: u64 = 0x48;
pub const FIRST_SYSCALL: u64 = SYS_EP_CREATE;
pub const LAST_SYSCALL: u64 = SYS_REVOKE;

pub const FLAG_NONBLOCK: u64 = 1;

pub const EPERM: i64 = -1;
pub const ESRCH: i64 = -3;
pub const EINTR: i64 = -4;
pub const EIO: i64 = -5;
pub const EBADF: i64 = -9;
pub const EAGAIN: i64 = -11;
pub const ENOMEM: i64 = -12;
pub const EFAULT: i64 = -14;
pub const EINVAL: i64 = -22;
pub const EMFILE: i64 = -24;
pub const EPIPE: i64 = -32;
pub const ECANCELED: i64 = -125;

/// Whether `nr` belongs to this ABI.
pub const fn owns(nr: u64) -> bool {
    nr >= FIRST_SYSCALL && nr <= LAST_SYSCALL
}

/// The negative RAX value for an error.
pub const fn errno(e: &IpcError) -> i64 {
    match e {
        IpcError::BadHandle => EBADF,
        IpcError::Perm => EPERM,
        IpcError::NoRights | IpcError::BadOwner | IpcError::BadArg | IpcError::WrongKind => EINVAL,
        IpcError::NoSlots | IpcError::NoObjects => ENOMEM,
        IpcError::Quota => EMFILE,
        IpcError::WouldBlock => EAGAIN,
        IpcError::Closed => EPIPE,
        IpcError::Revoked => ECANCELED,
        IpcError::Interrupted => EINTR,
        IpcError::NoCurrent | IpcError::IsIdle => ESRCH,
        IpcError::Sched(_) | IpcError::Corrupt(_) => EIO,
    }
}

/// The kernel operations the syscalls need. [`Bound`] implements it over a real domain and
/// scheduler; tests and other glue may implement it differently.
pub trait IpcService {
    fn create_endpoint(&mut self) -> Result<u64, IpcError>;
    fn create_notification(&mut self) -> Result<u64, IpcError>;
    fn send(&mut self, ep: u64, m: &UserMessage, block: bool) -> Result<SendOutcome, IpcError>;
    fn recv(&mut self, ep: u64, block: bool) -> Result<RecvOutcome, IpcError>;
    fn signal(&mut self, nf: u64, bits: u64) -> Result<(), IpcError>;
    fn wait(&mut self, nf: u64, block: bool) -> Result<WaitOutcome, IpcError>;
    fn derive(&mut self, h: u64, mask: Rights) -> Result<u64, IpcError>;
    fn close(&mut self, h: u64) -> Result<(), IpcError>;
    fn revoke(&mut self, h: u64) -> Result<u64, IpcError>;
}

/// What the caller still has to write when a blocked operation completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pending {
    Send,
    Recv { out: u64 },
    Wait { out: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Abi {
    /// The syscall finished; this is the RAX value.
    Done(i64),
    /// The calling thread is Blocked; see the module docs.
    Blocked(Pending),
}

fn check_user(mem: &impl UserMemory, ptr: u64, len: usize, write: bool) -> Result<(), i64> {
    if !user_range_ok(ptr, len as u64) || ptr == 0 {
        return Err(EFAULT);
    }
    mem.check(ptr, len as u64, write).map_err(|e| e as i64)
}

fn flags(a: u64) -> Result<bool, i64> {
    if a & !FLAG_NONBLOCK != 0 {
        return Err(EINVAL);
    }
    Ok(a & FLAG_NONBLOCK == 0)
}

fn done(r: Result<u64, IpcError>) -> Abi {
    Abi::Done(match r {
        Ok(v) if v <= i64::MAX as u64 => v as i64,
        Ok(_) => EINVAL,
        Err(e) => errno(&e),
    })
}

fn fail(e: i64) -> Abi {
    Abi::Done(e)
}

/// Executes one IPC syscall. `args.nr` must satisfy [`owns`]; anything else is `ENOSYS`
/// (-38), so a dispatcher that forwards unknown numbers gets the standard answer.
pub fn handle<S: IpcService, M: UserMemory>(svc: &mut S, mem: &mut M, args: &SyscallArgs) -> Abi {
    let a = &args.a;
    match args.nr {
        SYS_EP_CREATE => done(svc.create_endpoint()),
        SYS_NF_CREATE => done(svc.create_notification()),
        SYS_SEND => {
            let block = match flags(a[2]) {
                Ok(b) => b,
                Err(e) => return fail(e),
            };
            if let Err(e) = check_user(mem, a[1], WIRE_IN, false) {
                return fail(e);
            }
            let mut raw = [0u8; WIRE_IN];
            if let Err(e) = mem.copy_from_user(a[1], &mut raw) {
                return fail(e as i64);
            }
            let Ok(m) = UserMessage::decode(&raw) else {
                return fail(EINVAL);
            };
            match svc.send(a[0], &m, block) {
                Ok(SendOutcome::Sent) => Abi::Done(0),
                Ok(SendOutcome::Blocked) => Abi::Blocked(Pending::Send),
                Err(e) => fail(errno(&e)),
            }
        }
        SYS_RECV => {
            let block = match flags(a[2]) {
                Ok(b) => b,
                Err(e) => return fail(e),
            };
            if let Err(e) = check_user(mem, a[1], WIRE_OUT, true) {
                return fail(e);
            }
            match svc.recv(a[0], block) {
                Ok(RecvOutcome::Received(d)) => match mem.copy_to_user(a[1], &d.encode()) {
                    Ok(()) => Abi::Done(0),
                    Err(e) => fail(e as i64),
                },
                Ok(RecvOutcome::Blocked) => Abi::Blocked(Pending::Recv { out: a[1] }),
                Err(e) => fail(errno(&e)),
            }
        }
        SYS_SIGNAL => done(svc.signal(a[0], a[1]).map(|()| 0)),
        SYS_WAIT => {
            let block = match flags(a[2]) {
                Ok(b) => b,
                Err(e) => return fail(e),
            };
            if let Err(e) = check_user(mem, a[1], 8, true) {
                return fail(e);
            }
            match svc.wait(a[0], block) {
                Ok(WaitOutcome::Signalled(w)) => match mem.copy_to_user(a[1], &w.to_le_bytes()) {
                    Ok(()) => Abi::Done(0),
                    Err(e) => fail(e as i64),
                },
                Ok(WaitOutcome::Blocked) => Abi::Blocked(Pending::Wait { out: a[1] }),
                Err(e) => fail(errno(&e)),
            }
        }
        SYS_DERIVE => {
            let Some(mask) = u16::try_from(a[1]).ok().and_then(Rights::from_bits) else {
                return fail(EINVAL);
            };
            done(svc.derive(a[0], mask))
        }
        SYS_CLOSE => done(svc.close(a[0]).map(|()| 0)),
        SYS_REVOKE => done(svc.revoke(a[0])),
        _ => fail(-38),
    }
}

/// Finishes a syscall that returned [`Abi::Blocked`], once the thread has run again and
/// `IpcDomain::resume` produced `r`. Writes the user output buffer and returns the RAX value.
pub fn complete<M: UserMemory>(mem: &mut M, p: Pending, r: Result<Resume, IpcError>) -> i64 {
    match (p, r) {
        (_, Err(e)) => errno(&e),
        (Pending::Send, Ok(Resume::Sent)) => 0,
        (Pending::Recv { out }, Ok(Resume::Received(d))) => {
            match mem.copy_to_user(out, &d.encode()) {
                Ok(()) => 0,
                Err(e) => e as i64,
            }
        }
        (Pending::Wait { out }, Ok(Resume::Signalled(w))) => {
            match mem.copy_to_user(out, &w.to_le_bytes()) {
                Ok(()) => 0,
                Err(e) => e as i64,
            }
        }
        // The wake-up result does not match the operation that blocked: refuse, never guess.
        _ => EIO,
    }
}

/// [`IpcService`] over a real domain, scheduler and calling process.
pub struct Bound<'a, const N: usize, const O: usize, const C: usize, const Q: usize> {
    pub dom: &'a mut IpcDomain<N, O, C, Q>,
    pub sched: &'a mut Scheduler<N>,
    /// The calling process id (the capability owner).
    pub owner: u64,
}

impl<const N: usize, const O: usize, const C: usize, const Q: usize> IpcService
    for Bound<'_, N, O, C, Q>
{
    fn create_endpoint(&mut self) -> Result<u64, IpcError> {
        self.dom.create_endpoint(self.owner)
    }
    fn create_notification(&mut self) -> Result<u64, IpcError> {
        self.dom.create_notification(self.owner)
    }
    fn send(&mut self, ep: u64, m: &UserMessage, block: bool) -> Result<SendOutcome, IpcError> {
        self.dom.send(self.sched, self.owner, ep, m, block)
    }
    fn recv(&mut self, ep: u64, block: bool) -> Result<RecvOutcome, IpcError> {
        self.dom.recv(self.sched, self.owner, ep, block)
    }
    fn signal(&mut self, nf: u64, bits: u64) -> Result<(), IpcError> {
        self.dom.signal(self.sched, self.owner, nf, bits)
    }
    fn wait(&mut self, nf: u64, block: bool) -> Result<WaitOutcome, IpcError> {
        self.dom.wait(self.sched, self.owner, nf, block)
    }
    fn derive(&mut self, h: u64, mask: Rights) -> Result<u64, IpcError> {
        self.dom.derive(self.owner, h, mask)
    }
    fn close(&mut self, h: u64) -> Result<(), IpcError> {
        self.dom.close(self.sched, self.owner, h)
    }
    fn revoke(&mut self, h: u64) -> Result<u64, IpcError> {
        self.dom.revoke(self.sched, self.owner, h).map(|n| n as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::IpcError as E;
    use crate::ipc_message::{Delivery, MAX_WORDS};
    use crate::thread::{Handle, Priority};
    use crate::usermode::SyscallError;
    extern crate std;
    use std::vec::Vec;

    type S = Scheduler<8>;
    type D = IpcDomain<8, 8, 32, 4>;

    const BASE: u64 = 0x10000;
    const SIZE: usize = 0x2000;
    const RO_END: u64 = BASE + 0x1000; // second page is read-only

    struct Mem {
        bytes: Vec<u8>,
    }

    impl Mem {
        fn new() -> Self {
            Self {
                bytes: std::vec![0; SIZE],
            }
        }
        fn put(&mut self, at: u64, b: &[u8]) {
            let o = (at - BASE) as usize;
            self.bytes[o..o + b.len()].copy_from_slice(b);
        }
        fn get(&self, at: u64, n: usize) -> Vec<u8> {
            let o = (at - BASE) as usize;
            self.bytes[o..o + n].to_vec()
        }
    }

    impl UserMemory for Mem {
        fn check(&self, start: u64, len: u64, write: bool) -> Result<(), SyscallError> {
            let end = start.checked_add(len).ok_or(SyscallError::Fault)?;
            if start < BASE || end > BASE + SIZE as u64 {
                return Err(SyscallError::Fault);
            }
            if write && end > RO_END {
                return Err(SyscallError::Fault); // only the first page is writable
            }
            Ok(())
        }
        fn copy_from_user(&self, src: u64, dst: &mut [u8]) -> Result<(), SyscallError> {
            self.check(src, dst.len() as u64, false)?;
            dst.copy_from_slice(&self.get(src, dst.len()));
            Ok(())
        }
        fn copy_to_user(&mut self, d: u64, src: &[u8]) -> Result<(), SyscallError> {
            self.check(d, src.len() as u64, true)?;
            self.put(d, src);
            Ok(())
        }
    }

    fn args(nr: u64, a: [u64; 3]) -> SyscallArgs {
        SyscallArgs {
            nr,
            a: [a[0], a[1], a[2], 0, 0, 0],
        }
    }

    fn run(s: &mut S) -> Handle {
        let sw = s.schedule().unwrap();
        if sw.is_switch()
            && let Some(prev) = sw.prev
        {
            s.switch_complete(prev).unwrap();
        }
        sw.next
    }

    fn run_until(s: &mut S, h: Handle) {
        for _ in 0..40 {
            if s.current() == Some(h) {
                return;
            }
            run(s);
        }
        panic!("thread never became current");
    }

    fn setup(n: usize) -> (S, D, Vec<Handle>, Mem) {
        let mut s = S::new();
        s.init_idle(0, 0).unwrap();
        let hs = (0..n)
            .map(|_| s.spawn(Priority::new(4).unwrap(), 0, 0).unwrap())
            .collect();
        (s, D::new(), hs, Mem::new())
    }

    /// One syscall by process `owner`.
    fn sys(d: &mut D, s: &mut S, m: &mut Mem, owner: u64, nr: u64, a: [u64; 3]) -> Abi {
        handle(
            &mut Bound {
                dom: d,
                sched: s,
                owner,
            },
            m,
            &args(nr, a),
        )
    }

    fn val(a: Abi) -> i64 {
        match a {
            Abi::Done(v) => v,
            Abi::Blocked(p) => panic!("unexpectedly blocked: {p:?}"),
        }
    }

    #[test]
    fn syscall_numbers_are_disjoint_from_d5_and_contiguous() {
        let all = [
            SYS_EP_CREATE,
            SYS_NF_CREATE,
            SYS_SEND,
            SYS_RECV,
            SYS_SIGNAL,
            SYS_WAIT,
            SYS_DERIVE,
            SYS_CLOSE,
            SYS_REVOKE,
        ];
        for (i, n) in all.iter().enumerate() {
            assert_eq!(*n, FIRST_SYSCALL + i as u64);
            assert!(owns(*n));
            assert!(
                crate::usermode::Sysno::from_u64(*n).is_none(),
                "collides with D5"
            );
        }
        assert!(!owns(0) && !owns(4) && !owns(0x3F) && !owns(0x49) && !owns(u64::MAX));
    }

    #[test]
    fn errno_values_are_negative_and_distinct_per_class() {
        let all = [
            E::BadHandle,
            E::Perm,
            E::NoRights,
            E::NoSlots,
            E::Quota,
            E::BadOwner,
            E::NoObjects,
            E::WouldBlock,
            E::BadArg,
            E::Closed,
            E::Revoked,
            E::Interrupted,
            E::WrongKind,
            E::NoCurrent,
            E::IsIdle,
            E::Corrupt("x"),
        ];
        for e in &all {
            assert!(errno(e) < 0, "{e:?}");
        }
        assert_eq!(errno(&E::BadHandle), EBADF);
        assert_eq!(errno(&E::Perm), SyscallError::Perm as i64, "agrees with D5");
        assert_eq!(
            errno(&E::WouldBlock),
            SyscallError::Again as i64,
            "agrees with D5"
        );
        let distinct = [
            EBADF, EPERM, EAGAIN, EPIPE, ECANCELED, EINTR, EMFILE, ENOMEM,
        ];
        for (i, a) in distinct.iter().enumerate() {
            for b in &distinct[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert_eq!(errno(&E::Sched(crate::thread::Error::NoCurrent)), EIO);
    }

    #[test]
    fn create_send_recv_round_trip_through_user_memory() {
        let (mut s, mut d, hs, mut m) = setup(1);
        run_until(&mut s, hs[0]);
        let ep = val(sys(&mut d, &mut s, &mut m, 1, SYS_EP_CREATE, [0; 3]));
        assert!(ep > 0);
        let msg = UserMessage::new(0xAB, &[1, 2, 3], None).unwrap();
        m.put(BASE, &msg.encode());
        assert_eq!(
            val(sys(
                &mut d,
                &mut s,
                &mut m,
                1,
                SYS_SEND,
                [ep as u64, BASE, 0]
            )),
            0
        );
        assert_eq!(
            val(sys(
                &mut d,
                &mut s,
                &mut m,
                1,
                SYS_RECV,
                [ep as u64, BASE + 0x100, FLAG_NONBLOCK]
            )),
            0
        );
        let raw: [u8; WIRE_OUT] = m.get(BASE + 0x100, WIRE_OUT).try_into().unwrap();
        let got = Delivery::decode(&raw).unwrap();
        assert_eq!(
            (got.label, got.data(), got.cap, got.cap_lost),
            (0xAB, &[1u64, 2, 3][..], None, false)
        );
        assert_eq!(
            val(sys(
                &mut d,
                &mut s,
                &mut m,
                1,
                SYS_RECV,
                [ep as u64, BASE + 0x100, FLAG_NONBLOCK]
            )),
            EAGAIN
        );
    }

    #[test]
    fn bad_user_pointers_fail_with_efault_and_consume_nothing() {
        let (mut s, mut d, hs, mut m) = setup(1);
        run_until(&mut s, hs[0]);
        let ep = val(sys(&mut d, &mut s, &mut m, 1, SYS_EP_CREATE, [0; 3])) as u64;
        m.put(BASE, &UserMessage::new(1, &[], None).unwrap().encode());
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 1, SYS_SEND, [ep, BASE, 0])),
            0
        );
        for bad in [
            0u64,
            8,
            u64::MAX - 10,
            0x7FFF_FFFF_F000,
            BASE - 1,
            BASE + SIZE as u64 - 10,
        ] {
            assert_eq!(
                val(sys(
                    &mut d,
                    &mut s,
                    &mut m,
                    1,
                    SYS_RECV,
                    [ep, bad, FLAG_NONBLOCK]
                )),
                EFAULT,
                "{bad:#x}"
            );
            assert_eq!(
                val(sys(
                    &mut d,
                    &mut s,
                    &mut m,
                    1,
                    SYS_SEND,
                    [ep, bad, FLAG_NONBLOCK]
                )),
                EFAULT,
                "{bad:#x}"
            );
            assert_eq!(
                val(sys(
                    &mut d,
                    &mut s,
                    &mut m,
                    1,
                    SYS_WAIT,
                    [ep, bad, FLAG_NONBLOCK]
                )),
                EFAULT,
                "{bad:#x}"
            );
        }
        // read-only memory is readable (send) but not writable (recv, wait)
        assert_eq!(
            val(sys(
                &mut d,
                &mut s,
                &mut m,
                1,
                SYS_RECV,
                [ep, RO_END, FLAG_NONBLOCK]
            )),
            EFAULT
        );
        assert_eq!(
            val(sys(
                &mut d,
                &mut s,
                &mut m,
                1,
                SYS_WAIT,
                [ep, RO_END, FLAG_NONBLOCK]
            )),
            EFAULT
        );
        assert_eq!(
            d.queued(1, ep),
            Ok(1),
            "the queued message survived every refused receive"
        );
    }

    #[test]
    fn malformed_arguments_are_einval() {
        let (mut s, mut d, hs, mut m) = setup(1);
        run_until(&mut s, hs[0]);
        let ep = val(sys(&mut d, &mut s, &mut m, 1, SYS_EP_CREATE, [0; 3])) as u64;
        let nf = val(sys(&mut d, &mut s, &mut m, 1, SYS_NF_CREATE, [0; 3])) as u64;
        let mut raw = UserMessage::new(1, &[], None).unwrap().encode();
        raw[8] = (MAX_WORDS + 1) as u8;
        m.put(BASE, &raw);
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 1, SYS_SEND, [ep, BASE, 0])),
            EINVAL,
            "len > 4"
        );
        let mut dirty = UserMessage::new(1, &[], None).unwrap().encode();
        dirty[40] = 1; // words[3] while len == 0
        m.put(BASE, &dirty);
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 1, SYS_SEND, [ep, BASE, 0])),
            EINVAL,
            "dirty padding"
        );
        m.put(BASE, &UserMessage::new(1, &[], None).unwrap().encode());
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 1, SYS_SEND, [ep, BASE, 2])),
            EINVAL,
            "unknown flag"
        );
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 1, SYS_RECV, [ep, BASE, 0x80])),
            EINVAL
        );
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 1, SYS_WAIT, [nf, BASE, 4])),
            EINVAL
        );
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 1, SYS_SIGNAL, [nf, 0, 0])),
            EINVAL,
            "zero signal"
        );
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 1, SYS_DERIVE, [ep, 1 << 20, 0])),
            EINVAL,
            "unknown rights"
        );
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 1, SYS_DERIVE, [ep, 0, 0])),
            EINVAL,
            "empty rights"
        );
        assert_eq!(
            val(sys(
                &mut d,
                &mut s,
                &mut m,
                1,
                SYS_DERIVE,
                [ep, u64::MAX, 0]
            )),
            EINVAL
        );
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 1, 0x49, [0; 3])),
            -38,
            "not ours"
        );
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 1, 4, [0; 3])),
            -38,
            "D5 numbers are not ours"
        );
    }

    #[test]
    fn handles_and_rights_are_enforced_at_the_syscall_boundary() {
        let (mut s, mut d, hs, mut m) = setup(1);
        run_until(&mut s, hs[0]);
        let ep = val(sys(&mut d, &mut s, &mut m, 1, SYS_EP_CREATE, [0; 3])) as u64;
        m.put(BASE, &UserMessage::new(1, &[], None).unwrap().encode());
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 2, SYS_SEND, [ep, BASE, 0])),
            EBADF,
            "other process"
        );
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 1, SYS_SEND, [0, BASE, 0])),
            EBADF
        );
        assert_eq!(
            val(sys(
                &mut d,
                &mut s,
                &mut m,
                1,
                SYS_SEND,
                [u64::MAX, BASE, 0]
            )),
            EBADF
        );
        let recv_only = val(sys(
            &mut d,
            &mut s,
            &mut m,
            1,
            SYS_DERIVE,
            [ep, Rights::RECV.bits() as u64, 0],
        )) as u64;
        assert_eq!(
            val(sys(
                &mut d,
                &mut s,
                &mut m,
                1,
                SYS_SEND,
                [recv_only, BASE, 0]
            )),
            EPERM
        );
        let nf = val(sys(&mut d, &mut s, &mut m, 1, SYS_NF_CREATE, [0; 3])) as u64;
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 1, SYS_SEND, [nf, BASE, 0])),
            EPERM,
            "wrong object kind"
        );
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 1, SYS_SIGNAL, [ep, 1, 0])),
            EPERM
        );
        assert_eq!(
            val(sys(
                &mut d,
                &mut s,
                &mut m,
                1,
                SYS_REVOKE,
                [recv_only, 0, 0]
            )),
            EPERM,
            "no REVOKE right"
        );
    }

    #[test]
    fn handles_never_look_like_errors() {
        let (mut s, mut d, hs, mut m) = setup(1);
        run_until(&mut s, hs[0]);
        // churn one slot: handles must stay positive as generations climb
        for _ in 0..200 {
            let h = val(sys(&mut d, &mut s, &mut m, 1, SYS_EP_CREATE, [0; 3]));
            assert!(h > 0);
            assert_eq!(
                val(sys(&mut d, &mut s, &mut m, 1, SYS_CLOSE, [h as u64, 0, 0])),
                0
            );
        }
    }

    #[test]
    fn notification_syscalls_signal_wait_and_write_the_word() {
        let (mut s, mut d, hs, mut m) = setup(2);
        run_until(&mut s, hs[0]);
        let nf = val(sys(&mut d, &mut s, &mut m, 1, SYS_NF_CREATE, [0; 3])) as u64;
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 1, SYS_SIGNAL, [nf, 0b101, 0])),
            0
        );
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 1, SYS_SIGNAL, [nf, 1 << 63, 0])),
            0
        );
        assert_eq!(
            val(sys(
                &mut d,
                &mut s,
                &mut m,
                1,
                SYS_WAIT,
                [nf, BASE, FLAG_NONBLOCK]
            )),
            0
        );
        assert_eq!(m.get(BASE, 8), (0b101u64 | 1 << 63).to_le_bytes());
        assert_eq!(
            val(sys(
                &mut d,
                &mut s,
                &mut m,
                1,
                SYS_WAIT,
                [nf, BASE, FLAG_NONBLOCK]
            )),
            EAGAIN
        );
    }

    #[test]
    fn blocking_recv_completes_through_complete_and_writes_the_buffer() {
        let (mut s, mut d, hs, mut m) = setup(2);
        run_until(&mut s, hs[0]);
        let ep = val(sys(&mut d, &mut s, &mut m, 1, SYS_EP_CREATE, [0; 3])) as u64;
        let out = BASE + 0x200;
        let pending = match sys(&mut d, &mut s, &mut m, 1, SYS_RECV, [ep, out, 0]) {
            Abi::Blocked(p) => p,
            other => panic!("{other:?}"),
        };
        assert_eq!(pending, Pending::Recv { out });
        run_until(&mut s, hs[1]);
        m.put(BASE, &UserMessage::new(77, &[9], None).unwrap().encode());
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 1, SYS_SEND, [ep, BASE, 0])),
            0
        );
        run_until(&mut s, hs[0]);
        let r = d.resume(hs[0]);
        assert_eq!(complete(&mut m, pending, r), 0);
        let raw: [u8; WIRE_OUT] = m.get(out, WIRE_OUT).try_into().unwrap();
        let got = Delivery::decode(&raw).unwrap();
        assert_eq!((got.label, got.data()), (77, &[9u64][..]));
    }

    #[test]
    fn blocking_send_and_wait_complete_and_errors_map_to_errno() {
        let (mut s, mut d, hs, mut m) = setup(2);
        run_until(&mut s, hs[0]);
        let ep = val(sys(&mut d, &mut s, &mut m, 1, SYS_EP_CREATE, [0; 3])) as u64;
        m.put(BASE, &UserMessage::new(1, &[], None).unwrap().encode());
        for _ in 0..4 {
            assert_eq!(
                val(sys(&mut d, &mut s, &mut m, 1, SYS_SEND, [ep, BASE, 0])),
                0
            );
        }
        let pending = match sys(&mut d, &mut s, &mut m, 1, SYS_SEND, [ep, BASE, 0]) {
            Abi::Blocked(p) => p,
            other => panic!("{other:?}"),
        };
        assert_eq!(pending, Pending::Send);
        run_until(&mut s, hs[1]);
        // the receiving side frees a slot
        assert_eq!(
            val(sys(
                &mut d,
                &mut s,
                &mut m,
                1,
                SYS_RECV,
                [ep, BASE + 0x100, FLAG_NONBLOCK]
            )),
            0
        );
        run_until(&mut s, hs[0]);
        let r = d.resume(hs[0]);
        assert_eq!(complete(&mut m, pending, r), 0);
        assert_eq!(d.queued(1, ep), Ok(4));

        // a blocked WAIT whose notification is closed completes with EPIPE
        let nf = val(sys(&mut d, &mut s, &mut m, 1, SYS_NF_CREATE, [0; 3])) as u64;
        let pending = match sys(&mut d, &mut s, &mut m, 1, SYS_WAIT, [nf, BASE + 0x300, 0]) {
            Abi::Blocked(p) => p,
            other => panic!("{other:?}"),
        };
        run_until(&mut s, hs[1]);
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 1, SYS_CLOSE, [nf, 0, 0])),
            0
        );
        run_until(&mut s, hs[0]);
        let r = d.resume(hs[0]);
        assert_eq!(complete(&mut m, pending, r), EPIPE);
        // mismatched completion is refused rather than guessed
        assert_eq!(
            complete(&mut m, Pending::Send, Ok(Resume::Signalled(1))),
            EIO
        );
        assert_eq!(
            complete(&mut m, Pending::Wait { out: BASE }, Ok(Resume::Sent)),
            EIO
        );
        assert_eq!(
            complete(&mut m, Pending::Recv { out: BASE }, Err(E::Interrupted)),
            EINTR
        );
        // a bad out pointer at completion time is reported, not ignored
        assert_eq!(
            complete(
                &mut m,
                Pending::Wait { out: 0x10 },
                Ok(Resume::Signalled(1))
            ),
            EFAULT
        );
    }

    #[test]
    fn capability_transfer_works_end_to_end_through_the_abi() {
        let (mut s, mut d, hs, mut m) = setup(1);
        run_until(&mut s, hs[0]);
        let ep = val(sys(&mut d, &mut s, &mut m, 1, SYS_EP_CREATE, [0; 3])) as u64;
        let payload = val(sys(&mut d, &mut s, &mut m, 1, SYS_EP_CREATE, [0; 3])) as u64;
        // process 2 receives on ep through a kernel-granted capability
        let ep2 = d.grant(1, ep, 2, Rights::RECV).unwrap();
        m.put(
            BASE,
            &UserMessage::new(5, &[], Some(payload)).unwrap().encode(),
        );
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 1, SYS_SEND, [ep, BASE, 0])),
            0
        );
        assert_eq!(
            val(sys(
                &mut d,
                &mut s,
                &mut m,
                2,
                SYS_RECV,
                [ep2, BASE + 0x100, FLAG_NONBLOCK]
            )),
            0
        );
        let raw: [u8; WIRE_OUT] = m.get(BASE + 0x100, WIRE_OUT).try_into().unwrap();
        let got = Delivery::decode(&raw).unwrap();
        let theirs = got.cap.expect("capability delivered");
        // process 2 can use it; process 1 cannot use process 2's handle
        m.put(BASE, &UserMessage::new(6, &[], None).unwrap().encode());
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 2, SYS_SEND, [theirs, BASE, 0])),
            0
        );
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 1, SYS_SEND, [theirs, BASE, 0])),
            EBADF
        );
        // revoking the original removes process 2's copy
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 1, SYS_REVOKE, [payload, 0, 0])),
            1
        );
        assert_eq!(
            val(sys(&mut d, &mut s, &mut m, 2, SYS_SEND, [theirs, BASE, 0])),
            EBADF
        );
    }
}
