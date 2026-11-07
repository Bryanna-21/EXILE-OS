//! End-to-end: Batch 7 syscalls and user-address validation over Batch 8 page tables.
//! HOSTED: threads are real Batch 7 threads, "physical memory" is simulated.

mod common;
use common::*;
use omni_kernel7::status;
use omni_kernel7::syscall::{DEBUG_COPY_MAX, SYS_DEBUG_CHECKSUM, SYS_DEBUG_FILL};
use omni_kernel7::uaccess::{Perm, UserBacking};
use omni8::frames::FrameAllocator;
use omni8::paging::{MapError, Mapper, Perms};
use omni9::PagedBacking;

fn run_in_space(w: &World, space: u16, f: impl FnOnce() + 'static) {
    spawn_fn(&w.k.sched, space, f);
    run_all(&w.k.sched);
}

#[test]
fn mem_01_checksum_reads_through_the_page_tables() {
    let w = world(true);
    w.space(2, &[Range::rw(BASE, 1, 7)]);
    let (k, l) = (w.k, log());
    let l2 = l.clone();
    run_in_space(&w, 2, move || {
        push(&l2, sc(k, SYS_DEBUG_CHECKSUM, [BASE, 16, 0, 0, 0, 0]))
    });
    assert_eq!(snap(&l), vec![16 * 7]);
}

#[test]
fn mem_02_fill_crosses_a_page_boundary_into_two_different_frames() {
    let w = world(true);
    // two ranges so the two pages get unrelated physical frames
    let root = w.space(2, &[Range::rw(BASE, 1, 0), Range::rw(BASE + PAGE, 1, 0)]);
    let (k, l) = (w.k, log());
    let l2 = l.clone();
    run_in_space(&w, 2, move || {
        push(
            &l2,
            sc(k, SYS_DEBUG_FILL, [BASE + PAGE - 8, 16, 0xAB, 0, 0, 0]),
        );
        push(
            &l2,
            sc(k, SYS_DEBUG_CHECKSUM, [BASE + PAGE - 8, 16, 0, 0, 0, 0]),
        );
    });
    assert_eq!(snap(&l), vec![0, 16 * 0xAB]);
    let p0 = w.phys_of(root, BASE).unwrap();
    let p1 = w.phys_of(root, BASE + PAGE).unwrap();
    assert_ne!(
        p0 & !0xFFF,
        p1 & !0xFFF,
        "the pages must be backed by different frames"
    );
    // physical truth: last 8 bytes of page 0 and first 8 of page 1 hold 0xAB, nothing else does
    w.b.with_mem(|m| {
        for i in 0..PAGE {
            let expect = if i >= PAGE - 8 { 0xAB } else { 0 };
            assert_eq!(m.byte(p0 + i), expect, "page 0 byte {i}");
        }
        for i in 0..PAGE {
            let expect = if i < 8 { 0xAB } else { 0 };
            assert_eq!(m.byte(p1 + i), expect, "page 1 byte {i}");
        }
    });
}

#[test]
fn mem_03_invalid_user_addresses_fault_without_touching_memory() {
    let w = world(true);
    let root = w.space(2, &[Range::rw(BASE, 1, 0x11)]);
    let (k, l) = (w.k, log());
    let l2 = l.clone();
    run_in_space(&w, 2, move || {
        for addr in [
            0u64,                  // null
            BASE - 1,              // just below the mapping
            BASE + PAGE,           // just past it
            0x0000_8000_0000_0000, // USER_TOP
            0xFFFF_8000_0000_0000, // kernel half
            0xFFFF_FFFF_FFFF_F000, // top of memory
            0x0001_0000_0000_0000, // non-canonical
            u64::MAX,
        ] {
            push(&l2, sc(k, SYS_DEBUG_CHECKSUM, [addr, 8, 0, 0, 0, 0]));
            push(&l2, sc(k, SYS_DEBUG_FILL, [addr, 8, 0xFF, 0, 0, 0]));
        }
        // a range that starts inside and runs past the end of the mapping
        push(
            &l2,
            sc(k, SYS_DEBUG_CHECKSUM, [BASE + PAGE - 4, 8, 0, 0, 0, 0]),
        );
        // length that would overflow the address
        push(
            &l2,
            sc(k, SYS_DEBUG_CHECKSUM, [u64::MAX - 3, 8, 0, 0, 0, 0]),
        );
    });
    assert!(
        snap(&l).iter().all(|&s| s == status::FAULT),
        "{:?}",
        snap(&l)
    );
    assert_eq!(snap(&l).len(), 18);
    assert_eq!(w.byte_at(root, BASE), Some(0x11));
    assert_eq!(w.byte_at(root, BASE + PAGE - 1), Some(0x11));
}

