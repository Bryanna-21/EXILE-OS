//! IDT, trap-frame and report tests.

use core::mem::{offset_of, size_of};
use omni8::desc::*;
use omni8::idt::*;
use omni8::report::*;

fn gate(h: u64, ist: usize) -> IdtEntry {
    IdtEntry::interrupt_gate(h, SEL_KERNEL_CODE, ist, 0).unwrap()
}

#[test]
fn idt_01_entry_layout_and_encoding() {
    assert_eq!(size_of::<IdtEntry>(), 16);
    assert_eq!(size_of::<Idt>(), 4096);
    assert_eq!(IDT_LIMIT, 4095);
    let e = gate(0x0000_0012_3456_789A, 3);
    assert_eq!(e.handler(), 0x0000_0012_3456_789A);
    assert_eq!(e.selector(), 0x08);
    assert_eq!(e.ist(), 3);
    assert_eq!(e.gate_type(), 0x0E, "interrupt gate: IF cleared on entry");
    assert_eq!(e.dpl(), 0);
    assert!(e.is_present());
    assert!(!IdtEntry::MISSING.is_present());
}

#[test]
fn idt_02_gate_validation() {
    assert_eq!(
        IdtEntry::interrupt_gate(0, 0x08, 0, 0).err(),
        Some(DescError::Null)
    );
    assert_eq!(
        IdtEntry::interrupt_gate(0x0000_8000_0000_0000, 0x08, 0, 0).err(),
        Some(DescError::NonCanonical)
    );
    assert!(
        IdtEntry::interrupt_gate(0x1000, 0x08, 8, 0).is_err(),
        "IST > 7"
    );
    assert!(
        IdtEntry::interrupt_gate(0x1000, 0x08, 0, 4).is_err(),
        "DPL > 3"
    );
    assert!(
        IdtEntry::interrupt_gate(0x1000, 0x10, 0, 0).is_err(),
        "handler selector must be kernel code"
    );
    assert!(IdtEntry::interrupt_gate(0x1000, 0x0B, 0, 0).is_err());
    let user_gate = IdtEntry::interrupt_gate(0x1000, 0x08, 0, 3).unwrap();
    assert_eq!(user_gate.dpl(), 3);
}

#[test]
fn idt_03_error_code_table_is_the_architectural_one() {
    let with: Vec<usize> = (0..32).filter(|&v| has_error_code(v)).collect();
    assert_eq!(with, vec![8, 10, 11, 12, 13, 14, 17, 21, 29, 30]);
    assert!(!has_error_code(0) && !has_error_code(3) && !has_error_code(6) && !has_error_code(18));
    assert!(!has_error_code(32), "IRQs push no error code");
}

#[test]
fn idt_04_ist_assignment() {
    assert_eq!(ist_for_vector(8), IST_DOUBLE_FAULT);
    assert_eq!(ist_for_vector(2), IST_NMI);
    assert_eq!(ist_for_vector(18), IST_MACHINE_CHECK);
    assert_eq!(ist_for_vector(14), IST_FATAL);
    assert_eq!(ist_for_vector(13), IST_FATAL);
    assert_eq!(ist_for_vector(32), IST_IRQ);
    assert_eq!(ist_for_vector(255), IST_IRQ);
    // the four dedicated stacks plus IRQ are distinct and within 1..=7
    let mut s = [
        IST_FATAL,
        IST_DOUBLE_FAULT,
        IST_NMI,
        IST_MACHINE_CHECK,
        IST_IRQ,
    ];
    s.sort();
    assert_eq!(s, [1, 2, 3, 4, 5]);
}

#[test]
fn idt_05_validate_requires_all_exception_gates_and_listed_extras() {
    let mut idt = Idt::new();
    assert!(idt.validate(&[]).is_err(), "empty table is invalid");
    for v in 0..32 {
        idt.set(v, gate(0x10_0000 + v as u64 * 16, ist_for_vector(v)))
            .unwrap();
    }
    assert_eq!(idt.validate(&[]), Ok(()));
    assert_eq!(
        idt.validate(&[32]),
        Err(DescError::BadEntry(32)),
        "IRQ gate missing"
    );
    idt.set(32, gate(0x10_1000, IST_IRQ)).unwrap();
    assert_eq!(idt.validate(&[32]), Ok(()));
    // corrupt one exception gate: not present
    idt.entries[13] = IdtEntry::MISSING;
    assert_eq!(idt.validate(&[]), Err(DescError::BadEntry(13)));
    idt.set(13, gate(0x10_0000 + 13 * 16, IST_FATAL)).unwrap();
    assert_eq!(idt.validate(&[]), Ok(()));
    assert_eq!(idt.set(256, gate(0x1000, 0)), Err(DescError::BadEntry(256)));
    assert_eq!(idt.validate(&[256]), Err(DescError::BadEntry(256)));
}

