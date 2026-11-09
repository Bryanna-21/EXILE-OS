//! Channel objects, bounded queues, capability transfer and object lifetime.
//!
//! This module is pure kernel state: it never touches user memory and never blocks. The syscall layer
//! (`sys`) copies bytes in and out and calls into here under one lock.
//!
//! Lifetime rule: a channel end lives exactly while it is REACHABLE, that is, named by a handle in some
//! space's table, or enclosed in a message queued on a reachable end. After anything that can make an
//! object unreachable (release, close, move-out, space exit) `gc` runs a mark and sweep, so reference
//! cycles between queued messages are collected and cannot leak. Explicit close (needs ADMIN) destroys an
//! end at once for every holder: that is revocation.

use crate::caps::{Cap, CapTable, Handle, ObjectType, Rights};
use crate::status;
use omni_kernel7::sched::{Sched, SpaceId, ThreadId};

pub const MAX_OBJECTS: usize = 32;
pub const QUEUE_DEPTH: usize = 4;
pub const MAX_MSG_BYTES: usize = 1024;
pub const WIRE_HEADER: usize = 32;
pub const MAX_DESC: usize = 8;
pub const MAX_SPACES: usize = 8;
pub const MAX_WAITERS: usize = 4;
pub const ABI_VERSION_WIRE: u16 = 0x0001;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Move = 1,
    Copy = 2,
}

#[derive(Clone, Copy, Debug)]
pub struct Desc {
    pub handle: Handle,
    pub otype: ObjectType,
    pub mode: Mode,
    pub rights: Rights,
}

#[derive(Clone, Copy)]
struct Inflight {
    obj: u8,
    obj_gen: u16,
    rights: Rights,
}

#[derive(Clone, Copy)]
pub struct Msg {
    pub len: usize,
    pub bytes: [u8; MAX_MSG_BYTES],
    pub op: u16,
    pub kind: u16,
    pub request_id: u64,
    ndesc: usize,
    descs: [Inflight; MAX_DESC],
}

impl Msg {
    const EMPTY: Msg = Msg {
        len: 0,
        bytes: [0; MAX_MSG_BYTES],
        op: 0,
        kind: 0,
        request_id: 0,
        ndesc: 0,
        descs: [Inflight {
            obj: 0,
            obj_gen: 0,
            rights: Rights::NONE,
        }; MAX_DESC],
    };
    pub fn ndesc(&self) -> usize {
        self.ndesc
    }
}

/// Header fields of a received message (the payload goes to the caller's buffer).
#[derive(Clone, Copy)]
pub struct Taken {
    pub len: usize,
    pub op: u16,
    pub kind: u16,
    pub request_id: u64,
    pub ndesc: usize,
    pub handles: [Handle; MAX_DESC],
}

struct Object {
    live: bool,
    retired: bool,
    generation: u16,
    peer: u8,
    peer_dead: bool,
    q: [Msg; QUEUE_DEPTH],
    head: usize,
    len: usize,
    waiters: [Option<ThreadId>; MAX_WAITERS],
}

impl Object {
    const NEW: Object = Object {
        live: false,
        retired: false,
        generation: 1,
        peer: 0,
        peer_dead: false,
        q: [Msg::EMPTY; QUEUE_DEPTH],
        head: 0,
        len: 0,
        waiters: [None; MAX_WAITERS],
    };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stats {
    pub queued_messages: u32,
    pub queued_bytes: u64,
    pub peer_dead: bool,
    pub users: u32,
}

const NO_TABLE: Option<(SpaceId, CapTable)> = None;

pub struct State {
    objects: [Object; MAX_OBJECTS],
    tables: [Option<(SpaceId, CapTable)>; MAX_SPACES],
}

impl State {
    pub const fn new() -> Self {
        State {
            objects: [Object::NEW; MAX_OBJECTS],
            tables: [NO_TABLE; MAX_SPACES],
        }
    }

    // ---- tables ------------------------------------------------------------------

    pub fn table(&self, space: SpaceId) -> Option<&CapTable> {
        self.tables
            .iter()
            .flatten()
            .find(|(s, _)| *s == space)
            .map(|(_, t)| t)
    }

    /// The table of `space`, created on first use. `None` when `MAX_SPACES` tables exist.
    pub fn table_mut(&mut self, space: SpaceId) -> Option<&mut CapTable> {
        let i = match self
            .tables
            .iter()
            .position(|t| matches!(t, Some((s, _)) if *s == space))
        {
            Some(i) => i,
            None => {
                let i = self.tables.iter().position(|t| t.is_none())?;
                self.tables[i] = Some((space, CapTable::new()));
                i
            }
        };
        self.tables[i].as_mut().map(|(_, t)| t)
    }

