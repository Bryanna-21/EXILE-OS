//! Capability handles, rights and the per-space handle table.
//!
//! A handle is an untrusted 32-bit value `generation << 16 | index`. Whether it names a live capability
//! is decided only by the owning table. Generation 0 is never valid; a slot whose generation would
//! wrap is retired instead of reused, so a stale handle can never become valid again.

pub const MAX_HANDLES: usize = 32;

/// Rights bit set (Batch 2 `Rights`). Bits above `VALID_MASK` are rejected, never ignored.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rights(pub u32);

impl Rights {
    pub const NONE: Rights = Rights(0);
    pub const READ: Rights = Rights(1 << 0);
    pub const WRITE: Rights = Rights(1 << 1);
    pub const EXECUTE: Rights = Rights(1 << 2);
    pub const DUPLICATE: Rights = Rights(1 << 3);
    pub const TRANSFER: Rights = Rights(1 << 4);
    pub const WAIT: Rights = Rights(1 << 5);
    pub const SIGNAL: Rights = Rights(1 << 6);
    pub const ADMIN: Rights = Rights(1 << 7);
    pub const INSPECT: Rights = Rights(1 << 8);
    pub const VALID_MASK: u32 = 0x1FF;
    /// Rights given to both ends of a new channel.
    pub const CHANNEL_DEFAULT: Rights = Rights(
        Self::READ.0
            | Self::WRITE.0
            | Self::DUPLICATE.0
            | Self::TRANSFER.0
            | Self::WAIT.0
            | Self::ADMIN.0
            | Self::INSPECT.0,
    );

    pub const fn from_raw(bits: u32) -> Option<Rights> {
        if bits & !Self::VALID_MASK != 0 {
            None
        } else {
            Some(Rights(bits))
        }
    }
    pub const fn contains(self, need: Rights) -> bool {
        need.0 & !self.0 == 0
    }
    pub const fn union(self, o: Rights) -> Rights {
        Rights(self.0 | o.0)
    }
}

/// Kernel object kinds (Batch 2 `ObjectType`). Only `Channel` is implemented here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ObjectType {
    Channel = 1,
    MemoryObject = 2,
    Notification = 3,
    Cancellation = 4,
    Process = 5,
    Thread = 6,
    Timer = 7,
    ServiceEndpoint = 8,
}

impl ObjectType {
    pub const fn from_raw(v: u8) -> Option<ObjectType> {
        Some(match v {
            1 => ObjectType::Channel,
            2 => ObjectType::MemoryObject,
            3 => ObjectType::Notification,
            4 => ObjectType::Cancellation,
            5 => ObjectType::Process,
            6 => ObjectType::Thread,
            7 => ObjectType::Timer,
            8 => ObjectType::ServiceEndpoint,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Handle(pub u32);

impl Handle {
    pub const INVALID: Handle = Handle(0);
    pub const fn make(index: u16, generation: u16) -> Handle {
        Handle(((generation as u32) << 16) | index as u32)
    }
    pub const fn index(self) -> usize {
        (self.0 & 0xFFFF) as usize
    }
    pub const fn generation(self) -> u16 {
        (self.0 >> 16) as u16
    }
}

/// What a handle refers to. `obj` / `obj_gen` name a kernel object slot and its generation, so a
/// capability to a destroyed object is stale even if the slot is later reused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cap {
    pub obj: u8,
    pub obj_gen: u16,
    pub otype: ObjectType,
    pub rights: Rights,
}

#[derive(Clone, Copy)]
struct Slot {
    generation: u16,
    retired: bool,
    cap: Option<Cap>,
}

pub struct CapTable {
    slots: [Slot; MAX_HANDLES],
}

impl CapTable {
    pub const fn new() -> Self {
        CapTable {
            slots: [Slot {
                generation: 1,
                retired: false,
                cap: None,
            }; MAX_HANDLES],
        }
    }

    pub fn get(&self, h: Handle) -> Option<&Cap> {
        let s = self.slots.get(h.index())?;
        if h.generation() == 0 || s.generation != h.generation() {
            return None;
        }
        s.cap.as_ref()
    }

    pub fn free_slots(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| s.cap.is_none() && !s.retired)
            .count()
    }

    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.cap.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `None` when the table is full (quota).
    pub fn insert(&mut self, cap: Cap) -> Option<Handle> {
        let i = self
            .slots
            .iter()
            .position(|s| s.cap.is_none() && !s.retired)?;
        self.slots[i].cap = Some(cap);
        Some(Handle::make(i as u16, self.slots[i].generation))
    }

    pub fn remove(&mut self, h: Handle) -> Option<Cap> {
        self.get(h)?;
        let s = &mut self.slots[h.index()];
        let cap = s.cap.take();
        match s.generation.checked_add(1) {
            Some(g) => s.generation = g,
            None => s.retired = true, // never reuse a slot whose generation would wrap to 0
        }
        cap
    }

    /// Remove every capability for which `keep` is false (generation bumped, so the handles go stale).
    pub fn retain<F: FnMut(&Cap) -> bool>(&mut self, mut keep: F) {
        for s in self.slots.iter_mut() {
            if let Some(c) = s.cap
                && !keep(&c)
            {
                s.cap = None;
                match s.generation.checked_add(1) {
                    Some(g) => s.generation = g,
                    None => s.retired = true,
                }
            }
        }
    }

    pub fn caps(&self) -> impl Iterator<Item = &Cap> {
        self.slots.iter().filter_map(|s| s.cap.as_ref())
    }

    /// Test hook: set a free slot's generation (to exercise retirement). Not part of the kernel ABI.
    #[doc(hidden)]
    pub fn force_generation(&mut self, index: usize, g: u16) {
        self.slots[index].generation = g;
    }
}

impl Default for CapTable {
    fn default() -> Self {
        Self::new()
    }
}
