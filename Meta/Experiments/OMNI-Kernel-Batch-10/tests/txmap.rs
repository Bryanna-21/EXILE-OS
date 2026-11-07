//! SEC-14: transactional mapping and table teardown. HOSTED: page tables live in a hash map.

use omni8::paging::{
    FrameSource, HUGE_SIZE, MapError, Mapper, PAGE_SIZE, PHYS_LIMIT, Perms, PhysMem, UserWindow,
    translate,
};
use omni10::txmap::{FrameRelease, TX_MAX_UNITS, TxError, destroy_tables, map_range_atomic};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const WIN: UserWindow = UserWindow {
    start: 0x4000_0000,
    end: 0x8000_0000,
};
const DATA: u64 = 0x1_0000_0000; // data frames are outside the pool: only tables are pooled
const POOL_LO: u64 = 0x10_0000;

#[derive(Default)]
struct Mem(HashMap<u64, Box<[u64; 512]>>);
impl Mem {
    fn frame(&mut self, p: u64) -> &mut [u64; 512] {
        self.0
            .entry(p & !0xFFF)
            .or_insert_with(|| Box::new([0; 512]))
    }
}
impl PhysMem for Mem {
    fn read_u64(&self, phys: u64) -> u64 {
        self.0
            .get(&(phys & !0xFFF))
            .map_or(0, |f| f[((phys & 0xFFF) / 8) as usize])
    }
    fn write_u64(&mut self, phys: u64, v: u64) {
        self.frame(phys)[((phys & 0xFFF) / 8) as usize] = v;
    }
    fn zero_frame(&mut self, phys: u64) {
        self.frame(phys).fill(0);
    }
}

/// Table-frame pool with ownership tracking: a double free or a foreign free panics.
struct Pool {
    free: Vec<u64>,
    live: BTreeSet<u64>,
}
impl Pool {
    fn new(n: u64) -> Self {
        Pool {
            free: (0..n).rev().map(|i| POOL_LO + i * PAGE_SIZE).collect(),
            live: BTreeSet::new(),
        }
    }
}
impl FrameSource for Pool {
    fn alloc_frame(&mut self) -> Option<u64> {
        let f = self.free.pop()?;
        assert!(self.live.insert(f));
        Some(f)
    }
}
impl FrameRelease for Pool {
    fn free_frame(&mut self, phys: u64) {
        assert!(
            self.live.remove(&phys),
            "double or foreign free of {phys:#x}"
        );
        self.free.push(phys);
    }
}

type Snap = BTreeMap<u64, Vec<u64>>;
fn snap(mem: &Mem, pool: &Pool) -> Snap {
    pool.live
        .iter()
        .map(|&f| (f, mem.0.get(&f).map_or(vec![0; 512], |b| b.to_vec())))
        .collect()
}

/// A root plus one kernel mapping outside the user window, so lower-level tables exist and
/// share the first PML4 slot with the user window.
fn space(spare: u64) -> (Mem, Pool, u64) {
    let mut mem = Mem::default();
    let mut pool = Pool::new(1 + 3 + spare);
    let root = {
        let mut m = Mapper::new(&mut mem, &mut pool, WIN).unwrap();
        m.map_4k(0x20_0000, DATA, Perms::KERNEL_RW).unwrap();
        m.root()
    };
    (mem, pool, root)
}

fn atomic(
    mem: &mut Mem,
    pool: &mut Pool,
    root: u64,
    virt: u64,
    len: u64,
    perms: Perms,
    best: bool,
) -> Result<(), TxError> {
    map_range_atomic(mem, pool, root, WIN, virt, DATA, len, perms, best)
}

#[test]
fn tx_01_success_matches_plain_mapper_and_leaks_nothing() {
    let (mut mem, mut pool, root) = space(8);
    atomic(
        &mut mem,
        &mut pool,
        root,
        WIN.start,
        8 * PAGE_SIZE,
        Perms::USER_RW,
        false,
    )
    .unwrap();
    for i in 0..8 {
        let t = translate(&mem, root, WIN.start + i * PAGE_SIZE).unwrap();
        assert_eq!(t.phys, DATA + i * PAGE_SIZE);
        assert!(t.user && t.write && !t.exec);
    }
    // PML4 + kernel path (PDPT, PD, PT) + user path (PD, PT); user shares PML4 and PDPT[0]? No:
    // 0x4000_0000 is PDPT slot 1, so it adds a PD and a PT.
    assert_eq!(pool.live.len(), 1 + 3 + 2);
}