    /// Look a handle up. Order of checks (fixed, as in the Zig model): the handle must name a live
    /// capability to a live object, then the type must match, then the rights must suffice.
    pub fn resolve(
        &self,
        space: SpaceId,
        h: Handle,
        otype: ObjectType,
        need: Rights,
    ) -> Result<(usize, Cap), i64> {
        let cap = *self
            .table(space)
            .and_then(|t| t.get(h))
            .ok_or(status::INVALID_HANDLE)?;
        let o = &self.objects[cap.obj as usize];
        if !o.live || o.generation != cap.obj_gen {
            return Err(status::INVALID_HANDLE);
        }
        if cap.otype != otype {
            return Err(status::INVALID_HANDLE);
        }
        if !cap.rights.contains(need) {
            return Err(status::ACCESS_DENIED);
        }
        Ok((cap.obj as usize, cap))
    }

    // ---- objects -----------------------------------------------------------------

    /// Create a connected pair of ends in `space`'s table. All-or-nothing.
    pub fn create_pair(&mut self, space: SpaceId) -> Result<(Handle, Handle), i64> {
        if self
            .table_mut(space)
            .ok_or(status::QUOTA_EXCEEDED)?
            .free_slots()
            < 2
        {
            return Err(status::QUOTA_EXCEEDED);
        }
        let mut free =
            (0..MAX_OBJECTS).filter(|&i| !self.objects[i].live && !self.objects[i].retired);
        let (a, b) = match (free.next(), free.next()) {
            (Some(a), Some(b)) => (a, b),
            _ => return Err(status::QUOTA_EXCEEDED),
        };
        for (me, other) in [(a, b), (b, a)] {
            let o = &mut self.objects[me];
            o.live = true;
            o.peer = other as u8;
            o.peer_dead = false;
            o.head = 0;
            o.len = 0;
            o.waiters = [None; MAX_WAITERS];
        }
        let mk = |st: &State, i: usize| Cap {
            obj: i as u8,
            obj_gen: st.objects[i].generation,
            otype: ObjectType::Channel,
            rights: Rights::CHANNEL_DEFAULT,
        };
        let (ca, cb) = (mk(self, a), mk(self, b));
        let t = self.table_mut(space).ok_or(status::INTERNAL)?;
        let ha = t.insert(ca).ok_or(status::INTERNAL)?;
        let hb = t.insert(cb).ok_or(status::INTERNAL)?;
        Ok((ha, hb))
    }

    pub fn peer_of(&self, oi: usize) -> Option<usize> {
        let o = &self.objects[oi];
        let p = o.peer as usize;
        (o.live && self.objects[p].live && !o.peer_dead).then_some(p)
    }

    pub fn is_peer_dead(&self, oi: usize) -> bool {
        self.objects[oi].peer_dead
    }

    pub fn queue_len(&self, oi: usize) -> usize {
        self.objects[oi].len
    }

    pub fn live_objects(&self) -> usize {
        self.objects.iter().filter(|o| o.live).count()
    }

    pub fn stats(&self, oi: usize) -> Stats {
        let o = &self.objects[oi];
        let mut bytes = 0u64;
        for k in 0..o.len {
            bytes += o.q[(o.head + k) % QUEUE_DEPTH].len as u64;
        }
        let users = self
            .tables
            .iter()
            .flatten()
            .flat_map(|(_, t)| t.caps())
            .filter(|c| c.obj as usize == oi && c.obj_gen == o.generation)
            .count() as u32;
        Stats {
            queued_messages: o.len as u32,
            queued_bytes: bytes,
            peer_dead: o.peer_dead,
            users,
        }
    }

    // ---- send / receive ------------------------------------------------------------

