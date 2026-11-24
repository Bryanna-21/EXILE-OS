//! Per-thread TLS blocks and FS/GS base management (x86-64). SPDX-License-Identifier: Apache-2.0
//!
//! Layout is the System V x86-64 "variant II" TLS model: the thread's TLS block sits *below* the
//! thread pointer (TP), and the thread control block (TCB) starts at TP. The first word of the
//! TCB holds TP itself, so `fs:0` reads back the thread pointer, as the ABI requires.
//!
//! ```text
//! start            start+block_size = TP
//!   | .tdata | .tbss + padding |  TCB: [TP][reserved 0]  |
//!   '- filesz bytes copied  '- zeroed                    '- 16 bytes
//! ```
//!
//! * `TlsTemplate` validates the image description (alignment, sizes, a hard size cap) and
//!   computes a `TlsLayout` with checked arithmetic only.
//! * `TlsArena<N>` carves per-thread blocks out of one caller-supplied region. It owns no memory:
//!   all initialisation and scrubbing goes through the `TlsMemory` trait, which the caller backs
//!   with whatever mapping covers the region. Blocks never overlap (isolation), a block is owned
//!   by exactly one owner key, handles are generation-checked, and `free` scrubs the block before
//!   the space can be reused.
//! * `TlsContext` is the per-thread FS/GS value pair. `switch_out`/`switch_in` move it through
//!   the `TlsMsr` trait so context-switch persistence is host-testable. The real MSR access
//!   (`HwTlsMsr`) is compiled for x86-64 but is never executed by the host tests.
//!
//! GS convention: with the `swapgs` discipline used by `usermode.rs`, the *user's* GS base lives
//! in `IA32_KERNEL_GS_BASE` while the CPU is in the kernel, and `IA32_GS_BASE` holds the
//! kernel's per-CPU pointer. `TlsContext::gs_base` is therefore the user GS base and is
//! saved/restored through `IA32_KERNEL_GS_BASE`; `IA32_GS_BASE` is never touched here.
//!
//! Limits: one CPU (no cross-CPU arena locking; the caller serialises access), no dynamic TLS
//! (`__tls_get_addr`/DTV), no TLS for modules loaded after thread creation, no FSGSBASE
//! instructions (MSR path only).
#![allow(dead_code)]

pub const IA32_FS_BASE: u32 = 0xC000_0100;
pub const IA32_GS_BASE: u32 = 0xC000_0101;
pub const IA32_KERNEL_GS_BASE: u32 = 0xC000_0102;

/// Largest accepted TLS image (memsz), a deliberate policy cap against hostile ELF headers.
pub const MAX_TLS_MEMSZ: u64 = 64 * 1024;
/// Largest accepted TLS alignment.
pub const MAX_TLS_ALIGN: u64 = 4096;
/// Smallest alignment used for any block (TCB self-pointer must be naturally aligned).
pub const MIN_BLOCK_ALIGN: u64 = 16;
/// Size of the thread control block at the thread pointer: `[self pointer][reserved]`.
pub const TCB_SIZE: u64 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsError {
    /// Alignment is zero, not a power of two, or above `MAX_TLS_ALIGN`; or `filesz > memsz`.
    BadTemplate,
    /// `memsz` above `MAX_TLS_MEMSZ`.
    TooLarge,
    /// The arena region is empty, wraps, or is non-canonical.
    BadRegion,
    /// A size computation overflowed.
    Overflow,
    /// No gap in the region can hold the block.
    NoSpace,
    /// All handle slots are in use (or retired).
    NoSlots,
    /// The handle does not name a live block.
    Stale,
    /// This owner already has a live block.
    OwnerExists,
    /// The supplied image does not have exactly `filesz` bytes.
    ImageSize,
    /// An FS/GS value is not a canonical address.
    NonCanonical,
    /// `TlsMemory` refused an access.
    Memory,
}

fn is_canonical(addr: u64) -> bool {
    (((addr as i64) << 16) >> 16) as u64 == addr
}

fn align_up(v: u64, align: u64) -> Option<u64> {
    debug_assert!(align.is_power_of_two());
    v.checked_add(align - 1).map(|x| x & !(align - 1))
}