#[test]
fn tx_02_no_frame_mid_range_restores_tables_and_pool() {
    // 32 pages fit the first PT, then the second PT needs a frame the pool does not have.
    let (mut mem, mut pool, root) = space(2); // PD + PT#0 only
    let start = WIN.start + HUGE_SIZE - 32 * PAGE_SIZE;
    let before = snap(&mem, &pool);
    let free_before = pool.free.len();
    let r = atomic(
        &mut mem,
        &mut pool,
        root,
        start,
        64 * PAGE_SIZE,
        Perms::USER_RW,
        false,
    );
    assert_eq!(r, Err(TxError::Map(MapError::NoFrame)));
    assert_eq!(pool.free.len(), free_before, "frames leaked");
    assert_eq!(snap(&mem, &pool), before, "page tables not restored");
    for i in 0..64 {
        assert!(translate(&mem, root, start + i * PAGE_SIZE).is_none());
    }
}

#[test]
fn tx_03_already_mapped_mid_range_unmaps_the_pages_written_so_far() {
    let (mut mem, mut pool, root) = space(8);
    {
        let mut m = Mapper::from_root(&mut mem, &mut pool, root, WIN).unwrap();
        m.map_4k(WIN.start + 10 * PAGE_SIZE, DATA + 0x80_0000, Perms::USER_RO)
            .unwrap();
    }
    let before = snap(&mem, &pool);
    let r = atomic(
        &mut mem,
        &mut pool,
        root,
        WIN.start,
        16 * PAGE_SIZE,
        Perms::USER_RW,
        false,
    );
    assert_eq!(r, Err(TxError::Map(MapError::AlreadyMapped)));
    assert_eq!(snap(&mem, &pool), before);
    for i in 0..16 {
        let t = translate(&mem, root, WIN.start + i * PAGE_SIZE);
        if i == 10 {
            assert_eq!(
                t.unwrap().phys,
                DATA + 0x80_0000,
                "pre-existing mapping disturbed"
            );
        } else {
            assert!(t.is_none(), "page {i} left mapped");
        }
    }
}

#[test]
fn tx_04_user_bit_upgrade_of_an_existing_table_is_rolled_back() {
    let (mut mem, mut pool, root) = space(0); // no spare frames: the user path cannot be built
    let pml4_before = mem.read_u64(root);
    assert_eq!(
        pml4_before & 4,
        0,
        "kernel path must start without the USER bit"
    );
    let r = atomic(
        &mut mem,
        &mut pool,
        root,
        WIN.start,
        PAGE_SIZE,
        Perms::USER_RW,
        false,
    );
    assert_eq!(r, Err(TxError::Map(MapError::NoFrame)));
    // descend() upgraded PML4[0] with USER before running out of frames; it must be undone.
    assert_eq!(mem.read_u64(root), pml4_before);
}

#[test]
fn tx_05_oversized_request_is_refused_before_anything_changes() {
    let (mut mem, mut pool, root) = space(8);
    let before = snap(&mem, &pool);
    let n = TX_MAX_UNITS as u64 + 1;
    let r = atomic(
        &mut mem,
        &mut pool,
        root,
        WIN.start,
        n * PAGE_SIZE,
        Perms::USER_RW,
        false,
    );
    assert_eq!(r, Err(TxError::TooLarge));
    assert_eq!(snap(&mem, &pool), before);
    // exactly the maximum is accepted
    atomic(
        &mut mem,
        &mut pool,
        root,
        WIN.start,
        TX_MAX_UNITS as u64 * PAGE_SIZE,
        Perms::USER_RW,
        false,
    )
    .unwrap();
}

#[test]
fn tx_06_validation_failures_change_nothing() {
    let (mut mem, mut pool, root) = space(8);
    let before = snap(&mem, &pool);
    let wx = Perms {
        write: true,
        exec: true,
        user: true,
    };
    let cases: [(u64, u64, Perms, MapError); 4] = [
        (WIN.start, PAGE_SIZE, wx, MapError::WxViolation),
        (
            WIN.start + 1,
            PAGE_SIZE,
            Perms::USER_RW,
            MapError::Misaligned,
        ),
        (WIN.end, PAGE_SIZE, Perms::USER_RW, MapError::UserWindow),
        (0x20_0000, PAGE_SIZE, Perms::USER_RW, MapError::UserWindow),
    ];
    for (v, len, p, want) in cases {
        let r = atomic(&mut mem, &mut pool, root, v, len, p, false);
        assert_eq!(r, Err(TxError::Map(want)), "virt {v:#x}");
        assert_eq!(snap(&mem, &pool), before);
    }
    let r = atomic(
        &mut mem,
        &mut pool,
        root,
        WIN.start,
        PAGE_SIZE + 1,
        Perms::USER_RW,
        false,
    );
    assert_eq!(r, Err(TxError::Map(MapError::Misaligned)));
    // bad root is refused without touching the pool
    let r = atomic(
        &mut mem,
        &mut pool,
        0,
        WIN.start,
        PAGE_SIZE,
        Perms::USER_RW,
        false,
    );
    assert_eq!(r, Err(TxError::Map(MapError::PhysRange)));
    assert_eq!(snap(&mem, &pool), before);
}

