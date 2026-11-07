//! GDT / TSS descriptor tests (unit-level; nothing here loads a descriptor into a CPU).

use core::mem::{offset_of, size_of};
use omni_kernel7::msr;
use omni8::desc::*;

#[test]
fn gdt_01_layout_and_limit() {
    assert_eq!(size_of::<Gdt>(), 56);
    assert_eq!(GDT_LIMIT, 55);
    let g = Gdt::new(0x0020_0000).unwrap();
    assert_eq!(g.entries[0], 0);
    assert_eq!(g.entries[1], 0x00AF_9A00_0000_FFFF);
    assert_eq!(g.entries[2], 0x00CF_9200_0000_FFFF);
    assert_eq!(g.entries[3], 0x00CF_F200_0000_FFFF);
    assert_eq!(g.entries[4], 0x00AF_FA00_0000_FFFF);
}

#[test]
fn gdt_02_selectors_match_the_syscall_layout() {
    assert_eq!(SEL_KERNEL_CODE, 0x08);
    assert_eq!(SEL_KERNEL_DATA, 0x10);
    assert_eq!(SEL_USER_DATA, 0x1B);
    assert_eq!(SEL_USER_CODE, 0x23);
    assert_eq!(SEL_TSS, 0x28);
    assert_eq!(validate_selector_layout(), Ok(()));
    // SYSRET derives these from STAR[63:48] = 0x10 (Batch 7)
    assert_eq!(
        msr::sysret_selectors(msr::SYSRET_BASE),
        (SEL_USER_CODE, SEL_USER_DATA)
    );
    // user selectors index entries 4 and 3 and carry RPL 3
    assert_eq!(SEL_USER_CODE >> 3, 4);
    assert_eq!(SEL_USER_DATA >> 3, 3);
}

#[test]
fn gdt_03_tss_descriptor_encoding() {
    let base = 0xFFFF_8000_1234_5678u64;
    let [lo, hi] = tss_descriptor(base, 103);
    assert_eq!(lo & 0xFFFF, 103);
    assert_eq!((lo >> 16) & 0xFF_FFFF, base & 0xFF_FFFF);
    assert_eq!(
        (lo >> 40) & 0xFF,
        0x89,
        "present, DPL0, available 64-bit TSS"
    );
    assert_eq!((lo >> 48) & 0xF, 0, "limit 19:16");
    assert_eq!(lo >> 56, (base >> 24) & 0xFF);
    assert_eq!(hi, base >> 32);
    // round trip a low address as the kernel uses
    let [lo, hi] = tss_descriptor(0x0012_3450, 103);
    assert_eq!(
        ((lo >> 16) & 0xFF_FFFF) | ((lo >> 56) << 24) | (hi << 32),
        0x0012_3450
    );
}

#[test]
fn gdt_04_new_rejects_bad_tss_base() {
    assert_eq!(Gdt::new(0), Err(DescError::Null));
    assert_eq!(
        Gdt::new(0x0000_9000_0000_0000),
        Err(DescError::NonCanonical)
    );
    assert_eq!(Gdt::new(0x2001), Err(DescError::Misaligned));
    assert!(Gdt::new(0xFFFF_FFFF_8000_0000).is_ok());
}

#[test]
fn gdt_05_validate_detects_every_corruption() {
    let base = 0x0020_0000u64;
    let good = Gdt::new(base).unwrap();
    assert_eq!(good.validate(base), Ok(()));
    for i in 0..GDT_ENTRIES {
        let mut bad = good;
        bad.entries[i] ^= 1 << 41; // flip a bit inside the access byte / type
        assert!(
            bad.validate(base).is_err(),
            "entry {i} corruption not detected"
        );
    }
    // clearing the L bit of kernel code (a 32-bit code segment in long mode) is caught
    let mut bad = good;
    bad.entries[1] &= !(1u64 << 53);
    assert_eq!(bad.validate(base), Err(DescError::BadEntry(1)));
    // wrong TSS base
    assert!(good.validate(base + 8).is_err());
    // wrong table length
    assert!(validate_entries(&good.entries[..6], base).is_err());
}

#[test]
fn gdt_06_user_descriptors_are_ring3_and_kernel_ones_ring0() {
    let g = Gdt::new(0x0020_0000).unwrap();
    let dpl = |v: u64| (v >> 45) & 3;
    assert_eq!(dpl(g.entries[1]), 0);
    assert_eq!(dpl(g.entries[2]), 0);
    assert_eq!(dpl(g.entries[3]), 3);
    assert_eq!(dpl(g.entries[4]), 3);
    assert_eq!(dpl(g.entries[5]), 0, "TSS is not loadable from ring 3");
    // user code must be 64-bit (L) and not 32-bit (D)
    assert_ne!(g.entries[4] & (1 << 53), 0);
    assert_eq!(g.entries[4] & (1 << 54), 0);
}

#[test]
fn tss_01_layout_matches_the_architecture() {
    assert_eq!(size_of::<Tss>(), TSS_SIZE);
    assert_eq!(TSS_SIZE, 104);
    assert_eq!(offset_of!(Tss, rsp), 4);
    assert_eq!(offset_of!(Tss, ist), 36);
    assert_eq!(offset_of!(Tss, iomap_base), 102);
}

#[test]
fn tss_02_new_validates_stacks() {
    let ist = [
        0x2000_0000,
        0x2000_4000,
        0x2000_8000,
        0x2000_C000,
        0x2001_0000,
        0,
        0,
    ];
    let t = Tss::new(0x2002_0000, ist).unwrap();
    assert_eq!(t.rsp0(), 0x2002_0000);
    let got = t.ist;
    assert_eq!(got, ist);
    let iomap = t.iomap_base;
    assert_eq!(
        iomap as usize, TSS_SIZE,
        "no I/O bitmap: ring 3 port access always faults"
    );
    assert_eq!(Tss::new(0, ist).err(), Some(DescError::Null));
    assert_eq!(
        Tss::new(0x2002_0008, ist).err(),
        Some(DescError::Misaligned)
    );
    assert_eq!(
        Tss::new(0x0000_9000_0000_0000, ist).err(),
        Some(DescError::NonCanonical)
    );
    let mut bad = ist;
    bad[2] = 0x2000_8004;
    assert_eq!(
        Tss::new(0x2002_0000, bad).err(),
        Some(DescError::Misaligned)
    );
    bad[2] = 0x0000_8000_0000_0000;
    assert_eq!(
        Tss::new(0x2002_0000, bad).err(),
        Some(DescError::NonCanonical)
    );
}

#[test]
fn tss_03_set_rsp0_validates() {
    let mut t = Tss::new(0x2002_0000, [0; 7]).unwrap();
    assert_eq!(t.set_rsp0(0x2003_0000), Ok(()));
    assert_eq!(t.rsp0(), 0x2003_0000);
    assert_eq!(t.set_rsp0(0), Err(DescError::Null));
    assert_eq!(t.set_rsp0(0x2003_0001), Err(DescError::Misaligned));
    assert_eq!(
        t.set_rsp0(0x0000_8000_0000_0000),
        Err(DescError::NonCanonical)
    );
    assert_eq!(t.rsp0(), 0x2003_0000, "failed updates change nothing");
}
