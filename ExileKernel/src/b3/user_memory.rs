//! `UserMemory` over a process address space (Mission B4). SPDX-License-Identifier: Apache-2.0
//!
//! [`SpaceMemory`] is the adapter `ipc_abi` uses to read send buffers and write receive/wait
//! results. It does not reimplement anything: every check and copy is delegated to the D5
//! [`AddressSpace`], which walks the page tables of *that process* and verifies, page by page,
//! that the page is present, user-accessible and (for writes) writable. This adapter only adds
//! the policy that belongs in front of it:
//!
//! * `usermode::user_range_ok` first: the range must lie wholly inside the user half
//!   (`USER_START..=USER_END`) without wrapping, so kernel addresses, the null page and
//!   overflowing `ptr + len` are rejected before any page-table access;
//! * a read never needs write permission and a write always does;
//! * every failure (unmapped, wrong permission, out of range, physical access error) is
//!   reported as `SyscallError::Fault`. User space learns nothing about *why*.
//!
//! The `check` / copy split is not atomic against another CPU changing the mappings, but EUK is
//! single-CPU with interrupts off in system calls, and the copy re-validates the whole range
//! before it writes the first byte. A physical-memory error in the middle of a copy can leave a
//! prefix written (the physical layer reports it as a fault); callers treat any `Fault` from
//! `copy_to_user` as "not delivered" and `ipc_abi::write_delivery` then closes the capability
//! that the receive handed over.
#![allow(dead_code)]

use crate::address_space::{Access, AddressSpace, PhysMem};
use crate::process::{Error as ProcError, ProcHandle, ProcessTable};
use crate::usermode::{SyscallError, UserMemory, user_range_ok};

pub struct SpaceMemory<'a, P: PhysMem + ?Sized> {
    space: &'a AddressSpace,
    phys: &'a mut P,
}

impl<'a, P: PhysMem + ?Sized> SpaceMemory<'a, P> {
    pub fn new(space: &'a AddressSpace, phys: &'a mut P) -> Self {
        Self { space, phys }
    }

    /// The memory of process `h`. Fails if the process is gone or already released its space.
    pub fn for_process<const PN: usize>(
        table: &'a ProcessTable<PN>,
        h: ProcHandle,
        phys: &'a mut P,
    ) -> Result<Self, ProcError> {
        let space = table.space(h)?.ok_or(ProcError::BadState)?;
        Ok(Self::new(space, phys))
    }

    fn range(start: u64, len: u64) -> Result<(), SyscallError> {
        if user_range_ok(start, len) {
            Ok(())
        } else {
            Err(SyscallError::Fault)
        }
    }
}

