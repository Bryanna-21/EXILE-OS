//! Shared fixtures for the Batch 9 integration tests: a simulated physical memory, a world
//! that wires Batch 7 (scheduler, syscalls, region model) to Batch 8 (page tables, frame
//! allocator) through `PagedBacking`, and small thread/syscall helpers.
//!
//! Everything here is HOSTED. Threads run on the real Batch 7 context-switch assembly on host
//! stacks; "physical memory" is a hash map. No CPU page-table walk or ring transition occurs.
#![allow(dead_code)]

use omni_kernel7::sched::{Sched, SpaceId, ThreadId};
use omni_kernel7::syscall::{Kernel, SyscallFrame};
use omni_kernel7::uaccess::Perm;
use omni8::frames::FrameAllocator;
use omni8::paging::{Mapper, Perms, PhysMem, UserWindow};
use omni9::PagedBacking;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const WINDOW: UserWindow = UserWindow {
    start: 0x4000_0000,
    end: 0x8000_0000,
};
pub const BASE: u64 = 0x4000_0000;
pub const PAGE: u64 = 4096;

#[derive(Default)]
pub struct SimPhys {
    pub frames: HashMap<u64, Vec<u64>>,
}

impl SimPhys {
    fn frame(&mut self, phys: u64) -> &mut Vec<u64> {
        assert_eq!(phys % 4096, 0, "frame access must be page aligned");
        self.frames.entry(phys).or_insert_with(|| vec![0; 512])
    }
    pub fn byte(&self, phys: u64) -> u8 {
        let w = self
            .frames
            .get(&(phys & !0xFFF))
            .map_or(0, |f| f[((phys & 0xFFF) / 8) as usize]);
        w.to_le_bytes()[(phys & 7) as usize]
    }
    pub fn fill(&mut self, phys: u64, len: u64, b: u8) {
        for a in phys..phys + len {
            let i = ((a & 0xFFF) / 8) as usize;
            let mut w = self.frame(a & !0xFFF)[i].to_le_bytes();
            w[(a & 7) as usize] = b;
            self.frame(a & !0xFFF)[i] = u64::from_le_bytes(w);
        }
    }
}

impl PhysMem for SimPhys {
    fn read_u64(&self, phys: u64) -> u64 {
        self.frames
            .get(&(phys & !0xFFF))
            .map_or(0, |f| f[((phys & 0xFFF) / 8) as usize])
    }
    fn write_u64(&mut self, phys: u64, v: u64) {
        let i = ((phys & 0xFFF) / 8) as usize;
        self.frame(phys & !0xFFF)[i] = v;
    }
    fn zero_frame(&mut self, phys: u64) {
        self.frame(phys).iter_mut().for_each(|w| *w = 0);
    }
}

pub struct Pool(pub Box<FrameAllocator>);
impl omni8::paging::FrameSource for Pool {
    fn alloc_frame(&mut self) -> Option<u64> {
        self.0.alloc()
    }
}

pub struct World {
    pub k: &'static Kernel,
    pub b: &'static PagedBacking<SimPhys>,
    pub pool: &'static Mutex<Pool>,
}

pub fn world(debug: bool) -> World {
    world_with_pool(debug, 0x10_0000, 0x100_0000)
}

pub fn world_with_pool(debug: bool, lo: u64, hi: u64) -> World {
    let b: &'static PagedBacking<SimPhys> =
        Box::leak(Box::new(PagedBacking::new(SimPhys::default())));
    let mut a = Box::new(FrameAllocator::new());
    a.add_range(lo, hi);
    let pool: &'static Mutex<Pool> = Box::leak(Box::new(Mutex::new(Pool(a))));
    // Kernel embeds the whole scheduler (16 stacks); build it on a big stack.
    let addr = std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || Box::into_raw(Box::new(Kernel::new(b, debug))) as usize)
        .unwrap()
        .join()
        .unwrap();
    // SAFETY: just leaked, never freed.
    let k = unsafe { &*(addr as *const Kernel) };
    World { k, b, pool }
}