#[test]
fn mem_04_argument_limits_are_enforced_before_any_access() {
    let w = world(true);
    w.space(2, &[Range::rw(BASE, 2, 1)]);
    let (k, l) = (w.k, log());
    let l2 = l.clone();
    run_in_space(&w, 2, move || {
        push(
            &l2,
            sc(
                k,
                SYS_DEBUG_CHECKSUM,
                [BASE, DEBUG_COPY_MAX + 1, 0, 0, 0, 0],
            ),
        );
        push(&l2, sc(k, SYS_DEBUG_FILL, [BASE, 8, 256, 0, 0, 0]));
        push(&l2, sc(k, SYS_DEBUG_CHECKSUM, [BASE, 0, 0, 0, 0, 0]));
        push(
            &l2,
            sc(k, SYS_DEBUG_CHECKSUM, [BASE, DEBUG_COPY_MAX, 0, 0, 0, 0]),
        );
    });
    assert_eq!(
        snap(&l),
        vec![
            status::INVALID_ARGUMENT,
            status::INVALID_ARGUMENT,
            0,
            DEBUG_COPY_MAX as i64
        ]
    );
}

#[test]
fn mem_05_read_only_pages_can_be_read_but_not_written() {
    let w = world(true);
    let root = w.space(2, &[Range::ro(BASE, 1, 5)]);
    let (k, l) = (w.k, log());
    let l2 = l.clone();
    run_in_space(&w, 2, move || {
        push(&l2, sc(k, SYS_DEBUG_CHECKSUM, [BASE, 4, 0, 0, 0, 0]));
        push(&l2, sc(k, SYS_DEBUG_FILL, [BASE, 4, 9, 0, 0, 0]));
    });
    assert_eq!(snap(&l), vec![20, status::FAULT]);
    assert_eq!(w.byte_at(root, BASE), Some(5));
}

#[test]
fn mem_06_page_tables_refuse_what_the_region_model_allows() {
    let w = world(true);
    // model says RW; page tables say read-only
    let r = Range {
        va: BASE,
        pages: 1,
        pt: Some(Perms::USER_RO),
        model: Some(Perm::RW),
        fill: 3,
    };
    let root = w.space(2, &[r]);
    let (k, l) = (w.k, log());
    let l2 = l.clone();
    run_in_space(&w, 2, move || {
        push(&l2, sc(k, SYS_DEBUG_FILL, [BASE, 4, 9, 0, 0, 0]));
        push(&l2, sc(k, SYS_DEBUG_CHECKSUM, [BASE, 4, 0, 0, 0, 0]));
    });
    assert_eq!(snap(&l), vec![status::FAULT, 12]);
    assert_eq!(w.byte_at(root, BASE), Some(3));
    // model says mapped; page tables do not map it at all
    let w = world(true);
    w.space(
        2,
        &[Range {
            va: BASE,
            pages: 1,
            pt: None,
            model: Some(Perm::RW),
            fill: 0,
        }],
    );
    let (k, l) = (w.k, log());
    let l2 = l.clone();
    run_in_space(&w, 2, move || {
        push(&l2, sc(k, SYS_DEBUG_CHECKSUM, [BASE, 4, 0, 0, 0, 0]))
    });
    assert_eq!(snap(&l), vec![status::FAULT]);
}

#[test]
fn mem_07_region_model_refuses_what_the_page_tables_allow() {
    let w = world(true);
    // page tables map RW; the model has no region
    w.space(
        2,
        &[Range {
            va: BASE,
            pages: 1,
            pt: Some(Perms::USER_RW),
            model: None,
            fill: 4,
        }],
    );
    let (k, l) = (w.k, log());
    let l2 = l.clone();
    run_in_space(&w, 2, move || {
        push(&l2, sc(k, SYS_DEBUG_CHECKSUM, [BASE, 4, 0, 0, 0, 0]))
    });
    assert_eq!(
        snap(&l),
        vec![status::FAULT],
        "both layers must agree for access to succeed"
    );
}

#[test]
fn mem_08_supervisor_pages_are_never_user_accessible() {
    let w = world(true);
    // A kernel mapping below the user window, which the (too permissive) model lists as RW.
    let addr = 0x0100_0000u64;
    let root = {
        let mut pool = w.pool.lock().unwrap();
        w.b.with_mem(|mem| {
            let f = pool.0.alloc().unwrap();
            mem.fill(f, PAGE, 0x66);
            let mut m = Mapper::new(mem, &mut *pool, WINDOW).unwrap();
            m.map_4k(addr, f, Perms::KERNEL_RW).unwrap();
            m.root()
        })
    };
    w.b.bind(2, root).unwrap();
    {
        let mut sp = w.k.spaces.lock();
        sp.create(2).unwrap();
        sp.get_mut(2).unwrap().map(addr, PAGE, Perm::RW).unwrap();
    }
    let (k, l) = (w.k, log());
    let l2 = l.clone();
    run_in_space(&w, 2, move || {
        push(&l2, sc(k, SYS_DEBUG_CHECKSUM, [addr, 4, 0, 0, 0, 0]));
        push(&l2, sc(k, SYS_DEBUG_FILL, [addr, 4, 1, 0, 0, 0]));
    });
    assert_eq!(snap(&l), vec![status::FAULT, status::FAULT]);
    assert_eq!(w.byte_at(root, addr), Some(0x66));
}