/// Description of a thread's TLS image (the ELF PT_TLS program header).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TlsTemplate {
    filesz: u64,
    memsz: u64,
    align: u64,
}

/// Computed placement of one thread's TLS block relative to its (aligned) start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TlsLayout {
    /// Bytes below TP: `memsz` rounded up to `alignment`. TP is at `start + block_size`.
    pub block_size: u64,
    /// `block_size + TCB_SIZE`.
    pub total: u64,
    /// Required alignment of `start` (and therefore of TP): `max(align, 16)`.
    pub alignment: u64,
}

impl TlsTemplate {
    pub fn new(filesz: u64, memsz: u64, align: u64) -> Result<Self, TlsError> {
        if align == 0 || !align.is_power_of_two() || align > MAX_TLS_ALIGN || filesz > memsz {
            return Err(TlsError::BadTemplate);
        }
        if memsz > MAX_TLS_MEMSZ {
            return Err(TlsError::TooLarge);
        }
        Ok(Self {
            filesz,
            memsz,
            align,
        })
    }

    pub const fn filesz(&self) -> u64 {
        self.filesz
    }
    pub const fn memsz(&self) -> u64 {
        self.memsz
    }
    pub const fn align(&self) -> u64 {
        self.align
    }

    pub fn layout(&self) -> Result<TlsLayout, TlsError> {
        let alignment = self.align.max(MIN_BLOCK_ALIGN);
        let block_size = align_up(self.memsz, alignment).ok_or(TlsError::Overflow)?;
        let total = block_size.checked_add(TCB_SIZE).ok_or(TlsError::Overflow)?;
        Ok(TlsLayout {
            block_size,
            total,
            alignment,
        })
    }
}

/// Bounds-checked access to the memory that backs the arena region.
pub trait TlsMemory {
    fn write(&mut self, addr: u64, data: &[u8]) -> Result<(), TlsError>;
    fn zero(&mut self, addr: u64, len: u64) -> Result<(), TlsError>;
    fn write_u64(&mut self, addr: u64, value: u64) -> Result<(), TlsError> {
        self.write(addr, &value.to_le_bytes())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TlsHandle {
    index: u32,
    generation: u32,
}

/// Where a live block is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TlsBlock {
    pub owner: u64,
    /// First byte of the block (start of `.tdata`).
    pub start: u64,
    /// Thread pointer: the value to load into FS base.
    pub thread_pointer: u64,
    pub total: u64,
}

#[derive(Clone, Copy)]
struct Slot {
    generation: u32,
    /// Generation counter exhausted: never reused.
    retired: bool,
    used: bool,
    owner: u64,
    start: u64,
    tp: u64,
    total: u64,
}

const EMPTY: Slot = Slot {
    generation: 0,
    retired: false,
    used: false,
    owner: 0,
    start: 0,
    tp: 0,
    total: 0,
};

/// Fixed-capacity TLS block allocator over `[base, base + len)`.
pub struct TlsArena<const N: usize> {
    base: u64,
    len: u64,
    slots: [Slot; N],
}

impl<const N: usize> TlsArena<N> {
    pub fn new(base: u64, len: u64) -> Result<Self, TlsError> {
        let end = base.checked_add(len).ok_or(TlsError::BadRegion)?;
        if len == 0 || !is_canonical(base) || !is_canonical(end - 1) {
            return Err(TlsError::BadRegion);
        }
        Ok(Self {
            base,
            len,
            slots: [EMPTY; N],
        })
    }

    pub fn live(&self) -> usize {
        self.slots.iter().filter(|s| s.used).count()
    }

    pub fn used_bytes(&self) -> u64 {
        self.slots.iter().filter(|s| s.used).map(|s| s.total).sum()
    }

    fn slot(&self, h: TlsHandle) -> Result<&Slot, TlsError> {
        match self.slots.get(h.index as usize) {
            Some(s) if s.used && s.generation == h.generation => Ok(s),
            _ => Err(TlsError::Stale),
        }
    }

    pub fn block(&self, h: TlsHandle) -> Result<TlsBlock, TlsError> {
        let s = self.slot(h)?;
        Ok(TlsBlock {
            owner: s.owner,
            start: s.start,
            thread_pointer: s.tp,
            total: s.total,
        })
    }

