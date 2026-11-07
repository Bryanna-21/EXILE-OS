//! Page-table tests over a simulated physical memory.

use omni8::frames::{FRAME_SIZE, FrameAllocator};
use omni8::paging::*;
use std::collections::HashMap;

#[derive(Default)]
struct SimPhys {
    frames: HashMap<u64, Vec<u64>>,
}

impl SimPhys {
    fn frame(&mut self, phys: u64) -> &mut Vec<u64> {
        assert_eq!(phys % 4096, 0, "table access must be frame aligned");
        self.frames
            .entry(phys)
            .or_insert_with(|| vec![0xDEAD_BEEF_DEAD_BEEF; 512])
    }
}

impl PhysMem for SimPhys {
    fn read_u64(&self, phys: u64) -> u64 {
        let base = phys & !0xFFF;
        match self.frames.get(&base) {
            Some(f) => f[((phys & 0xFFF) / 8) as usize],
            None => panic!("read of untouched table frame {base:#x}"),
        }
    }
    fn write_u64(&mut self, phys: u64, v: u64) {
        let i = ((phys & 0xFFF) / 8) as usize;
        self.frame(phys & !0xFFF)[i] = v;
    }
    fn zero_frame(&mut self, phys: u64) {
        self.frame(phys).iter_mut().for_each(|w| *w = 0);
    }
}

const WINDOW: UserWindow = UserWindow {
    start: 0x4000_0000,
    end: 0x8000_0000,
};

fn pool() -> Box<FrameAllocator> {
    let mut a = Box::new(FrameAllocator::new());
    a.add_range(0x10_0000, 0x80_0000);
    a
}

struct Env {
    mem: SimPhys,
    frames: Box<FrameAllocator>,
}
fn env() -> Env {
    Env {
        mem: SimPhys::default(),
        frames: pool(),
    }
}

#[test]
fn pg_01_entry_bits() {
    assert_eq!(PRESENT, 1);
    assert_eq!(WRITE, 2);
    assert_eq!(USER, 4);
    assert_eq!(HUGE, 0x80);
    assert_eq!(NX, 1u64 << 63);
    assert_eq!(ADDR_MASK, 0x000F_FFFF_FFFF_F000);
}

#[test]
fn pg_02_map_4k_and_translate() {
    let mut e = env();
    let mut m = Mapper::new(&mut e.mem, &mut *e.frames, WINDOW).unwrap();
    m.map_4k(0x10_0000, 0x10_0000, Perms::KERNEL_RX).unwrap();
    m.map_4k(0x10_1000, 0x30_0000, Perms::KERNEL_RW).unwrap();
    let t = m.translate(0x10_0123).unwrap();
    assert_eq!(
        (t.phys, t.size, t.write, t.exec, t.user),
        (0x10_0123, 4096, false, true, false)
    );
    let t = m.translate(0x10_1FF8).unwrap();
    assert_eq!(
        (t.phys, t.write, t.exec, t.user),
        (0x30_0FF8, true, false, false)
    );
    assert!(m.translate(0x10_2000).is_none());
    assert!(m.translate(0).is_none());
    assert!(
        m.translate(0x0000_9000_0000_0000).is_none(),
        "non-canonical"
    );
}

#[test]
fn pg_03_wx_is_refused() {
    let mut e = env();
    let mut m = Mapper::new(&mut e.mem, &mut *e.frames, WINDOW).unwrap();
    let wx = Perms {
        write: true,
        exec: true,
        user: false,
    };
    assert_eq!(
        m.map_4k(0x10_0000, 0x10_0000, wx),
        Err(MapError::WxViolation)
    );
    assert_eq!(
        m.map_2m(0x20_0000, 0x20_0000, wx),
        Err(MapError::WxViolation)
    );
    let uwx = Perms {
        write: true,
        exec: true,
        user: true,
    };
    assert_eq!(
        m.map_4k(0x4000_0000, 0x40_0000, uwx),
        Err(MapError::WxViolation)
    );
    assert!(m.translate(0x10_0000).is_none(), "nothing was mapped");
}

#[test]
fn pg_04_alignment_and_range_checks() {
    let mut e = env();
    let mut m = Mapper::new(&mut e.mem, &mut *e.frames, WINDOW).unwrap();
    assert_eq!(
        m.map_4k(0x10_0001, 0x10_0000, Perms::KERNEL_RW),
        Err(MapError::Misaligned)
    );
    assert_eq!(
        m.map_4k(0x10_0000, 0x10_0800, Perms::KERNEL_RW),
        Err(MapError::Misaligned)
    );
    assert_eq!(
        m.map_2m(0x20_1000, 0x20_0000, Perms::KERNEL_RW),
        Err(MapError::Misaligned)
    );
    assert_eq!(
        m.map_4k(0x0000_8000_0000_0000, 0x10_0000, Perms::KERNEL_RW),
        Err(MapError::NonCanonical)
    );
    assert_eq!(
        m.map_4k(0x10_0000, PHYS_LIMIT, Perms::KERNEL_RW),
        Err(MapError::PhysRange)
    );
    assert_eq!(
        m.map_4k(0x10_0000, PHYS_LIMIT - 4096, Perms::KERNEL_RW),
        Ok(()),
        "last legal frame"
    );
    assert_eq!(
        m.map_range_4k(0x20_0000, 0x20_0000, 0x1001, Perms::KERNEL_RW),
        Err(MapError::Misaligned)
    );
    assert_eq!(
        m.map_range_4k(u64::MAX - 0xFFF, 0x20_0000, 0x2000, Perms::KERNEL_RW),
        Err(MapError::Overflow)
    );
    assert_eq!(
        m.map_range_4k(0x20_0000, u64::MAX - 0xFFF, 0x2000, Perms::KERNEL_RW),
        Err(MapError::Overflow)
    );
}