impl<P: PhysMem + ?Sized> UserMemory for SpaceMemory<'_, P> {
    fn check(&self, start: u64, len: u64, write: bool) -> Result<(), SyscallError> {
        Self::range(start, len)?;
        let access = if write { Access::Write } else { Access::Read };
        self.space
            .validate_user_range(&*self.phys, start, len, access)
            .map_err(|_| SyscallError::Fault)
    }

    fn copy_from_user(&self, src: u64, dst: &mut [u8]) -> Result<(), SyscallError> {
        Self::range(src, dst.len() as u64)?;
        self.space
            .copy_from_user(&*self.phys, src, dst)
            .map_err(|_| SyscallError::Fault)
    }

    fn copy_to_user(&mut self, dst: u64, src: &[u8]) -> Result<(), SyscallError> {
        Self::range(dst, src.len() as u64)?;
        self.space
            .copy_to_user(&mut *self.phys, dst, src)
            .map_err(|_| SyscallError::Fault)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::ipc::IpcDomain;
    use super::super::ipc_abi::{self, Abi, Bound};
    use super::super::ipc_message::UserMessage;
    use super::*;
    use crate::address_space::fakes::{Fx, TestMem};
    use crate::address_space::{MemError, PAGE, Perms};
    use crate::sched::Scheduler;
    use crate::thread::Priority;
    use crate::usermode::SyscallArgs;

    pub(crate) const RW: u64 = 0x40_0000;
    pub(crate) const RO: u64 = 0x50_0000;

    /// A `PhysMem` that fails writes on demand and counts the bytes it was asked to write.
    pub(crate) struct Flaky {
        pub inner: TestMem,
        pub fail_writes: bool,
        pub written: usize,
    }

    impl PhysMem for Flaky {
        fn read_u64(&self, pa: u64) -> Result<u64, MemError> {
            self.inner.read_u64(pa)
        }
        fn write_u64(&mut self, pa: u64, v: u64) -> Result<(), MemError> {
            self.inner.write_u64(pa, v)
        }
        fn read_bytes(&self, pa: u64, out: &mut [u8]) -> Result<(), MemError> {
            self.inner.read_bytes(pa, out)
        }
        fn write_bytes(&mut self, pa: u64, d: &[u8]) -> Result<(), MemError> {
            if self.fail_writes {
                return Err(MemError);
            }
            self.written += d.len();
            self.inner.write_bytes(pa, d)
        }
        fn zero_page(&mut self, pa: u64) -> Result<(), MemError> {
            self.inner.zero_page(pa)
        }
    }

    /// Two RW pages at `RW`, two RO pages at `RO`, nothing around them.
    pub(crate) fn fixture() -> (Fx, AddressSpace) {
        let mut fx = Fx::new(64);
        let mut sp = fx.space();
        sp.map_anon(&mut fx.mem, &mut fx.frames, RW, 2 * PAGE, Perms::rw())
            .unwrap();
        sp.map_anon(&mut fx.mem, &mut fx.frames, RO, 2 * PAGE, Perms::ro())
            .unwrap();
        (fx, sp)
    }

    #[test]
    fn reads_and_writes_round_trip_across_a_page_boundary() {
        let (mut fx, sp) = fixture();
        let mut m = SpaceMemory::new(&sp, &mut fx.mem);
        let data: [u8; 64] = core::array::from_fn(|i| i as u8);
        let at = RW + PAGE - 10; // straddles the two pages
        assert_eq!(m.check(at, 64, true), Ok(()));
        assert_eq!(m.copy_to_user(at, &data), Ok(()));
        let mut back = [0u8; 64];
        assert_eq!(m.copy_from_user(at, &mut back), Ok(()));
        assert_eq!(back, data);
    }

    #[test]
    fn kernel_null_wrapping_and_out_of_range_addresses_are_faults() {
        let (mut fx, sp) = fixture();
        let mut m = SpaceMemory::new(&sp, &mut fx.mem);
        let mut b = [0u8; 8];
        for bad in [
            0u64,
            1,
            PAGE - 8,
            0xFFFF_8000_0000_0000,
            0xFFFF_FFFF_FFFF_FFF8,
            crate::usermode::USER_END - 4,
            u64::MAX,
        ] {
            assert_eq!(m.check(bad, 8, false), Err(SyscallError::Fault), "{bad:#x}");
            assert_eq!(m.check(bad, 8, true), Err(SyscallError::Fault));
            assert_eq!(m.copy_from_user(bad, &mut b), Err(SyscallError::Fault));
            assert_eq!(m.copy_to_user(bad, &b), Err(SyscallError::Fault));
        }
        // ptr + len wraps around the address space
        assert_eq!(m.check(RW, u64::MAX, false), Err(SyscallError::Fault));
        assert_eq!(
            m.check(RW, u64::MAX - RW + 1, false),
            Err(SyscallError::Fault)
        );
    }

    #[test]
    fn unmapped_and_partially_mapped_ranges_fault_without_writing() {
        let (mut fx, sp) = fixture();
        let mut m = SpaceMemory::new(&sp, &mut fx.mem);
        assert_eq!(m.check(0x60_0000, 8, false), Err(SyscallError::Fault));
        // range that starts mapped but runs off the end of the region
        let at = RW + 2 * PAGE - 4;
        assert_eq!(m.check(at, 8, true), Err(SyscallError::Fault));
        let before = {
            let mut b = [0u8; 4];
            m.copy_from_user(at, &mut b).unwrap();
            b
        };
        assert_eq!(m.copy_to_user(at, &[0x55; 8]), Err(SyscallError::Fault));
        let mut after = [0u8; 4];
        m.copy_from_user(at, &mut after).unwrap();
        assert_eq!(before, after, "no partial write into the mapped part");
    }

    #[test]
    fn permissions_are_enforced_per_direction() {
        let (mut fx, sp) = fixture();
        let mut m = SpaceMemory::new(&sp, &mut fx.mem);
        assert_eq!(
            m.check(RO, 16, false),
            Ok(()),
            "reading read-only memory is fine"
        );
        assert_eq!(m.check(RO, 16, true), Err(SyscallError::Fault));
        assert_eq!(m.copy_to_user(RO, &[1; 16]), Err(SyscallError::Fault));
        let mut b = [9u8; 16];
        assert_eq!(m.copy_from_user(RO, &mut b), Ok(()));
        assert_eq!(b, [0; 16], "fresh anonymous pages are zero");
    }

    #[test]
    fn zero_length_ranges_are_valid_even_for_odd_addresses() {
        let (mut fx, sp) = fixture();
        let m = SpaceMemory::new(&sp, &mut fx.mem);
        assert_eq!(m.check(RW, 0, true), Ok(()));
        assert_eq!(m.copy_from_user(RW, &mut []), Ok(()));
    }

    #[test]
    fn a_physical_write_error_is_a_fault() {
        let (fx, sp) = fixture();
        let mut phys = Flaky {
            inner: fx.mem,
            fail_writes: true,
            written: 0,
        };
        let mut m = SpaceMemory::new(&sp, &mut phys);
        assert_eq!(
            m.check(RW, 8, true),
            Ok(()),
            "the check cannot see this error"
        );
        assert_eq!(m.copy_to_user(RW, &[1; 8]), Err(SyscallError::Fault));
    }

    #[test]
    fn for_process_refuses_a_process_without_an_address_space() {
        let table = ProcessTable::<4>::new();
        let mut fx = Fx::new(8);
        let bogus = {
            // a handle that was never issued: take it from a second table
            let mut t2 = ProcessTable::<4>::new();
            let sp = fx.space();
            t2.create(None, crate::process::Limits::DEFAULT, sp)
                .map_err(|(e, _)| e)
                .unwrap()
        };
        assert!(SpaceMemory::for_process(&table, bogus, &mut fx.mem).is_err());
    }

    /// A failed copy-out of a received message must not leave the delivered capability behind
    /// (the B3 fix, now exercised through the real address-space adapter).
    #[test]
    fn failed_copy_out_through_the_adapter_closes_the_delivered_capability() {
        let (fx, sp) = fixture();
        let mut phys = Flaky {
            inner: fx.mem,
            fail_writes: false,
            written: 0,
        };
        let mut s = Scheduler::<4>::new();
        s.init_idle(0, 0).unwrap();
        let t = s.spawn(Priority::new(4).unwrap(), 0, 0).unwrap();
        for _ in 0..4 {
            if s.current() == Some(t) {
                break;
            }
            let sw = s.schedule().unwrap();
            if sw.is_switch()
                && let Some(p) = sw.prev
            {
                s.switch_complete(p).unwrap();
            }
        }
        assert_eq!(s.current(), Some(t));
        let mut d = IpcDomain::<4, 4, 16, 2>::new();
        let ep = d.create_endpoint(1).unwrap();
        let nf = d.create_notification(1).unwrap();
        d.send(
            &mut s,
            1,
            ep,
            &UserMessage::new(1, &[], Some(nf)).unwrap(),
            false,
        )
        .unwrap();
        let held = d.caps().owner_count(1);
        phys.fail_writes = true;
        let mut mem = SpaceMemory::new(&sp, &mut phys);
        let args = SyscallArgs {
            nr: ipc_abi::SYS_RECV,
            a: [ep, RW, ipc_abi::FLAG_NONBLOCK, 0, 0, 0],
        };
        let r = ipc_abi::handle(
            &mut Bound {
                dom: &mut d,
                sched: &mut s,
                owner: 1,
            },
            &mut mem,
            &args,
        );
        assert_eq!(r, Abi::Done(SyscallError::Fault as i64));
        assert_eq!(
            d.caps().owner_count(1),
            held,
            "delivered capability closed again"
        );
        assert_eq!(d.caps().limbo_count(), 0);
        d.check_invariants(&s).unwrap();
    }
}