    /// Handle of the live block owned by `owner`, if any.
    pub fn find_owner(&self, owner: u64) -> Option<TlsHandle> {
        self.slots
            .iter()
            .position(|s| s.used && s.owner == owner)
            .map(|i| TlsHandle {
                index: i as u32,
                generation: self.slots[i].generation,
            })
    }

    /// FS/GS value pair for a thread running on this block (user GS base starts at zero).
    pub fn context_for(&self, h: TlsHandle) -> Result<TlsContext, TlsError> {
        Ok(TlsContext {
            fs_base: self.slot(h)?.tp,
            gs_base: 0,
        })
    }

    /// Lowest aligned start inside the region where `total` bytes fit without touching a block.
    fn find_gap(&self, total: u64, alignment: u64) -> Option<u64> {
        let region_end = self.base + self.len; // validated in `new`
        let mut best: Option<u64> = None;
        let try_from = |cand: u64, best: &mut Option<u64>| {
            let Some(start) = align_up(cand, alignment) else {
                return;
            };
            let Some(end) = start.checked_add(total) else {
                return;
            };
            if end > region_end || best.is_some_and(|b| b <= start) {
                return;
            }
            let clash = self
                .slots
                .iter()
                .any(|s| s.used && start < s.start + s.total && s.start < end);
            if !clash {
                *best = Some(start);
            }
        };
        try_from(self.base, &mut best);
        for s in self.slots.iter().filter(|s| s.used) {
            try_from(s.start + s.total, &mut best);
        }
        best
    }

    /// Allocates and initialises a block for `owner`. `image` must be exactly the template's
    /// `filesz` bytes. On any failure nothing stays allocated and the extent is scrubbed.
    pub fn alloc<M: TlsMemory>(
        &mut self,
        owner: u64,
        template: &TlsTemplate,
        image: &[u8],
        mem: &mut M,
    ) -> Result<TlsHandle, TlsError> {
        if image.len() as u64 != template.filesz {
            return Err(TlsError::ImageSize);
        }
        if self.slots.iter().any(|s| s.used && s.owner == owner) {
            return Err(TlsError::OwnerExists);
        }
        let layout = template.layout()?;
        let index = self
            .slots
            .iter()
            .position(|s| !s.used && !s.retired)
            .ok_or(TlsError::NoSlots)?;
        let start = self
            .find_gap(layout.total, layout.alignment)
            .ok_or(TlsError::NoSpace)?;
        let tp = start + layout.block_size;

        let init = (|| {
            mem.write(start, image)?;
            mem.zero(start + template.filesz, layout.block_size - template.filesz)?;
            mem.write_u64(tp, tp)?;
            mem.write_u64(tp + 8, 0)
        })();
        if let Err(e) = init {
            let _ = mem.zero(start, layout.total);
            return Err(e);
        }
        let s = &mut self.slots[index];
        s.used = true;
        s.owner = owner;
        s.start = start;
        s.tp = tp;
        s.total = layout.total;
        Ok(TlsHandle {
            index: index as u32,
            generation: s.generation,
        })
    }

    /// Scrubs and releases a block. If scrubbing fails the block stays allocated (retryable).
    pub fn free<M: TlsMemory>(&mut self, h: TlsHandle, mem: &mut M) -> Result<(), TlsError> {
        let (start, total) = {
            let s = self.slot(h)?;
            (s.start, s.total)
        };
        mem.zero(start, total)?;
        let s = &mut self.slots[h.index as usize];
        s.used = false;
        s.owner = 0;
        s.start = 0;
        s.tp = 0;
        s.total = 0;
        match s.generation.checked_add(1) {
            Some(g) => s.generation = g,
            None => s.retired = true,
        }
        Ok(())
    }

    /// Thread-destruction cleanup: frees `owner`'s block if it has one. Returns whether it did.
    pub fn free_owner<M: TlsMemory>(&mut self, owner: u64, mem: &mut M) -> Result<bool, TlsError> {
        match self.find_owner(owner) {
            Some(h) => self.free(h, mem).map(|()| true),
            None => Ok(false),
        }
    }