    /// Validate enclosed capabilities against the sender's table. Changes nothing.
    pub fn validate_descs(&self, space: SpaceId, descs: &[Desc], dst: usize) -> Result<(), i64> {
        for (i, d) in descs.iter().enumerate() {
            if descs[i + 1..].iter().any(|o| o.handle == d.handle) {
                return Err(status::INVALID_ARGUMENT); // the same handle twice
            }
        }
        for d in descs {
            let need = match d.mode {
                Mode::Move => Rights::TRANSFER,
                Mode::Copy => Rights::TRANSFER.union(Rights::DUPLICATE),
            };
            let (obj, cap) = self.resolve(space, d.handle, d.otype, need)?;
            if obj == dst {
                return Err(status::INVALID_ARGUMENT); // a channel end inside its own queue
            }
            match d.mode {
                Mode::Move => {
                    if d.rights != cap.rights {
                        return Err(status::INVALID_ARGUMENT); // a move cannot attenuate
                    }
                }
                Mode::Copy => {
                    if !d.rights.contains(Rights::TRANSFER) {
                        return Err(status::INVALID_ARGUMENT);
                    }
                    if !cap.rights.contains(d.rights) {
                        return Err(status::ACCESS_DENIED); // escalation attempt
                    }
                }
            }
        }
        Ok(())
    }

    /// Enqueue a validated message on `dst`. Caller must have run `validate_descs` under the same lock
    /// and checked the queue has room. Moves leave the sender's table; copies do not.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_send(
        &mut self,
        space: SpaceId,
        dst: usize,
        bytes: &[u8],
        op: u16,
        kind: u16,
        request_id: u64,
        descs: &[Desc],
    ) -> Result<(), i64> {
        if self.objects[dst].len >= QUEUE_DEPTH
            || bytes.len() > MAX_MSG_BYTES
            || descs.len() > MAX_DESC
        {
            return Err(status::INTERNAL);
        }
        // Resolve first (nothing is changed yet), then mutate. The message is built in place in the queue
        // slot: a `Msg` is over a kilobyte and thread stacks are small, so it is never copied by value.
        let mut inflight = [Inflight {
            obj: 0,
            obj_gen: 0,
            rights: Rights::NONE,
        }; MAX_DESC];
        for (i, d) in descs.iter().enumerate() {
            let (obj, cap) = self.resolve(space, d.handle, d.otype, Rights::NONE)?;
            let rights = match d.mode {
                Mode::Move => cap.rights,
                Mode::Copy => d.rights,
            };
            inflight[i] = Inflight {
                obj: obj as u8,
                obj_gen: cap.obj_gen,
                rights,
            };
        }
        for d in descs.iter().filter(|d| d.mode == Mode::Move) {
            self.table_mut(space)
                .and_then(|t| t.remove(d.handle))
                .ok_or(status::INTERNAL)?;
        }
        let o = &mut self.objects[dst];
        let tail = (o.head + o.len) % QUEUE_DEPTH;
        let m = &mut o.q[tail];
        m.len = bytes.len();
        m.bytes[..bytes.len()].copy_from_slice(bytes);
        m.op = op;
        m.kind = kind;
        m.request_id = request_id;
        m.ndesc = descs.len();
        m.descs = inflight;
        o.len += 1;
        Ok(())
    }

    /// Remove the oldest message of `oi`, copy its bytes into `payload` and install its capabilities in
    /// `space`'s table. `Ok(None)` when the queue is empty. All-or-nothing: if the table lacks room the
    /// message stays queued.
    pub fn take(
        &mut self,
        space: SpaceId,
        oi: usize,
        payload: &mut [u8; MAX_MSG_BYTES],
    ) -> Result<Option<Taken>, i64> {
        let o = &self.objects[oi];
        if o.len == 0 {
            return Ok(None);
        }
        let head = o.head;
        let n = o.q[head].ndesc;
        if self.table(space).map_or(0, |t| t.free_slots()) < n {
            return Err(status::QUOTA_EXCEEDED);
        }
        let mut t = Taken {
            len: o.q[head].len,
            op: o.q[head].op,
            kind: o.q[head].kind,
            request_id: o.q[head].request_id,
            ndesc: n,
            handles: [Handle::INVALID; MAX_DESC],
        };
        payload[..t.len].copy_from_slice(&o.q[head].bytes[..t.len]);
        let descs = o.q[head].descs;
        let o = &mut self.objects[oi];
        o.head = (o.head + 1) % QUEUE_DEPTH;
        o.len -= 1;
        for (i, d) in descs[..n].iter().enumerate() {
            let live = self.objects[d.obj as usize].live
                && self.objects[d.obj as usize].generation == d.obj_gen;
            if live && let Some(tb) = self.table_mut(space) {
                t.handles[i] = tb
                    .insert(Cap {
                        obj: d.obj,
                        obj_gen: d.obj_gen,
                        otype: ObjectType::Channel,
                        rights: d.rights,
                    })
                    .unwrap_or(Handle::INVALID);
            }
        }
        Ok(Some(t))
    }

