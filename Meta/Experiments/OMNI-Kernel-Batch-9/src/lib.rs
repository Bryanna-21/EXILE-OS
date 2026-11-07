//! OMNI Batch 9 integration glue (`no_std`, hosted-tested only).
//!
//! Batch 7 validates user addresses against a *region model* (`uaccess::Spaces`) and then
//! asks a `UserBacking` to move the bytes. Batch 8 builds real x86-64 page tables. Nothing in
//! Batches 7 or 8 connects the two. `PagedBacking` is that connection: a `UserBacking` that
//! only touches a byte if the Batch 8 page-table walk for the caller's address space says the
//! page is present, user-accessible and (for writes) writable. The two checks are
//! independent, so a user access succeeds only when both the region model and the page tables
//! agree (defence in depth, and a way to detect disagreement).
//!
//! Limits: this runs over a `PhysMem` implementation, which in tests is a simulated memory.
//! It is not hardware page-table enforcement and says nothing about what a CPU would do.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
// `is_multiple_of` needs a newer rustc than this crate's declared minimum (1.88).
#![allow(clippy::manual_is_multiple_of)]

use omni_kernel7::sync::SpinLock;
use omni_kernel7::uaccess::{USER_TOP, UserBacking};
use omni8::paging::{self, PHYS_LIMIT, PhysMem};

/// Address spaces that can be bound at once (matches Batch 7 `MAX_SPACES`).
pub const MAX_BOUND: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BindError {
    /// Root is zero, misaligned or beyond the physical address limit.
    BadRoot,
    /// The space already has a root.
    Exists,
    /// No free binding slot.
    Full,
}

struct Inner<M> {
    mem: M,
    roots: [Option<(u16, u64)>; MAX_BOUND],
}

/// `UserBacking` over Batch 8 page tables.
pub struct PagedBacking<M: PhysMem + Send> {
    inner: SpinLock<Inner<M>>,
}

impl<M: PhysMem + Send> PagedBacking<M> {
    pub const fn new(mem: M) -> Self {
        PagedBacking {
            inner: SpinLock::new(Inner {
                mem,
                roots: [None; MAX_BOUND],
            }),
        }
    }

    /// Associate a Batch 7 space id with the root of its page tables.
    pub fn bind(&self, space: u16, root: u64) -> Result<(), BindError> {
        if root == 0 || root % paging::PAGE_SIZE != 0 || root >= PHYS_LIMIT {
            return Err(BindError::BadRoot);
        }
        let mut g = self.inner.lock();
        if g.roots.iter().flatten().any(|&(s, _)| s == space) {
            return Err(BindError::Exists);
        }
        match g.roots.iter_mut().find(|s| s.is_none()) {
            Some(slot) => {
                *slot = Some((space, root));
                Ok(())
            }
            None => Err(BindError::Full),
        }
    }

    /// Remove a binding. After this every access for the space is refused.
    pub fn unbind(&self, space: u16) -> bool {
        let mut g = self.inner.lock();
        for slot in g.roots.iter_mut() {
            if matches!(slot, Some((s, _)) if *s == space) {
                *slot = None;
                return true;
            }
        }
        false
    }

    /// Run `f` with exclusive access to the physical memory (used to build page tables).
    pub fn with_mem<R>(&self, f: impl FnOnce(&mut M) -> R) -> R {
        f(&mut self.inner.lock().mem)
    }

    fn access(&self, space: u16, addr: u64, len: usize, write: bool, data: Data<'_>) -> bool {
        let mut g = self.inner.lock();
        let Some(root) = g
            .roots
            .iter()
            .flatten()
            .find(|&&(s, _)| s == space)
            .map(|&(_, r)| r)
        else {
            return false;
        };
        if len == 0 {
            return true;
        }
        let Some(end) = addr.checked_add(len as u64) else {
            return false;
        };
        if end > USER_TOP {
            return false;
        }
        // Pass 1: every page in the range must be present, user-accessible and (for a write)
        // writable. Nothing is touched unless the whole range passes.
        let mut cur = addr;
        while cur < end {
            let Some(t) = paging::translate(&g.mem, root, cur) else {
                return false;
            };
            if !t.user || (write && !t.write) {
                return false;
            }
            cur = (cur - (cur % t.size)) + t.size;
        }
        // Pass 2: move the bytes.
        let mut cur = addr;
        let mut done = 0usize;
        match data {
            Data::Read(out) => {
                while cur < end {
                    let t = match paging::translate(&g.mem, root, cur) {
                        Some(t) => t,
                        None => return false,
                    };
                    let n = ((t.size - (cur % t.size)).min(end - cur)) as usize;
                    read_phys(&g.mem, t.phys, &mut out[done..done + n]);
                    done += n;
                    cur += n as u64;
                }
            }
            Data::Write(src) => {
                while cur < end {
                    let t = match paging::translate(&g.mem, root, cur) {
                        Some(t) => t,
                        None => return false,
                    };
                    let n = ((t.size - (cur % t.size)).min(end - cur)) as usize;
                    write_phys(&mut g.mem, t.phys, &src[done..done + n]);
                    done += n;
                    cur += n as u64;
                }
            }
        }
        true
    }
}

enum Data<'a> {
    Read(&'a mut [u8]),
    Write(&'a [u8]),
}

fn read_phys<M: PhysMem>(mem: &M, phys: u64, out: &mut [u8]) {
    for (i, b) in out.iter_mut().enumerate() {
        let a = phys + i as u64;
        *b = mem.read_u64(a & !7).to_le_bytes()[(a & 7) as usize];
    }
}

fn write_phys<M: PhysMem>(mem: &mut M, phys: u64, data: &[u8]) {
    for (i, &b) in data.iter().enumerate() {
        let a = phys + i as u64;
        let mut w = mem.read_u64(a & !7).to_le_bytes();
        w[(a & 7) as usize] = b;
        mem.write_u64(a & !7, u64::from_le_bytes(w));
    }
}

impl<M: PhysMem + Send> UserBacking for PagedBacking<M> {
    fn read(&self, space: u16, addr: u64, out: &mut [u8]) -> bool {
        let len = out.len();
        self.access(space, addr, len, false, Data::Read(out))
    }

    fn write(&self, space: u16, addr: u64, data: &[u8]) -> bool {
        self.access(space, addr, data.len(), true, Data::Write(data))
    }
}