    /// Structural self-check: blocks are inside the region, aligned, disjoint, TP is inside.
    pub fn check_invariants(&self) -> Result<(), &'static str> {
        let end = self.base + self.len;
        for (i, a) in self.slots.iter().enumerate().filter(|(_, s)| s.used) {
            if a.start < self.base || a.start + a.total > end {
                return Err("block outside region");
            }
            if a.tp < a.start || a.tp + TCB_SIZE != a.start + a.total {
                return Err("thread pointer misplaced");
            }
            for b in self.slots[i + 1..].iter().filter(|s| s.used) {
                if a.start < b.start + b.total && b.start < a.start + a.total {
                    return Err("blocks overlap");
                }
                if a.owner == b.owner {
                    return Err("duplicate owner");
                }
            }
        }
        Ok(())
    }
}

/// MSR access used for FS/GS base switching.
pub trait TlsMsr {
    fn rdmsr(&mut self, msr: u32) -> u64;
    fn wrmsr(&mut self, msr: u32, value: u64);
}

/// The FS/GS state of one thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TlsContext {
    pub fs_base: u64,
    /// User GS base (held in `IA32_KERNEL_GS_BASE` while in the kernel; see the module docs).
    pub gs_base: u64,
}

impl TlsContext {
    pub fn validate(&self) -> Result<(), TlsError> {
        if is_canonical(self.fs_base) && is_canonical(self.gs_base) {
            Ok(())
        } else {
            Err(TlsError::NonCanonical)
        }
    }
}

/// Captures the running thread's FS/GS state (the thread may have changed it itself).
pub fn switch_out<M: TlsMsr>(msr: &mut M) -> TlsContext {
    TlsContext {
        fs_base: msr.rdmsr(IA32_FS_BASE),
        gs_base: msr.rdmsr(IA32_KERNEL_GS_BASE),
    }
}

/// Loads `next`. A non-canonical value is rejected before any MSR is written (a `wrmsr` of a
/// non-canonical base would raise #GP in the kernel).
pub fn switch_in<M: TlsMsr>(msr: &mut M, next: &TlsContext) -> Result<(), TlsError> {
    next.validate()?;
    msr.wrmsr(IA32_FS_BASE, next.fs_base);
    msr.wrmsr(IA32_KERNEL_GS_BASE, next.gs_base);
    Ok(())
}

/// Save `prev` and load `next` in one step.
pub fn switch<M: TlsMsr>(
    msr: &mut M,
    prev: &mut TlsContext,
    next: &TlsContext,
) -> Result<(), TlsError> {
    next.validate()?;
    *prev = switch_out(msr);
    switch_in(msr, next)
}

#[cfg(target_arch = "x86_64")]
pub struct HwTlsMsr(());

#[cfg(target_arch = "x86_64")]
impl HwTlsMsr {
    /// # Safety
    /// Ring 0 only; FS/GS base MSRs exist on every x86-64 CPU.
    pub const unsafe fn new() -> Self {
        Self(())
    }
}