/// One contiguous range of a user space.
#[derive(Clone, Copy)]
pub struct Range {
    pub va: u64,
    pub pages: u64,
    /// What the page tables get (None: leave unmapped in the page tables).
    pub pt: Option<Perms>,
    /// What Batch 7's region model gets (None: leave unmapped in the model).
    pub model: Option<Perm>,
    pub fill: u8,
}

impl Range {
    pub fn rw(va: u64, pages: u64, fill: u8) -> Range {
        Range {
            va,
            pages,
            pt: Some(Perms::USER_RW),
            model: Some(Perm::RW),
            fill,
        }
    }
    pub fn ro(va: u64, pages: u64, fill: u8) -> Range {
        Range {
            va,
            pages,
            pt: Some(Perms::USER_RO),
            model: Some(Perm::R),
            fill,
        }
    }
}

impl World {
    /// Create a space in both layers. Returns the page-table root.
    pub fn space(&self, id: SpaceId, ranges: &[Range]) -> u64 {
        let mut pool = self.pool.lock().unwrap();
        let root = self.b.with_mem(|mem| {
            let mut data: Vec<(u64, u64, Perms, u8)> = Vec::new();
            for r in ranges {
                if let Some(p) = r.pt {
                    for i in 0..r.pages {
                        let f = pool.0.alloc().expect("pool exhausted");
                        mem.fill(f, PAGE, r.fill);
                        data.push((r.va + i * PAGE, f, p, r.fill));
                    }
                }
            }
            let mut m = Mapper::new(mem, &mut *pool, WINDOW).expect("root");
            for (va, f, p, _) in data {
                m.map_4k(va, f, p).expect("map");
            }
            m.root()
        });
        self.b.bind(id, root).expect("bind");
        let mut sp = self.k.spaces.lock();
        sp.create(id).expect("create");
        for r in ranges {
            if let Some(perm) = r.model {
                sp.get_mut(id)
                    .unwrap()
                    .map(r.va, r.pages * PAGE, perm)
                    .expect("model map");
            }
        }
        root
    }

    pub fn phys_of(&self, root: u64, va: u64) -> Option<u64> {
        self.b
            .with_mem(|m| omni8::paging::translate(m, root, va).map(|t| t.phys))
    }

    pub fn byte_at(&self, root: u64, va: u64) -> Option<u8> {
        let p = self.phys_of(root, va)?;
        Some(self.b.with_mem(|m| m.byte(p)))
    }
}

extern "C" fn tramp(arg: usize) {
    // SAFETY: `arg` came from `spawn_fn`.
    let b = unsafe { Box::from_raw(arg as *mut Box<dyn FnOnce()>) };
    let f: Box<dyn FnOnce()> = *b;
    f();
}

pub fn spawn_fn(s: &Sched, space: SpaceId, f: impl FnOnce() + 'static) -> ThreadId {
    let b: Box<Box<dyn FnOnce()>> = Box::new(Box::new(f));
    s.spawn(space, tramp, Box::into_raw(b) as usize).unwrap()
}

pub type Log = Arc<Mutex<Vec<i64>>>;
pub fn log() -> Log {
    Arc::new(Mutex::new(Vec::new()))
}
pub fn push(l: &Log, v: i64) {
    l.lock().unwrap().push(v);
}
pub fn snap(l: &Log) -> Vec<i64> {
    l.lock().unwrap().clone()
}

pub fn sc(k: &Kernel, nr: u64, a: [u64; 6]) -> i64 {
    let mut f = SyscallFrame {
        rax: nr,
        rdi: a[0],
        rsi: a[1],
        rdx: a[2],
        r10: a[3],
        r8: a[4],
        r9: a[5],
        rcx: 0x4000_0000,
        r11: 0x202,
        user_rsp: 0x7FFF_0000,
        ..Default::default()
    };
    k.syscall(&mut f);
    f.rax as i64
}

pub fn run_all(s: &Sched) -> omni_kernel7::sched::RunExit {
    s.run(|_| false)
}