#[test]
fn pg_05_double_map_and_huge_conflicts() {
    let mut e = env();
    let mut m = Mapper::new(&mut e.mem, &mut *e.frames, WINDOW).unwrap();
    m.map_4k(0x10_0000, 0x10_0000, Perms::KERNEL_RW).unwrap();
    assert_eq!(
        m.map_4k(0x10_0000, 0x20_0000, Perms::KERNEL_RW),
        Err(MapError::AlreadyMapped)
    );
    // a 2 MiB mapping over a region that already has a page table is a conflict
    assert_eq!(
        m.map_2m(0, 0, Perms::KERNEL_RW),
        Err(MapError::HugeConflict)
    );
    m.map_2m(0x40_0000, 0x40_0000, Perms::KERNEL_RW).unwrap();
    assert_eq!(
        m.map_2m(0x40_0000, 0x40_0000, Perms::KERNEL_RW),
        Err(MapError::AlreadyMapped)
    );
    assert_eq!(
        m.map_4k(0x40_1000, 0x60_0000, Perms::KERNEL_RW),
        Err(MapError::HugeConflict)
    );
    let t = m.translate(0x40_5678).unwrap();
    assert_eq!((t.phys, t.size), (0x40_5678, 2 * 1024 * 1024));
}

#[test]
fn pg_06_user_window_rules() {
    let mut e = env();
    let mut m = Mapper::new(&mut e.mem, &mut *e.frames, WINDOW).unwrap();
    // user mapping outside the window
    assert_eq!(
        m.map_4k(0x10_0000, 0x10_0000, Perms::USER_RW),
        Err(MapError::UserWindow)
    );
    assert_eq!(
        m.map_4k(0x3FFF_F000, 0x10_0000, Perms::USER_RW),
        Err(MapError::UserWindow)
    );
    assert_eq!(
        m.map_4k(0x8000_0000, 0x10_0000, Perms::USER_RW),
        Err(MapError::UserWindow)
    );
    // kernel mapping inside the window
    assert_eq!(
        m.map_4k(0x4000_0000, 0x10_0000, Perms::KERNEL_RW),
        Err(MapError::UserWindow)
    );
    assert_eq!(
        m.map_2m(0x4000_0000, 0x40_0000, Perms::KERNEL_RW),
        Err(MapError::UserWindow)
    );
    // a 2 MiB kernel page that merely overlaps the window start
    assert_eq!(
        m.map_2m(0x3FE0_0000, 0x40_0000, Perms::KERNEL_RW),
        Ok(()),
        "ends exactly at the window"
    );
    // legal user mappings
    assert_eq!(m.map_4k(0x4000_0000, 0x50_0000, Perms::USER_RX), Ok(()));
    assert_eq!(
        m.map_4k(0x7FFF_F000, 0x50_1000, Perms::USER_RW),
        Ok(()),
        "last window page"
    );
    let t = m.translate(0x4000_0010).unwrap();
    assert_eq!((t.user, t.write, t.exec), (true, false, true));
    // overflow of window arithmetic cannot sneak a mapping past the check
    assert_eq!(
        m.map_4k(0xFFFF_FFFF_FFFF_F000, 0x10_0000, Perms::USER_RW),
        Err(MapError::UserWindow)
    );
}

#[test]
fn pg_07_kernel_mappings_stay_supervisor_only_even_when_the_user_path_shares_upper_levels() {
    let mut e = env();
    let mut m = Mapper::new(&mut e.mem, &mut *e.frames, WINDOW).unwrap();
    m.map_2m(0, 0, Perms::KERNEL_RW).unwrap();
    m.map_4k(0x4000_0000, 0x60_0000, Perms::USER_RW).unwrap();
    // PML4[0] now carries USER (needed to reach the user window) ...
    let root = m.root();
    assert_ne!(e.mem.read_u64(root) & USER, 0);
    // ... but the kernel identity page is still not reachable from ring 3
    let k = translate(&e.mem, root, 0x1234).unwrap();
    assert!(
        !k.user,
        "effective user permission is the AND of all levels"
    );
    let u = translate(&e.mem, root, 0x4000_0000).unwrap();
    assert!(u.user && u.write);
}