#[cfg(target_arch = "x86_64")]
impl TlsMsr for HwTlsMsr {
    fn rdmsr(&mut self, msr: u32) -> u64 {
        let (lo, hi): (u32, u32);
        // SAFETY: `HwTlsMsr::new` contract: ring 0, MSR exists.
        unsafe {
            core::arch::asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi,
                options(nomem, nostack, preserves_flags));
        }
        (hi as u64) << 32 | lo as u64
    }
    fn wrmsr(&mut self, msr: u32, value: u64) {
        // SAFETY: as above; `switch_in` writes only canonical values.
        unsafe {
            core::arch::asm!("wrmsr", in("ecx") msr, in("eax") value as u32,
                in("edx") (value >> 32) as u32, options(nomem, nostack, preserves_flags));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u64 = 0x0000_1000_0000;
    const LEN: u64 = 0x4000;

    struct Mem {
        base: u64,
        bytes: std::vec::Vec<u8>,
        fail_after: Option<usize>,
        ops: usize,
    }
    impl Mem {
        fn new() -> Self {
            Self {
                base: BASE,
                bytes: vec![0xAA; LEN as usize],
                fail_after: None,
                ops: 0,
            }
        }
        fn range(&mut self, addr: u64, len: u64) -> Result<core::ops::Range<usize>, TlsError> {
            self.ops += 1;
            if self.fail_after.is_some_and(|n| self.ops > n) {
                return Err(TlsError::Memory);
            }
            let off = addr.checked_sub(self.base).ok_or(TlsError::Memory)?;
            let end = off.checked_add(len).ok_or(TlsError::Memory)?;
            if end > self.bytes.len() as u64 {
                return Err(TlsError::Memory);
            }
            Ok(off as usize..end as usize)
        }
        fn read(&self, addr: u64, len: usize) -> &[u8] {
            let o = (addr - self.base) as usize;
            &self.bytes[o..o + len]
        }
        fn u64_at(&self, addr: u64) -> u64 {
            u64::from_le_bytes(self.read(addr, 8).try_into().unwrap())
        }
    }
    impl TlsMemory for Mem {
        fn write(&mut self, addr: u64, data: &[u8]) -> Result<(), TlsError> {
            let r = self.range(addr, data.len() as u64)?;
            self.bytes[r].copy_from_slice(data);
            Ok(())
        }
        fn zero(&mut self, addr: u64, len: u64) -> Result<(), TlsError> {
            let r = self.range(addr, len)?;
            self.bytes[r].fill(0);
            Ok(())
        }
    }

    struct FakeMsr {
        fs: u64,
        kgs: u64,
        gs: u64,
        writes: std::vec::Vec<(u32, u64)>,
    }
    impl FakeMsr {
        fn new() -> Self {
            Self {
                fs: 0,
                kgs: 0,
                gs: 0xFFFF_8000_0000_1000,
                writes: vec![],
            }
        }
    }
    impl TlsMsr for FakeMsr {
        fn rdmsr(&mut self, msr: u32) -> u64 {
            match msr {
                IA32_FS_BASE => self.fs,
                IA32_KERNEL_GS_BASE => self.kgs,
                IA32_GS_BASE => self.gs,
                _ => panic!("unexpected msr {msr:#x}"),
            }
        }
        fn wrmsr(&mut self, msr: u32, v: u64) {
            self.writes.push((msr, v));
            match msr {
                IA32_FS_BASE => self.fs = v,
                IA32_KERNEL_GS_BASE => self.kgs = v,
                IA32_GS_BASE => self.gs = v,
                _ => panic!("unexpected msr {msr:#x}"),
            }
        }
    }

    fn tpl() -> TlsTemplate {
        TlsTemplate::new(8, 24, 8).unwrap()
    }
    const IMG: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

    #[test]
    fn template_validation() {
        assert_eq!(
            TlsTemplate::new(0, 0, 1).unwrap().layout().unwrap().total,
            16
        );
        assert_eq!(TlsTemplate::new(0, 0, 0), Err(TlsError::BadTemplate));
        assert_eq!(TlsTemplate::new(0, 8, 3), Err(TlsError::BadTemplate));
        assert_eq!(TlsTemplate::new(0, 8, 8192), Err(TlsError::BadTemplate));
        assert_eq!(TlsTemplate::new(9, 8, 8), Err(TlsError::BadTemplate));
        assert_eq!(
            TlsTemplate::new(0, MAX_TLS_MEMSZ + 1, 8),
            Err(TlsError::TooLarge)
        );
        assert!(TlsTemplate::new(MAX_TLS_MEMSZ, MAX_TLS_MEMSZ, 4096).is_ok());
        assert_eq!(
            TlsTemplate::new(u64::MAX, u64::MAX, 8),
            Err(TlsError::TooLarge)
        );
    }

    #[test]
    fn layout_math() {
        let l = TlsTemplate::new(1, 3, 1).unwrap().layout().unwrap();
        assert_eq!((l.block_size, l.total, l.alignment), (16, 32, 16));
        let l = TlsTemplate::new(8, 24, 8).unwrap().layout().unwrap();
        assert_eq!((l.block_size, l.total, l.alignment), (32, 48, 16));
        let l = TlsTemplate::new(0, 100, 64).unwrap().layout().unwrap();
        assert_eq!((l.block_size, l.alignment), (128, 64));
        assert_eq!(l.block_size % l.alignment, 0, "TP stays aligned");
        let l = TlsTemplate::new(0, MAX_TLS_MEMSZ, 4096)
            .unwrap()
            .layout()
            .unwrap();
        assert_eq!(l.block_size, MAX_TLS_MEMSZ);
    }

    #[test]
    fn layout_overflow_is_caught() {
        // Construct directly: `new` caps memsz, so this proves `layout` is independently checked.
        let t = TlsTemplate {
            filesz: 0,
            memsz: u64::MAX - 3,
            align: 4096,
        };
        assert_eq!(t.layout(), Err(TlsError::Overflow));
        let t = TlsTemplate {
            filesz: 0,
            memsz: u64::MAX - 20,
            align: 8,
        };
        assert_eq!(t.layout(), Err(TlsError::Overflow));
    }

    #[test]
    fn alloc_initialises_image_bss_and_tcb() {
        let mut m = Mem::new();
        let mut a = TlsArena::<4>::new(BASE + 1, LEN - 1).unwrap();
        let h = a.alloc(7, &tpl(), &IMG, &mut m).unwrap();
        let b = a.block(h).unwrap();
        assert_eq!(b.start % 16, 0, "start honours alignment even for odd base");
        assert_eq!(m.read(b.start, 8), IMG);
        assert!(
            m.read(b.start + 8, 24).iter().all(|&x| x == 0),
            "BSS zeroed"
        );
        assert_eq!(b.thread_pointer, b.start + 32);
        assert_eq!(m.u64_at(b.thread_pointer), b.thread_pointer, "fs:0 is TP");
        assert_eq!(m.u64_at(b.thread_pointer + 8), 0);
        assert_eq!(a.context_for(h).unwrap().fs_base, b.thread_pointer);
        a.check_invariants().unwrap();
    }

    #[test]
    fn threads_are_isolated() {
        let mut m = Mem::new();
        let mut a = TlsArena::<4>::new(BASE, LEN).unwrap();
        let h1 = a.alloc(1, &tpl(), &IMG, &mut m).unwrap();
        let h2 = a.alloc(2, &tpl(), &IMG, &mut m).unwrap();
        let (b1, b2) = (a.block(h1).unwrap(), a.block(h2).unwrap());
        assert!(b1.start + b1.total <= b2.start || b2.start + b2.total <= b1.start);
        assert_ne!(b1.thread_pointer, b2.thread_pointer);
        // Thread 1 scribbles over its whole block (incl. TCB): thread 2 is unaffected.
        m.write(b1.start, &vec![0x55; b1.total as usize]).unwrap();
        assert_eq!(m.read(b2.start, 8), IMG);
        assert_eq!(m.u64_at(b2.thread_pointer), b2.thread_pointer);
        a.check_invariants().unwrap();
    }

    #[test]
    fn contexts_survive_simulated_context_switches() {
        let mut m = Mem::new();
        let mut a = TlsArena::<4>::new(BASE, LEN).unwrap();
        let h1 = a.alloc(1, &tpl(), &IMG, &mut m).unwrap();
        let h2 = a.alloc(2, &tpl(), &IMG, &mut m).unwrap();
        let mut c1 = a.context_for(h1).unwrap();
        let mut c2 = a.context_for(h2).unwrap();
        let mut msr = FakeMsr::new();
        switch_in(&mut msr, &c1).unwrap();
        assert_eq!(msr.fs, c1.fs_base);
        // Thread 1 sets its own user GS base, then is switched out for thread 2.
        msr.kgs = 0x0000_2000_0000;
        switch(&mut msr, &mut c1, &c2).unwrap();
        assert_eq!(c1.gs_base, 0x0000_2000_0000, "thread-set GS captured");
        assert_eq!(msr.fs, c2.fs_base);
        assert_eq!(msr.kgs, 0, "thread 2 does not inherit thread 1's GS");
        // Thread 2 moves FS within its block; switch back to thread 1.
        msr.fs = c2.fs_base - 8;
        switch(&mut msr, &mut c2, &c1).unwrap();
        assert_eq!(c2.fs_base, a.block(h2).unwrap().thread_pointer - 8);
        assert_eq!(msr.fs, a.block(h1).unwrap().thread_pointer);
        assert_eq!(msr.kgs, 0x0000_2000_0000);
        assert!(
            msr.writes.iter().all(|(r, _)| *r != IA32_GS_BASE),
            "kernel GS untouched"
        );
        assert_eq!(msr.gs, 0xFFFF_8000_0000_1000);
    }

    #[test]
    fn non_canonical_bases_are_rejected_before_any_write() {
        let mut msr = FakeMsr::new();
        for bad in [
            TlsContext {
                fs_base: 0x0000_8000_0000_0000,
                gs_base: 0,
            },
            TlsContext {
                fs_base: 0,
                gs_base: 0x1234_0000_0000_0000,
            },
        ] {
            assert_eq!(switch_in(&mut msr, &bad), Err(TlsError::NonCanonical));
            let mut prev = TlsContext::default();
            assert_eq!(
                switch(&mut msr, &mut prev, &bad),
                Err(TlsError::NonCanonical)
            );
        }
        assert!(msr.writes.is_empty());
        // Canonical kernel-half and the highest user address are accepted.
        let ok = TlsContext {
            fs_base: 0x0000_7FFF_FFFF_F000,
            gs_base: 0xFFFF_8000_0000_0000,
        };
        switch_in(&mut msr, &ok).unwrap();
    }

    #[test]
    fn free_scrubs_rejects_stale_and_double_free() {
        let mut m = Mem::new();
        let mut a = TlsArena::<2>::new(BASE, LEN).unwrap();
        let h = a.alloc(1, &tpl(), &IMG, &mut m).unwrap();
        let b = a.block(h).unwrap();
        a.free(h, &mut m).unwrap();
        assert!(
            m.read(b.start, b.total as usize).iter().all(|&x| x == 0),
            "scrubbed"
        );
        assert_eq!(a.free(h, &mut m), Err(TlsError::Stale), "double free");
        assert_eq!(a.block(h), Err(TlsError::Stale));
        assert_eq!(a.context_for(h), Err(TlsError::Stale));
        // The slot is reused with a new generation: the old handle stays dead.
        let h2 = a.alloc(2, &tpl(), &IMG, &mut m).unwrap();
        assert_ne!(h, h2);
        assert_eq!(a.block(h), Err(TlsError::Stale));
        assert_eq!(a.free(h, &mut m), Err(TlsError::Stale));
        assert!(a.block(h2).is_ok());
    }

    #[test]
    fn bad_handles_and_duplicate_owner() {
        let mut m = Mem::new();
        let mut a = TlsArena::<2>::new(BASE, LEN).unwrap();
        let bogus = TlsHandle {
            index: 99,
            generation: 0,
        };
        assert_eq!(a.block(bogus), Err(TlsError::Stale));
        assert_eq!(a.free(bogus, &mut m), Err(TlsError::Stale));
        a.alloc(1, &tpl(), &IMG, &mut m).unwrap();
        assert_eq!(a.alloc(1, &tpl(), &IMG, &mut m), Err(TlsError::OwnerExists));
        assert_eq!(
            a.alloc(2, &tpl(), &IMG[..4], &mut m),
            Err(TlsError::ImageSize)
        );
        assert_eq!(a.live(), 1);
    }

    #[test]
    fn exhaustion_slots_space_and_reuse_with_no_leak() {
        let mut m = Mem::new();
        // Slot exhaustion.
        let mut a = TlsArena::<2>::new(BASE, LEN).unwrap();
        let h1 = a.alloc(1, &tpl(), &IMG, &mut m).unwrap();
        let h2 = a.alloc(2, &tpl(), &IMG, &mut m).unwrap();
        assert_eq!(a.alloc(3, &tpl(), &IMG, &mut m), Err(TlsError::NoSlots));
        // Space exhaustion: region fits exactly two 48-byte blocks.
        let mut small = TlsArena::<8>::new(BASE, 96).unwrap();
        let s1 = small.alloc(1, &tpl(), &IMG, &mut m).unwrap();
        let s2 = small.alloc(2, &tpl(), &IMG, &mut m).unwrap();
        assert_eq!(small.alloc(3, &tpl(), &IMG, &mut m), Err(TlsError::NoSpace));
        assert_eq!(small.used_bytes(), 96);
        // Free the first: its gap is reused (first fit), nothing leaks.
        let at = small.block(s1).unwrap().start;
        small.free(s1, &mut m).unwrap();
        let s3 = small.alloc(3, &tpl(), &IMG, &mut m).unwrap();
        assert_eq!(small.block(s3).unwrap().start, at);
        small.free(s2, &mut m).unwrap();
        small.free(s3, &mut m).unwrap();
        assert_eq!((small.live(), small.used_bytes()), (0, 0));
        // The whole region is available again as one block.
        let big = TlsTemplate::new(0, 80, 16).unwrap(); // 80 + 16 = 96
        small.alloc(9, &big, &[], &mut m).unwrap();
        // The first arena also drains.
        a.free(h1, &mut m).unwrap();
        a.free(h2, &mut m).unwrap();
        assert_eq!((a.live(), a.used_bytes()), (0, 0));
        a.check_invariants().unwrap();
    }

    #[test]
    fn free_owner_is_thread_destruction_cleanup() {
        let mut m = Mem::new();
        let mut a = TlsArena::<4>::new(BASE, LEN).unwrap();
        a.alloc(5, &tpl(), &IMG, &mut m).unwrap();
        assert_eq!(a.free_owner(6, &mut m), Ok(false));
        assert_eq!(a.free_owner(5, &mut m), Ok(true));
        assert_eq!(a.free_owner(5, &mut m), Ok(false));
        assert_eq!(a.live(), 0);
    }

    #[test]
    fn memory_failure_rolls_back_and_scrubs() {
        let mut a = TlsArena::<4>::new(BASE, LEN).unwrap();
        for fail_after in 0..4 {
            let mut m = Mem::new();
            m.fail_after = Some(fail_after);
            assert_eq!(a.alloc(1, &tpl(), &IMG, &mut m), Err(TlsError::Memory));
            assert_eq!(
                a.live(),
                0,
                "nothing stays allocated (fail_after={fail_after})"
            );
        }
        // A scrub failure on free keeps the block (fail closed) and it can be retried.
        let mut m = Mem::new();
        let h = a.alloc(1, &tpl(), &IMG, &mut m).unwrap();
        m.fail_after = Some(m.ops);
        assert_eq!(a.free(h, &mut m), Err(TlsError::Memory));
        assert_eq!(a.live(), 1);
        m.fail_after = None;
        a.free(h, &mut m).unwrap();
        assert_eq!(a.live(), 0);
    }

    #[test]
    fn region_validation() {
        assert_eq!(TlsArena::<1>::new(BASE, 0).err(), Some(TlsError::BadRegion));
        assert_eq!(
            TlsArena::<1>::new(u64::MAX - 4, 100).err(),
            Some(TlsError::BadRegion)
        );
        assert_eq!(
            TlsArena::<1>::new(0x0000_7FFF_FFFF_F000, 0x2000).err(),
            Some(TlsError::BadRegion),
            "crosses into the non-canonical hole"
        );
        assert!(TlsArena::<1>::new(0xFFFF_8000_0000_0000, 0x1000).is_ok());
    }

    #[test]
    fn many_cycles_keep_invariants() {
        let mut m = Mem::new();
        let mut a = TlsArena::<8>::new(BASE, LEN).unwrap();
        let mut live: std::vec::Vec<TlsHandle> = vec![];
        let mut seed = 0x1234_5678_9abc_def1u64;
        for step in 0..2000u64 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            if (seed >> 33) % 2 == 0 || live.is_empty() {
                let t = TlsTemplate::new(0, (seed >> 40) % 200, 1 << ((seed >> 20) % 7)).unwrap();
                if let Ok(h) = a.alloc(step + 1, &t, &[], &mut m) {
                    live.push(h);
                }
            } else {
                let h = live.swap_remove(((seed >> 45) as usize) % live.len());
                a.free(h, &mut m).unwrap();
            }
            a.check_invariants().unwrap();
        }
        for h in live {
            a.free(h, &mut m).unwrap();
        }
        assert_eq!((a.live(), a.used_bytes()), (0, 0));
    }
}