#[test]
fn tx_07_best_mode_with_huge_pages_rolls_back() {
    let (mut mem, mut pool, root) = space(3);
    {
        // occupy the second 2 MiB slot with a 4 KiB mapping so the huge map there conflicts
        let mut m = Mapper::from_root(&mut mem, &mut pool, root, WIN).unwrap();
        m.map_4k(
            WIN.start + HUGE_SIZE + PAGE_SIZE,
            DATA + 0x90_0000,
            Perms::USER_RO,
        )
        .unwrap();
    }
    let before = snap(&mem, &pool);
    // first 2 MiB huge page maps, the second collides with the existing PT
    let r = map_range_atomic(
        &mut mem,
        &mut pool,
        root,
        WIN,
        WIN.start,
        0x2000_0000,
        2 * HUGE_SIZE,
        Perms::USER_RW,
        true,
    );
    assert!(
        matches!(
            r,
            Err(TxError::Map(
                MapError::HugeConflict | MapError::AlreadyMapped
            ))
        ),
        "{r:?}"
    );
    assert_eq!(snap(&mem, &pool), before);
    assert!(
        translate(&mem, root, WIN.start).is_none(),
        "first huge page left mapped"
    );
    assert!(translate(&mem, root, WIN.start + HUGE_SIZE + PAGE_SIZE).is_some());
}

#[test]
fn tx_08_repeated_failures_never_accumulate_leaks() {
    let (mut mem, mut pool, root) = space(2);
    let start = WIN.start + HUGE_SIZE - 32 * PAGE_SIZE;
    let free0 = pool.free.len();
    let before = snap(&mem, &pool);
    for _ in 0..200 {
        let r = atomic(
            &mut mem,
            &mut pool,
            root,
            start,
            64 * PAGE_SIZE,
            Perms::USER_RW,
            false,
        );
        assert_eq!(r, Err(TxError::Map(MapError::NoFrame)));
    }
    assert_eq!(pool.free.len(), free0);
    assert_eq!(snap(&mem, &pool), before);
}

#[test]
fn tx_09_destroy_tables_frees_every_table_and_no_data_frame() {
    let (mut mem, mut pool, root) = space(16);
    atomic(
        &mut mem,
        &mut pool,
        root,
        WIN.start,
        40 * PAGE_SIZE,
        Perms::USER_RW,
        false,
    )
    .unwrap();
    map_range_atomic(
        &mut mem,
        &mut pool,
        root,
        WIN,
        WIN.start + 2 * HUGE_SIZE,
        0x4000_0000,
        HUGE_SIZE,
        Perms::USER_RW,
        true,
    )
    .unwrap();
    let tables = pool.live.len();
    let freed = destroy_tables(&mut mem, &mut pool, root, PHYS_LIMIT);
    assert_eq!(freed, tables);
    assert!(pool.live.is_empty(), "table frames leaked: {:?}", pool.live);
    assert_eq!(pool.free.len(), 1 + 3 + 16, "pool not whole again");
}

#[test]
fn tx_10_destroy_tables_rejects_bad_roots_and_double_destroy_is_caught() {
    let (mut mem, mut pool, root) = space(4);
    for bad in [0u64, root + 1, PHYS_LIMIT, u64::MAX] {
        assert_eq!(destroy_tables(&mut mem, &mut pool, bad, PHYS_LIMIT), 0);
    }
    assert_eq!(pool.live.len(), 4, "a rejected root changed the pool");
    destroy_tables(&mut mem, &mut pool, root, PHYS_LIMIT);
    // The pool's ownership tracking turns a second teardown into a loud failure.
    let again = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        destroy_tables(&mut mem, &mut pool, root, PHYS_LIMIT)
    }));
    assert!(again.is_err(), "double destruction went unnoticed");
}
