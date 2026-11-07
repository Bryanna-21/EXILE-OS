//! Physical frame allocator tests.

use omni8::frames::*;

const MIB: u64 = 1024 * 1024;

fn alloc() -> Box<FrameAllocator> {
    Box::new(FrameAllocator::new())
}

#[test]
fn fr_01_empty_allocator_has_nothing() {
    let mut a = alloc();
    assert_eq!(a.free_frames(), 0);
    assert_eq!(a.pool_frames(), 0);
    assert_eq!(a.alloc(), None);
}

#[test]
fn fr_02_add_range_narrows_inward() {
    let mut a = alloc();
    assert_eq!(a.add_range(0x1001, 0x4FFF), 2, "0x2000 and 0x3000 only");
    assert_eq!(a.alloc(), Some(0x2000));
    assert_eq!(a.alloc(), Some(0x3000));
    assert_eq!(a.alloc(), None);
    assert_eq!(a.add_range(0x5000, 0x5000), 0);
    assert_eq!(a.add_range(0x6000, 0x5000), 0);
    assert_eq!(
        a.add_range(u64::MAX - 10, u64::MAX),
        0,
        "overflowing start is ignored"
    );
}

#[test]
fn fr_03_add_range_is_idempotent_and_clipped() {
    let mut a = alloc();
    assert_eq!(a.add_range(MIB, 2 * MIB), 256);
    assert_eq!(a.add_range(MIB, 2 * MIB), 0, "already in the pool");
    assert_eq!(
        a.add_range(2 * MIB - 0x2000, 2 * MIB + 0x1000),
        1,
        "only the new frame is added"
    );
    let mut b = alloc();
    assert_eq!(
        b.add_range(CEILING - 0x2000, CEILING + 10 * MIB),
        2,
        "clipped at the ceiling"
    );
    assert_eq!(b.add_range(CEILING, CEILING + MIB), 0);
}

#[test]
fn fr_04_reserve_widens_outward() {
    let mut a = alloc();
    a.add_range(MIB, 2 * MIB);
    assert_eq!(
        a.reserve_range(MIB + 1, MIB + 0x1001),
        2,
        "partial frames are reserved whole"
    );
    assert!(!a.is_free(MIB));
    assert!(!a.is_free(MIB + 0x1000));
    assert!(a.is_free(MIB + 0x2000));
    assert_eq!(a.free_frames(), 254);
    assert_eq!(a.reserve_range(5, 5), 0);
    assert_eq!(a.reserve_range(0, 100), 0, "outside the pool");
    assert_eq!(
        a.reserve_range(MIB, u64::MAX),
        254,
        "huge end clamps instead of overflowing"
    );
    assert_eq!(a.free_frames(), 0);
}

#[test]
fn fr_05_alloc_never_returns_a_reserved_or_foreign_frame() {
    let mut a = alloc();
    a.add_range(MIB, MIB + 0x10_000);
    a.reserve_range(MIB + 0x4000, MIB + 0x8000);
    let mut got = vec![];
    while let Some(f) = a.alloc() {
        got.push(f);
    }
    assert_eq!(got.len(), 12);
    assert!(got.iter().all(|&f| (MIB..MIB + 0x10_000).contains(&f)));
    assert!(
        got.iter()
            .all(|&f| !(MIB + 0x4000..MIB + 0x8000).contains(&f))
    );
    let mut s = got.clone();
    s.sort();
    s.dedup();
    assert_eq!(s.len(), got.len(), "no frame handed out twice");
}

#[test]
fn fr_06_free_and_double_free() {
    let mut a = alloc();
    a.add_range(MIB, MIB + 0x4000);
    let f = a.alloc().unwrap();
    assert_eq!(a.free(f), Ok(()));
    assert_eq!(a.free(f), Err(FrameError::NotAllocated), "double free");
    assert_eq!(
        a.free(MIB + 0x2000),
        Err(FrameError::NotAllocated),
        "never allocated"
    );
    assert_eq!(
        a.free(0x9000),
        Err(FrameError::NotAllocated),
        "not in the pool"
    );
    assert_eq!(a.free(f + 1), Err(FrameError::Misaligned));
    assert_eq!(a.free(CEILING), Err(FrameError::OutOfRange));
    assert_eq!(a.free(!0xFFF), Err(FrameError::OutOfRange));
    assert_eq!(a.alloc(), Some(f), "freed frame is reused first");
}

#[test]
fn fr_07_reserved_frame_cannot_be_freed_into_the_pool() {
    let mut a = alloc();
    a.add_range(MIB, MIB + 0x4000);
    let f = a.alloc().unwrap();
    a.reserve_range(f, f + 1); // reserving an allocated frame removes it from the pool
    assert_eq!(a.free(f), Err(FrameError::NotAllocated));
    assert_eq!(a.free_frames(), 3);
}

#[test]
fn fr_08_exhaustion_and_accounting() {
    let mut a = alloc();
    a.add_range(0, 0x8000);
    assert_eq!(a.pool_frames(), 8);
    let v: Vec<_> = (0..8).map(|_| a.alloc().unwrap()).collect();
    assert_eq!(a.alloc(), None);
    assert_eq!(a.free_frames(), 0);
    for f in v {
        a.free(f).unwrap();
    }
    assert_eq!(a.free_frames(), 8);
    assert_eq!(a.pool_frames(), 8);
}

#[test]
fn fr_09_capacity_constants() {
    assert_eq!(MAX_FRAMES as u64 * FRAME_SIZE, CEILING);
    assert_eq!(CEILING, 1 << 30);
}