#[test]
fn mem_09_two_spaces_are_isolated_from_each_other() {
    let w = world(true);
    let r2 = w.space(
        2,
        &[Range::rw(BASE, 1, 2), Range::rw(BASE + 4 * PAGE, 1, 0x20)],
    );
    let r3 = w.space(3, &[Range::rw(BASE, 1, 3)]);
    let (k, l) = (w.k, log());
    for sp in [2u16, 3] {
        let l2 = l.clone();
        spawn_fn(&k.sched, sp, move || {
            push(&l2, sp as i64);
            push(&l2, sc(k, SYS_DEBUG_CHECKSUM, [BASE, 4, 0, 0, 0, 0]));
            push(&l2, sc(k, SYS_DEBUG_FILL, [BASE, 4, 0x7E, 0, 0, 0]));
            // only space 2 maps this page
            push(
                &l2,
                sc(k, SYS_DEBUG_CHECKSUM, [BASE + 4 * PAGE, 4, 0, 0, 0, 0]),
            );
        });
    }
    run_all(&k.sched);
    assert_eq!(
        snap(&l),
        vec![2, 8, 0, 4 * 0x20, 3, 12, 0, status::FAULT],
        "each space sees its own bytes and cannot reach the other's page"
    );
    assert_eq!(w.byte_at(r2, BASE), Some(0x7E));
    assert_eq!(w.byte_at(r3, BASE), Some(0x7E));
    assert_ne!(w.phys_of(r2, BASE), w.phys_of(r3, BASE));
}

#[test]
fn mem_10_unbound_destroyed_and_kernel_spaces_are_refused() {
    let w = world(true);
    w.space(2, &[Range::rw(BASE, 1, 1)]);
    // space 4 exists in the model but was never bound to page tables
    {
        let mut sp = w.k.spaces.lock();
        sp.create(4).unwrap();
        sp.get_mut(4).unwrap().map(BASE, PAGE, Perm::RW).unwrap();
    }
    let (k, l) = (w.k, log());
    let (l1, l4, l0) = (l.clone(), l.clone(), l.clone());
    spawn_fn(&k.sched, 4, move || {
        push(&l4, sc(k, SYS_DEBUG_CHECKSUM, [BASE, 4, 0, 0, 0, 0]))
    });
    spawn_fn(&k.sched, 0, move || {
        push(&l0, sc(k, SYS_DEBUG_CHECKSUM, [BASE, 4, 0, 0, 0, 0]))
    });
    // space 2: destroy the model entry while keeping the page tables, then access
    assert!(k.spaces.lock().destroy(2));
    spawn_fn(&k.sched, 2, move || {
        push(&l1, sc(k, SYS_DEBUG_CHECKSUM, [BASE, 4, 0, 0, 0, 0]))
    });
    run_all(&k.sched);
    let mut got = snap(&l);
    got.sort();
    assert_eq!(
        got,
        vec![status::FAULT, status::ACCESS_DENIED, status::ACCESS_DENIED],
        "kernel thread and destroyed space: ACCESS_DENIED; model-only space: FAULT from the backing"
    );
}

#[test]
fn mem_11_bindings_validate_and_can_be_removed() {
    let b = PagedBacking::new(SimPhys::default());
    assert_eq!(b.bind(1, 0), Err(omni9::BindError::BadRoot));
    assert_eq!(b.bind(1, 0x1001), Err(omni9::BindError::BadRoot));
    assert_eq!(b.bind(1, 1 << 52), Err(omni9::BindError::BadRoot));
    assert_eq!(b.bind(1, 0x1000), Ok(()));
    assert_eq!(b.bind(1, 0x2000), Err(omni9::BindError::Exists));
    for s in 2..=8u16 {
        assert_eq!(b.bind(s, 0x1000 * s as u64), Ok(()));
    }
    assert_eq!(b.bind(9, 0x9000), Err(omni9::BindError::Full));
    assert!(b.unbind(3));
    assert!(!b.unbind(3));
    assert_eq!(b.bind(9, 0x9000), Ok(()));
    // an unbound space and a bound space with empty tables both refuse
    let mut out = [0u8; 4];
    assert!(!b.read(3, BASE, &mut out));
    assert!(!b.read(1, BASE, &mut out));
    // zero-length access on a bound space is a no-op success; on an unbound one it is refused
    assert!(b.read(1, BASE, &mut []));
    assert!(!b.read(3, BASE, &mut []));
}

