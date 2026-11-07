//! Timer parameter tests, user-mode entry validation, and Batch 7 compatibility checks.

use omni_kernel7::{msr, sched, uaccess};
use omni8::desc::{SEL_USER_CODE, SEL_USER_DATA};
use omni8::idt::{IRQ_BASE, TIMER_VECTOR, VECTOR_USER_TEST};
use omni8::paging::UserWindow;
use omni8::timer::*;
use omni8::usermode::*;

#[test]
fn tm_01_pit_divisors() {
    assert_eq!(divisor_for(100), Ok(11932));
    assert_eq!(divisor_for(1000), Ok(1193));
    assert_eq!(divisor_for(19), Ok(62799));
    assert_eq!(divisor_for(PIT_BASE_HZ), Ok(1));
    assert_eq!(divisor_for(18), Err(TimerError::Frequency));
    assert_eq!(divisor_for(0), Err(TimerError::Frequency));
    assert_eq!(divisor_for(PIT_BASE_HZ + 1), Err(TimerError::Frequency));
    assert_eq!(divisor_for(u32::MAX), Err(TimerError::Frequency));
}

#[test]
fn tm_02_pit_sequence() {
    assert_eq!(
        pit_sequence(100).unwrap(),
        [(0x43, 0x34), (0x40, 0x9C), (0x40, 0x2E)]
    );
    assert_eq!(pit_sequence(5), Err(TimerError::Frequency));
}

#[test]
fn tm_03_pic_sequence_remaps_above_the_exceptions() {
    let s = pic_sequence(0x20, 0x28, 0xFE, 0xFF).unwrap();
    assert_eq!(
        s,
        [
            (0x20, 0x11),
            (0xA0, 0x11),
            (0x21, 0x20),
            (0xA1, 0x28),
            (0x21, 0x04),
            (0xA1, 0x02),
            (0x21, 0x01),
            (0xA1, 0x01),
            (0x21, 0xFE),
            (0xA1, 0xFF),
        ]
    );
    assert_eq!(
        pic_sequence(0x08, 0x28, 0, 0),
        Err(TimerError::Offset),
        "would overlap CPU exceptions"
    );
    assert_eq!(
        pic_sequence(0x21, 0x28, 0, 0),
        Err(TimerError::Offset),
        "not a multiple of 8"
    );
    assert_eq!(pic_sequence(0x20, 0x24, 0, 0), Err(TimerError::Offset));
    assert_eq!(
        pic_sequence(0x20, 0x20, 0, 0),
        Err(TimerError::Offset),
        "both PICs on the same vectors"
    );
    assert_eq!(IRQ_BASE, 0x20);
    assert_eq!(TIMER_VECTOR, IRQ_BASE, "IRQ0 is the timer");
}

#[test]
fn tm_04_tick_counter() {
    let t = Ticks::new();
    assert_eq!(t.get(), 0);
    assert_eq!(t.tick(), 1);
    assert_eq!(t.tick(), 2);
    assert_eq!(t.get(), 2);
}

const W: UserWindow = UserWindow {
    start: 0x4000_0000,
    end: 0x8000_0000,
};

#[test]
fn um_01_iret_frame() {
    let f = iret_frame(0x4000_0000, 0x4000_2000, W).unwrap();
    assert_eq!(
        f,
        [
            0x4000_0000,
            SEL_USER_CODE as u64,
            0x202,
            0x4000_2000,
            SEL_USER_DATA as u64
        ]
    );
    assert_eq!(f[2] & 0x3000, 0, "IOPL 0");
    assert_ne!(
        f[2] & 0x200,
        0,
        "IF set: interrupts stay deliverable in ring 3"
    );
    assert_eq!(f[1] & 3, 3);
    assert_eq!(f[4] & 3, 3);
}

#[test]
fn um_02_iret_frame_rejects_bad_state() {
    assert_eq!(
        iret_frame(0x1000, 0x4000_2000, W),
        Err(UserEntryError::OutsideWindow),
        "kernel-range rip"
    );
    assert_eq!(
        iret_frame(0x4000_0000, 0x1000, W),
        Err(UserEntryError::OutsideWindow)
    );
    assert_eq!(
        iret_frame(0x8000_0000, 0x4000_2000, W),
        Err(UserEntryError::OutsideWindow)
    );
    assert_eq!(
        iret_frame(0x4000_0000, 0x4000_0000, W),
        Err(UserEntryError::OutsideWindow),
        "stack grows down out of the window"
    );
    assert_eq!(
        iret_frame(0x4000_0000, 0x8000_0000, W),
        Ok([0x4000_0000, 0x23, 0x202, 0x8000_0000, 0x1B]),
        "stack top at window end is fine"
    );
    assert_eq!(
        iret_frame(0x4000_0000, 0x8000_0008, W),
        Err(UserEntryError::OutsideWindow)
    );
    assert_eq!(
        iret_frame(0x4000_0000, 0x4000_2004, W),
        Err(UserEntryError::MisalignedStack)
    );
    assert_eq!(
        iret_frame(0x4000_0000, 0, W),
        Err(UserEntryError::NullStack)
    );
    assert_eq!(
        iret_frame(0x0000_9000_0000_0000, 0x4000_2000, W),
        Err(UserEntryError::NonCanonical)
    );
    assert_eq!(
        iret_frame(0x4000_0000, 0xFFFF_FFFF_FFFF_FFF8, W),
        Err(UserEntryError::OutsideWindow)
    );
}

#[test]
fn bc_01_selectors_agree_with_batch7_msr_module() {
    assert_eq!(msr::USER_CS, SEL_USER_CODE);
    assert_eq!(msr::USER_SS, SEL_USER_DATA);
    assert_eq!(
        msr::star_value(msr::KERNEL_CS, msr::SYSRET_BASE) >> 48,
        0x10
    );
}

#[test]
fn bc_02_user_window_lies_inside_batch7_user_space_and_above_the_kernel_identity_map() {
    const { assert!(W.end <= uaccess::USER_TOP) };
    // the identity-mapped kernel region is at most 1 GiB (the frame allocator ceiling)
    const { assert!(W.start >= omni8::frames::CEILING) };
}

#[test]
fn bc_03_batch7_context_constants_match_the_boot_stack_assumptions() {
    use omni_kernel7::context;
    assert_eq!(
        context::STACK_SIZE % 4096,
        0,
        "thread stacks are page multiples"
    );
    assert_eq!(context::FRAME_BYTES, 64);
    assert_eq!(context::INITIAL_RFLAGS, 0x202, "threads start with IF set");
    const { assert!(sched::MAX_THREADS >= 4) };
}

#[test]
fn bc_04_user_test_vector_is_outside_exceptions_and_irqs() {
    assert!(
        VECTOR_USER_TEST as usize >= 48,
        "clear of CPU exceptions and the 16 PIC vectors"
    );
    assert_eq!(VECTOR_USER_TEST, 0x81);
}