    // ---- waiting -------------------------------------------------------------------

    /// Register `id` as a blocked receiver of `oi`. Entries of threads that no longer exist are
    /// scrubbed first, so a dead thread can never pin a waiter slot.
    pub fn add_waiter(&mut self, oi: usize, id: ThreadId, sched: &Sched) -> bool {
        let w = &mut self.objects[oi].waiters;
        for slot in w.iter_mut() {
            if let Some(t) = *slot
                && sched.space_of(t).is_none()
            {
                *slot = None;
            }
        }
        match w.iter_mut().find(|s| s.is_none()) {
            Some(s) => {
                *s = Some(id);
                true
            }
            None => false,
        }
    }

    pub fn remove_waiter(&mut self, oi: usize, id: ThreadId) {
        for s in self.objects[oi].waiters.iter_mut() {
            if *s == Some(id) {
                *s = None;
            }
        }
    }

    pub fn wake_one(&mut self, oi: usize, sched: &Sched) {
        for s in self.objects[oi].waiters.iter_mut() {
            if let Some(id) = s.take()
                && sched.wake(id).is_ok()
            {
                return;
            }
        }
    }

    fn wake_all(&mut self, oi: usize, sched: &Sched, cancel: bool) {
        for s in self.objects[oi].waiters.iter_mut() {
            if let Some(id) = s.take() {
                let _ = if cancel {
                    sched.cancel(id)
                } else {
                    sched.wake(id)
                };
            }
        }
    }

    // ---- destruction and collection ----------------------------------------------

    fn destroy_one(&mut self, i: usize, sched: &Sched) {
        if !self.objects[i].live {
            return;
        }
        self.wake_all(i, sched, true);
        let peer = self.objects[i].peer as usize;
        let o = &mut self.objects[i];
        o.live = false;
        o.head = 0;
        o.len = 0;
        match o.generation.checked_add(1) {
            Some(g) => o.generation = g,
            None => o.retired = true,
        }
        if self.objects[peer].live {
            self.objects[peer].peer_dead = true;
            self.wake_all(peer, sched, false); // receivers re-check and see PEER_DEAD
        }
    }

    /// Destroy end `oi` now for every holder (revocation), then collect what became unreachable.
    pub fn close(&mut self, oi: usize, sched: &Sched) {
        self.destroy_one(oi, sched);
        self.gc(sched);
    }

    /// Mark and sweep. Roots are all handle tables; edges are capabilities enclosed in queued messages.
    pub fn gc(&mut self, sched: &Sched) {
        let mut mark = [false; MAX_OBJECTS];
        let mut stack = [0u8; MAX_OBJECTS];
        let mut sp = 0;
        for (_, t) in self.tables.iter().flatten() {
            for c in t.caps() {
                let i = c.obj as usize;
                let o = &self.objects[i];
                if o.live && o.generation == c.obj_gen && !mark[i] {
                    mark[i] = true;
                    stack[sp] = i as u8;
                    sp += 1;
                }
            }
        }
        while sp > 0 {
            sp -= 1;
            let o = &self.objects[stack[sp] as usize];
            for k in 0..o.len {
                let m = &o.q[(o.head + k) % QUEUE_DEPTH];
                for d in &m.descs[..m.ndesc] {
                    let j = d.obj as usize;
                    let t = &self.objects[j];
                    if t.live && t.generation == d.obj_gen && !mark[j] {
                        mark[j] = true;
                        stack[sp] = j as u8;
                        sp += 1;
                    }
                }
            }
        }
        for (i, marked) in mark.iter().enumerate() {
            if !marked {
                self.destroy_one(i, sched);
            }
        }
        // Capabilities to destroyed objects are dead weight: free their table slots (the handles
        // were already invalid, so this changes no result, only quota).
        let objs = &self.objects;
        for (_, t) in self.tables.iter_mut().flatten() {
            t.retain(|c| {
                let o = &objs[c.obj as usize];
                o.live && o.generation == c.obj_gen
            });
        }
    }

    /// Release every capability of an exiting space and collect. Idempotent.
    pub fn cleanup_space(&mut self, space: SpaceId, sched: &Sched) {
        for t in self.tables.iter_mut() {
            if matches!(t, Some((s, _)) if *s == space) {
                *t = None;
            }
        }
        self.gc(sched);
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}