#[test]
fn mem_12_huge_pages_are_walked_and_bounded() {
    let w = world(true);
    let mut pool = w.pool.lock().unwrap();
    let root = w.b.with_mem(|mem| {
        let mut m = Mapper::new(mem, &mut *pool, WINDOW).unwrap();
        m.map_2m(BASE, 0x4000_0000, Perms::USER_RW).unwrap();
        m.root()
    });
    drop(pool);
    w.b.bind(2, root).unwrap();
    w.b.with_mem(|m| m.fill(0x4000_0000 + 0x1F_FFF0, 16, 0x3C));
    let mut out = [0u8; 16];
    assert!(w.b.read(2, BASE + 0x1F_FFF0, &mut out));
    assert_eq!(out, [0x3C; 16]);
    // crossing the end of the 2 MiB page into unmapped space faults, and writes nothing
    assert!(!w.b.write(2, BASE + 0x1F_FFF8, &[1u8; 16]));
    assert_eq!(w.b.with_mem(|m| m.byte(0x4000_0000 + 0x1F_FFF8)), 0x3C);
}

#[test]
fn mem_13_a_write_spanning_writable_and_read_only_pages_writes_nothing() {
    let w = world(true);
    let root = w.space(
        2,
        &[Range::rw(BASE, 1, 0x10), Range::ro(BASE + PAGE, 1, 0x20)],
    );
    assert!(!w.b.write(2, BASE + PAGE - 4, &[0xEE; 8]));
    for i in 0..PAGE {
        assert_eq!(
            w.byte_at(root, BASE + i),
            Some(0x10),
            "page 0 byte {i} changed by a refused write"
        );
    }
    assert!(w.b.write(2, BASE + PAGE - 4, &[0xEE; 4]));
    assert_eq!(w.byte_at(root, BASE + PAGE - 1), Some(0xEE));
}

#[test]
fn mem_14_a_refused_copy_from_user_zeroes_the_output_buffer() {
    let w = world(true);
    w.space(
        2,
        &[Range {
            va: BASE,
            pages: 1,
            pt: None,
            model: Some(Perm::RW),
            fill: 0,
        }],
    );
    let mut out = [0xAAu8; 16];
    let r = w.k.spaces.lock().copy_from_user(w.b, 2, 2, BASE, &mut out);
    assert_eq!(r, Err(omni_kernel7::uaccess::UserError::Fault));
    assert_eq!(
        out, [0u8; 16],
        "no stale or partial data may leak to the caller"
    );
}

#[test]
fn mem_15_a_caller_whose_space_is_not_loaded_is_refused() {
    let w = world(true);
    w.space(2, &[Range::rw(BASE, 1, 1)]);
    let mut out = [0u8; 4];
    let r = w.k.spaces.lock().copy_from_user(w.b, 2, 3, BASE, &mut out);
    assert_eq!(r, Err(omni_kernel7::uaccess::UserError::SpaceMismatch));
}

#[test]
fn mem_16_mapper_allocation_failure_leaves_no_visible_mapping() {
    // Pool of exactly 2 frames: the root and one table; a 4 KiB mapping needs 3 tables + data.
    let mut mem = SimPhys::default();
    let mut a = Box::new(FrameAllocator::new());
    a.add_range(0x10_0000, 0x10_0000 + 2 * PAGE);
    let mut pool = Pool(a);
    let mut m = Mapper::new(&mut mem, &mut pool, WINDOW).unwrap();
    let r = m.map_4k(BASE, 0x20_0000, Perms::USER_RW);
    assert_eq!(r, Err(MapError::NoFrame));
    assert_eq!(
        m.translate(BASE),
        None,
        "a failed map must not leave a usable mapping"
    );
    let root = m.root();
    // KNOWN LIMITATION (documented in the security review): tables allocated before the
    // failure stay linked and are not returned to the pool.
    assert_eq!(pool.0.free_frames(), 0);
    let _ = (FrameAllocator::new(), root);
}

#[test]
fn mem_17_frame_pool_exhaustion_is_reported_not_papered_over() {
    let w = world_with_pool(true, 0x10_0000, 0x10_0000 + 6 * PAGE);
    // root + 3 tables + 1 data frame use 5 of the 6 frames; one more root fits, a second cannot
    w.space(2, &[Range::rw(BASE, 1, 1)]);
    let mut pool = w.pool.lock().unwrap();
    let r =
        w.b.with_mem(|mem| Mapper::new(mem, &mut *pool, WINDOW).map(|m| m.root()));
    assert!(r.is_ok(), "one frame is still left for a root");
    let r2 =
        w.b.with_mem(|mem| Mapper::new(mem, &mut *pool, WINDOW).map(|m| m.root()));
    assert_eq!(r2, Err(MapError::NoFrame));
}