#[test]
fn pg_08_unmap() {
    let mut e = env();
    let mut m = Mapper::new(&mut e.mem, &mut *e.frames, WINDOW).unwrap();
    m.map_4k(0x10_0000, 0x77_0000, Perms::KERNEL_RW).unwrap();
    assert_eq!(m.unmap_4k(0x10_0000), Ok(0x77_0000));
    assert!(m.translate(0x10_0000).is_none());
    assert_eq!(m.unmap_4k(0x10_0000), Err(MapError::NotMapped));
    assert_eq!(m.unmap_4k(0x7000_0000), Err(MapError::NotMapped));
    assert_eq!(m.unmap_4k(0x10_0001), Err(MapError::Misaligned));
    m.map_2m(0x40_0000, 0x40_0000, Perms::KERNEL_RW).unwrap();
    assert_eq!(m.unmap_4k(0x40_0000), Err(MapError::HugeConflict));
    // remapping after unmap works
    m.map_4k(0x10_0000, 0x78_0000, Perms::KERNEL_RO).unwrap();
    assert_eq!(m.translate(0x10_0000).unwrap().phys, 0x78_0000);
}

#[test]
fn pg_09_out_of_frames_is_reported_not_ignored() {
    let mut mem = SimPhys::default();
    let mut frames = Box::new(FrameAllocator::new());
    frames.add_range(0x10_0000, 0x10_0000 + 3 * 4096); // root + 2 tables, not enough for 4 levels
    let mut m = Mapper::new(&mut mem, &mut *frames, WINDOW).unwrap();
    assert_eq!(
        m.map_4k(0x10_0000, 0x20_0000, Perms::KERNEL_RW),
        Err(MapError::NoFrame)
    );
}

#[test]
fn pg_10_map_range_best_uses_huge_pages_only_when_aligned() {
    let mut e = env();
    let mut m = Mapper::new(&mut e.mem, &mut *e.frames, WINDOW).unwrap();
    // 0x1000 .. 0x60_0000: ragged head (4K), aligned middle (2M pages), ragged tail
    m.map_range_best(0x1000, 0x1000, 0x60_0000 - 0x1000, Perms::KERNEL_RW)
        .unwrap();
    assert_eq!(m.translate(0x1000).unwrap().size, 4096);
    assert_eq!(m.translate(0x20_0000).unwrap().size, 2 * 1024 * 1024);
    assert_eq!(m.translate(0x40_0000).unwrap().size, 2 * 1024 * 1024);
    assert!(m.translate(0x60_0000).is_none());
    assert_eq!(m.translate(0x5F_F000).unwrap().size, 2 * 1024 * 1024);
}

#[test]
fn pg_11_kernel_image_style_w_xor_x_layout() {
    // text RX, rodata R, data RW+NX at 4 KiB granularity inside the first 2 MiB
    let mut e = env();
    let mut m = Mapper::new(&mut e.mem, &mut *e.frames, WINDOW).unwrap();
    m.map_range_4k(0x10_0000, 0x10_0000, 0x3000, Perms::KERNEL_RX)
        .unwrap();
    m.map_range_4k(0x10_3000, 0x10_3000, 0x2000, Perms::KERNEL_RO)
        .unwrap();
    m.map_range_4k(0x10_5000, 0x10_5000, 0x9000, Perms::KERNEL_RW)
        .unwrap();
    let p = |a| m.translate(a).unwrap();
    assert!(p(0x10_2FFF).exec && !p(0x10_2FFF).write);
    assert!(!p(0x10_3000).exec && !p(0x10_3000).write);
    assert!(!p(0x10_5000).exec && p(0x10_5000).write);
    assert!(!p(0x10_D000).exec && p(0x10_D000).write);
    assert!(
        m.translate(0x10_E000).is_none(),
        "guard: nothing mapped after the image"
    );
    // no page anywhere is both writable and executable
    for a in (0x10_0000..0x10_E000).step_by(4096) {
        let t = m.translate(a).unwrap();
        assert!(!(t.write && t.exec), "W^X violated at {a:#x}");
    }
}

#[test]
fn pg_12_allocator_frames_are_what_the_tables_use() {
    let mut e = env();
    let before = e.frames.free_frames();
    let mut m = Mapper::new(&mut e.mem, &mut *e.frames, WINDOW).unwrap();
    m.map_4k(0x10_0000, 0x20_0000, Perms::KERNEL_RW).unwrap();
    // root + PDPT + PD + PT
    assert_eq!(before - e.frames.free_frames(), 4);
    assert_eq!(FRAME_SIZE, 4096);
}

#[test]
fn pg_13_from_root_rejects_bad_roots() {
    let mut e = env();
    assert!(Mapper::from_root(&mut e.mem, &mut *e.frames, 0, WINDOW).is_err());
    assert!(Mapper::from_root(&mut e.mem, &mut *e.frames, 0x1001, WINDOW).is_err());
    assert!(Mapper::from_root(&mut e.mem, &mut *e.frames, PHYS_LIMIT, WINDOW).is_err());
}