#[test]
fn idt_06_trap_frame_layout_matches_the_stub() {
    assert_eq!(size_of::<TrapFrame>(), TRAP_FRAME_SIZE);
    assert_eq!(TRAP_FRAME_SIZE, 176);
    // general registers sit in the reverse of push order (rax pushed first = highest)
    assert_eq!(offset_of!(TrapFrame, r15), 0);
    assert_eq!(offset_of!(TrapFrame, r8), 56);
    assert_eq!(offset_of!(TrapFrame, rdi), 64);
    assert_eq!(offset_of!(TrapFrame, rbx), 88);
    assert_eq!(offset_of!(TrapFrame, rax), 112);
    assert_eq!(offset_of!(TrapFrame, vector), 120);
    assert_eq!(offset_of!(TrapFrame, error), 128);
    // hardware frame
    assert_eq!(offset_of!(TrapFrame, rip), 136);
    assert_eq!(offset_of!(TrapFrame, cs), 144);
    assert_eq!(offset_of!(TrapFrame, rflags), 152);
    assert_eq!(offset_of!(TrapFrame, rsp), 160);
    assert_eq!(offset_of!(TrapFrame, ss), 168);
    // 176 = 11 * 16: with a 16-aligned IST top the stub's rsp is 16-aligned at the call
    assert_eq!(TRAP_FRAME_SIZE % 16, 0);
}

#[test]
fn idt_07_page_fault_decoding() {
    let d = PageFaultError::decode(0b0000_0010);
    assert!(!d.present && d.write && !d.user && !d.instruction_fetch);
    let d = PageFaultError::decode(0b0001_0101);
    assert!(d.present && !d.write && d.user && d.instruction_fetch);
    assert!(PageFaultError::decode(8).reserved_bit);
    assert!(PageFaultError::decode(32).protection_key);
    assert!(PageFaultError::decode(64).shadow_stack);
}

#[test]
fn idt_08_exception_names() {
    assert_eq!(exception_name(0), "#DE divide error");
    assert_eq!(exception_name(6), "#UD invalid opcode");
    assert_eq!(exception_name(8), "#DF double fault");
    assert_eq!(exception_name(13), "#GP general protection");
    assert_eq!(exception_name(14), "#PF page fault");
    assert_eq!(exception_name(32), "external interrupt");
    assert_eq!(exception_name(u64::MAX), "external interrupt");
    assert_eq!(EXCEPTION_NAMES.len(), 32);
}

#[test]
fn idt_09_report_format_is_deterministic() {
    let f = TrapFrame {
        vector: 14,
        error: 2,
        rip: 0x10_1234,
        cs: 8,
        rflags: 0x202,
        rsp: 0x20_0000,
        ss: 0x10,
        rax: 1,
        ..Default::default()
    };
    let mut s = String::new();
    write_trap_report(&mut s, &f, 0xDEAD_B000, 0x5000).unwrap();
    let expect = "\
OMNI8 FATAL EXCEPTION vector=14 name=#PF page fault err=0x0000000000000002
OMNI8   page-fault: write not-present supervisor cr2=0x00000000deadb000
OMNI8   rip=0x0000000000101234 cs=0x0008 ring0 rflags=0x0000000000000202 rsp=0x0000000000200000 ss=0x0010
OMNI8   rax=0x0000000000000001 rbx=0x0000000000000000 rcx=0x0000000000000000 rdx=0x0000000000000000
OMNI8   rsi=0x0000000000000000 rdi=0x0000000000000000 rbp=0x0000000000000000 cr3=0x0000000000005000
OMNI8   r8=0x0000000000000000 r9=0x0000000000000000 r10=0x0000000000000000 r11=0x0000000000000000
OMNI8   r12=0x0000000000000000 r13=0x0000000000000000 r14=0x0000000000000000 r15=0x0000000000000000
";
    assert_eq!(s, expect);
}

#[test]
fn idt_10_report_for_non_page_fault_and_user_ring() {
    let f = TrapFrame {
        vector: 6,
        error: 0,
        cs: 0x23,
        ..Default::default()
    };
    let mut s = String::new();
    write_trap_report(&mut s, &f, 0, 0).unwrap();
    assert!(s.starts_with(
        "OMNI8 FATAL EXCEPTION vector=6 name=#UD invalid opcode err=0x0000000000000000\n"
    ));
    assert!(!s.contains("page-fault:"));
    assert!(s.contains("cs=0x0023 ring3"));
    assert!(f.from_user());
    assert!(
        !TrapFrame {
            cs: 8,
            ..Default::default()
        }
        .from_user()
    );
}

#[test]
fn idt_11_page_fault_report_variants() {
    let mut s = String::new();
    write_page_fault(&mut s, 0b1_0001, 0x40_0000).unwrap();
    assert_eq!(
        s,
        "OMNI8   page-fault: fetch protection supervisor cr2=0x0000000000400000\n"
    );
    let mut s = String::new();
    write_page_fault(&mut s, 0b0_0110, 8).unwrap();
    assert_eq!(
        s,
        "OMNI8   page-fault: write not-present user cr2=0x0000000000000008\n"
    );
}
