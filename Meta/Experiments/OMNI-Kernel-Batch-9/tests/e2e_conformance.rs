//! Cross-batch conformance (Rust side) and concurrency. The Zig side is checked by
//! `scripts/check_abi_conformance.py`.

mod common;
use common::*;
use omni_kernel7::msr;
use omni_kernel7::sync::SpinLock;
use omni_kernel7::syscall::{SyscallFrame, entry_stub_bytes, validate_frame};
use omni_kernel7::uaccess::{USER_TOP, UserBacking};
use omni8::desc::{self, Gdt};
use omni8::idt::TRAP_FRAME_SIZE;
use omni8::usermode::{self, UserEntryError};
use std::sync::Arc;

#[test]
fn cf_01_gdt_descriptors_match_between_batch7_and_batch8() {
    // Batch 7 and Batch 8 each define the four flat descriptors. They must be bit-identical.
    let g = Gdt::new(0xFFFF_8000_0010_0000).unwrap();
    assert_eq!(g.entries[1], msr::GDT_KERNEL_CODE64);
    assert_eq!(g.entries[2], msr::GDT_KERNEL_DATA);
    assert_eq!(g.entries[3], msr::GDT_USER_DATA);
    assert_eq!(g.entries[4], msr::GDT_USER_CODE64);
    assert_eq!(desc::SEL_KERNEL_CODE, msr::KERNEL_CS);
    assert_eq!(desc::SEL_KERNEL_DATA, msr::KERNEL_SS);
    assert_eq!(desc::SEL_USER_DATA, msr::USER_SS);
    assert_eq!(desc::SEL_USER_CODE, msr::USER_CS);
    assert_eq!(
        msr::sysret_selectors(msr::SYSRET_BASE),
        (msr::USER_CS, msr::USER_SS)
    );
    assert_eq!(
        msr::star_value(msr::KERNEL_CS, msr::SYSRET_BASE) >> 48,
        msr::SYSRET_BASE as u64
    );
    desc::validate_selector_layout().unwrap();
}

#[test]
fn cf_02_user_window_lies_inside_the_batch7_user_range_and_entry_checks_agree() {
    const { assert!(WINDOW.end <= USER_TOP) };
    for (rip, rsp) in [
        (WINDOW.start, WINDOW.start + 8),
        (WINDOW.end - 1, WINDOW.end),
        (WINDOW.start, WINDOW.end),
        (0, 0x4000_1000),
        (USER_TOP, 0x4000_1000),
        (0x4000_0000, USER_TOP),
        (0xFFFF_8000_0000_0000, 0x4000_1000),
        (0x4000_0000, 0x4000_1003),
    ] {
        let enter = usermode::iret_frame(rip, rsp, WINDOW);
        let mut f = SyscallFrame {
            rax: 0,
            rcx: rip,
            user_rsp: rsp,
            r11: 0x202,
            ..Default::default()
        };
        let ret_ok = validate_frame(&mut f);
        // Soundness: whatever Batch 8 lets enter ring 3 must also be a state Batch 7 would let
        // return to ring 3. (Batch 8 is stricter: it also demands the user window.)
        if enter.is_ok() {
            assert!(
                ret_ok,
                "iret_frame accepted rip={rip:#x} rsp={rsp:#x} but validate_frame refuses it"
            );
        }
    }
    // USER_TOP itself is the first non-canonical address, so it is refused as such.
    assert_eq!(
        usermode::iret_frame(USER_TOP, 0x4000_1000, WINDOW),
        Err(UserEntryError::NonCanonical)
    );
    assert_eq!(
        usermode::iret_frame(0x0000_7FFF_FFFF_F000, 0x4000_1000, WINDOW),
        Err(UserEntryError::OutsideWindow)
    );
    assert!(usermode::iret_frame(0x4000_0000, 0x4000_1000, WINDOW).is_ok());
}

#[test]
fn cf_03_register_frame_sizes_are_pinned() {
    assert_eq!(
        std::mem::size_of::<SyscallFrame>(),
        128,
        "the Batch 7 entry stub pushes 16 qwords"
    );
    assert_eq!(TRAP_FRAME_SIZE, 176, "Batch 8 isr_common saves 22 qwords");
}

#[test]
fn cf_04_entry_stub_contains_the_privileged_instructions_it_claims() {
    let s = entry_stub_bytes();
    let has = |pat: &[u8]| s.windows(pat.len()).any(|w| w == pat);
    assert!(has(&[0x0F, 0x01, 0xF8]), "swapgs");
    assert!(has(&[0x48, 0x0F, 0x07]), "sysretq");
    assert!(
        !has(&[0x48, 0xCF]),
        "the SYSCALL path must not return with iretq"
    );
}

#[test]
fn cf_05_syscall_numbers_are_disjoint_from_the_batch5_ipc_numbers() {
    use omni_kernel7::syscall::*;
    let b7 = [
        SYS_YIELD,
        SYS_EXIT,
        SYS_SLEEP,
        SYS_THREAD_SELF,
        SYS_SPACE_SELF,
        SYS_DEBUG_CHECKSUM,
        SYS_DEBUG_FILL,
        SYS_DEBUG_ARGS,
    ];
    for nr in b7 {
        assert!(
            !(0x100..=0x107).contains(&nr),
            "{nr:#x} collides with a Batch 5 IPC syscall"
        );
    }
    let mut v = b7.to_vec();
    v.sort();
    v.dedup();
    assert_eq!(v.len(), b7.len(), "Batch 7 numbers are unique");
}

#[test]
fn conc_01_spinlock_serialises_real_os_threads() {
    let l = Arc::new(SpinLock::new(0u64));
    let hs: Vec<_> = (0..4)
        .map(|_| {
            let l = l.clone();
            std::thread::spawn(move || {
                for _ in 0..20_000 {
                    *l.lock() += 1;
                }
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    assert_eq!(*l.lock(), 80_000);
}

#[test]
fn conc_02_paged_backing_is_safe_under_concurrent_access_from_os_threads() {
    let w = world(true);
    let roots: Vec<u64> = (2u16..=5)
        .map(|s| w.space(s, &[Range::rw(BASE, 1, 0)]))
        .collect();
    let b = w.b;
    let hs: Vec<_> = (2u16..=5)
        .map(|s| {
            std::thread::spawn(move || {
                let data = [s as u8; 64];
                for _ in 0..2_000 {
                    assert!(b.write(s, BASE, &data));
                    let mut out = [0u8; 64];
                    assert!(b.read(s, BASE, &mut out));
                    assert_eq!(out, data);
                    // another space's page is never visible
                    let mut o2 = [0u8; 1];
                    assert!(!b.read(s, BASE + 4096, &mut o2));
                }
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    for (r, s) in roots.iter().zip(2u8..) {
        assert_eq!(w.byte_at(*r, BASE + 63), Some(s));
    }
}
