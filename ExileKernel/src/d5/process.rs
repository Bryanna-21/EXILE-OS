//! Process management (Mission D, requirements 12, 13, 18, 19). SPDX-License-Identifier: Apache-2.0
//!
//! A process is a kernel object that OWNS an address space, a set of threads (scheduler
//! handles), per-thread TLS keys, resource limits and an exit status. It is not a renamed
//! thread: threads belong to at most one process, die with it, and a process outlives them as a
//! zombie until its parent collects the status.
//!
//! # Lifecycle
//! ```text
//! Creating --add thread--> Running --exit()/kill event/fault/last thread--> Exiting
//! Exiting --every thread reaped, address space released--> Zombie --parent wait--> (slot freed)
//! ```
//! * Handles are `(slot, generation)`; a stale handle is rejected, a slot whose generation would
//!   wrap is retired. Pids are 64-bit and never reused.
//! * `exit` is idempotent and "exit group": every thread except the caller's is killed through
//!   [`ProcessEnv::kill_thread`] (which also releases the thread's locks and waits). The calling
//!   thread of a self-exit cannot be killed while running; the glue calls [`thread_exited`]
//!   after `terminate_current`.
//! * Teardown is incremental. A terminated thread can only be reclaimed after its context switch
//!   completed (`StillOnCpu` otherwise); [`ProcessTable::collect`] retries, so a process whose
//!   thread is still on the CPU simply stays `Exiting`. The address space is destroyed (all frames
//!   and tables returned) only when no thread is left AND it is not the active address space;
//!   run `collect` from a kernel thread or the idle thread.
//! * Children of an exiting process are re-parented to "no parent"; a parentless zombie is
//!   released immediately. The process created first with no parent is the root.
//!
//! # Events (signal equivalent)
//! A process has a 32-bit pending-event set and a mask. `EVENT_KILL` is unmaskable and is acted
//! on by [`ProcessTable::deliver`] at a safe point (syscall return, preemption); other events
//! are queued for the process to consume. There are no signal handlers or user-mode upcalls.
//!
//! # Limits and accounting
//! [`Limits`]: threads, mapped pages, children. They are checked BEFORE the resource is taken.
//! [`Usage`] reports live threads, mapped/table pages and children.
//!
//! Single CPU: no internal locking; callers serialise (see sched/dispatch locking rules).
#![allow(dead_code)]

use crate::address_space::{AddressSpace, Error as SpaceError, FrameAlloc, PAGE, Perms, PhysMem};
use crate::elf::{self, LoadedImage};
use crate::frame::TrapFrame;
use crate::thread::Handle;
use crate::tls::TlsContext;
use crate::usermode::{self, UserError};

pub const MAX_THREADS_PER_PROCESS: usize = 8;

pub const EVENT_KILL: u32 = 1 << 0;
pub const EVENT_CHILD_EXITED: u32 = 1 << 1;
pub const EVENT_FAULT: u32 = 1 << 2;
/// First bit available for application-defined events.
pub const EVENT_USER_FIRST: u32 = 1 << 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pid(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcHandle {
    index: u32,
    generation: u32,
}

impl ProcHandle {
    pub const fn index(self) -> usize {
        self.index as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcState {
    Creating,
    Running,
    Exiting,
    Zombie,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitStatus {
    Code(i32),
    /// Terminated by the kill event.
    Killed,
    /// Terminated by an unrecoverable user fault (`usermode::FaultReason` as a small code).
    Fault(u32),
}

impl ExitStatus {
    fn thread_status(self) -> i32 {
        match self {
            ExitStatus::Code(c) => c,
            ExitStatus::Killed => -9,
            ExitStatus::Fault(_) => -11,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    Threads,
    Pages,
    Children,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_threads: usize,
    /// Maximum mapped user pages (address-space data pages; page-table pages are separate).
    pub max_pages: u64,
    pub max_children: usize,
}

impl Limits {
    pub const DEFAULT: Limits = Limits {
        max_threads: MAX_THREADS_PER_PROCESS,
        max_pages: 4096,
        max_children: 16,
    };

    fn validate(&self) -> Result<(), Error> {
        if self.max_threads == 0
            || self.max_threads > MAX_THREADS_PER_PROCESS
            || self.max_pages == 0
        {
            return Err(Error::BadLimits);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub threads: usize,
    pub mapped_pages: u64,
    pub table_pages: u64,
    pub children: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvError {
    /// The thread has terminated but its context switch has not completed.
    StillOnCpu,
    /// The thread no longer exists (already reclaimed).
    Gone,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Stale,
    NoSlots,
    BadLimits,
    BadState,
    /// A resource limit would be exceeded.
    Limit(LimitKind),
    NoSuchThread,
    ThreadOwned,
    NotChild,
    /// `wait` on a child that has not exited.
    NotExited,
    NoParentSlot,
    Space(SpaceError),
    Elf(elf::Error),
    User(UserError),
    Env(EnvError),
    /// The address space is the active one; switch away before destroying it.
    SpaceActive,
}

impl From<SpaceError> for Error {
    fn from(e: SpaceError) -> Self {
        Error::Space(e)
    }
}
impl From<elf::Error> for Error {
    fn from(e: elf::Error) -> Self {
        Error::Elf(e)
    }
}
impl From<UserError> for Error {
    fn from(e: UserError) -> Self {
        Error::User(e)
    }
}

/// Services the process layer needs from the kernel; implemented by the kernel glue (and by a
/// fake in the tests).
pub trait ProcessEnv {
    fn phys(&mut self) -> (&mut dyn PhysMem, &mut dyn FrameAlloc);
    fn current_thread(&self) -> Option<Handle>;
    /// CR3 value currently loaded, to refuse destroying the active address space.
    fn active_cr3(&self) -> u64;
    /// Terminates a thread that is not running (any other state) and drops its lock/wait state.
    fn kill_thread(&mut self, h: Handle, status: i32) -> Result<(), EnvError>;
    /// Reclaims a terminated thread (stack etc.).
    fn reap_thread(&mut self, h: Handle) -> Result<(), EnvError>;
    /// Releases the TLS block of the thread identified by `key`.
    fn free_tls(&mut self, key: u64) -> Result<(), EnvError>;
}

/// What the architecture layer needs when a thread of a process is switched in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwitchContext {
    pub cr3: u64,
    pub tls: TlsContext,
}

/// Everything needed to enter user mode for the first time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Launch {
    pub pid: Pid,
    pub cr3: u64,
    pub frame: TrapFrame,
    pub tls: TlsContext,
}

#[derive(Clone, Copy)]
struct ThreadRec {
    handle: Handle,
    /// Identifies the thread's TLS block; `None` once released.
    tls_key: Option<u64>,
    tls_ctx: TlsContext,
    launch: Option<TrapFrame>,
    /// Terminated (killed, or it exited itself): waiting to be reclaimed.
    dead: bool,
}

struct Process {
    pid: Pid,
    parent: Option<ProcHandle>,
    state: ProcState,
    exit: Option<ExitStatus>,
    space: Option<AddressSpace>,
    threads: [Option<ThreadRec>; MAX_THREADS_PER_PROCESS],
    limits: Limits,
    children: usize,
    events: u32,
    mask: u32,
}

struct Slot {
    generation: u32,
    retired: bool,
    proc: Option<Process>,
}

pub struct ProcessTable<const P: usize> {
    slots: [Slot; P],
    next_pid: u64,
    root: Option<ProcHandle>,
}

impl<const P: usize> Default for ProcessTable<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const P: usize> ProcessTable<P> {
    pub fn new() -> Self {
        Self {
            slots: core::array::from_fn(|_| Slot {
                generation: 0,
                retired: false,
                proc: None,
            }),
            next_pid: 1,
            root: None,
        }
    }

    pub fn live(&self) -> usize {
        self.slots.iter().filter(|s| s.proc.is_some()).count()
    }

    fn get(&self, h: ProcHandle) -> Result<&Process, Error> {
        let s = self.slots.get(h.index()).ok_or(Error::Stale)?;
        match &s.proc {
            Some(p) if s.generation == h.generation => Ok(p),
            _ => Err(Error::Stale),
        }
    }

    fn get_mut(&mut self, h: ProcHandle) -> Result<&mut Process, Error> {
        let s = self.slots.get_mut(h.index()).ok_or(Error::Stale)?;
        match &mut s.proc {
            Some(p) if s.generation == h.generation => Ok(p),
            _ => Err(Error::Stale),
        }
    }

    fn handle_at(&self, i: usize) -> Option<ProcHandle> {
        let s = self.slots.get(i)?;
        s.proc.as_ref().map(|_| ProcHandle {
            index: i as u32,
            generation: s.generation,
        })
    }

    pub fn pid(&self, h: ProcHandle) -> Result<Pid, Error> {
        Ok(self.get(h)?.pid)
    }
    pub fn state(&self, h: ProcHandle) -> Result<ProcState, Error> {
        Ok(self.get(h)?.state)
    }
    pub fn parent(&self, h: ProcHandle) -> Result<Option<ProcHandle>, Error> {
        Ok(self.get(h)?.parent)
    }
    pub fn exit_status(&self, h: ProcHandle) -> Result<Option<ExitStatus>, Error> {
        Ok(self.get(h)?.exit)
    }
    pub fn root(&self) -> Option<ProcHandle> {
        self.root
    }
    pub fn space(&self, h: ProcHandle) -> Result<Option<&AddressSpace>, Error> {
        Ok(self.get(h)?.space.as_ref())
    }
    pub fn threads(&self, h: ProcHandle) -> Result<impl Iterator<Item = Handle> + '_, Error> {
        Ok(self.get(h)?.threads.iter().flatten().map(|t| t.handle))
    }

    pub fn usage(&self, h: ProcHandle) -> Result<Usage, Error> {
        let p = self.get(h)?;
        Ok(Usage {
            threads: p.threads.iter().flatten().filter(|t| !t.dead).count(),
            mapped_pages: p.space.as_ref().map_or(0, |s| s.mapped_pages()),
            table_pages: p.space.as_ref().map_or(0, |s| s.table_pages()),
            children: p.children,
        })
    }

    /// Owning process of a scheduler thread, if it belongs to one.
    pub fn owner_of(&self, t: Handle) -> Option<ProcHandle> {
        (0..P).find_map(|i| {
            let h = self.handle_at(i)?;
            let p = self.get(h).ok()?;
            p.threads
                .iter()
                .flatten()
                .any(|r| r.handle == t)
                .then_some(h)
        })
    }

    // ---- creation ----------------------------------------------------------------------

    /// Registers a process that owns `space`. On error the space is handed back untouched in
    /// `Err((error, space))` so the caller can destroy it.
    pub fn create(
        &mut self,
        parent: Option<ProcHandle>,
        limits: Limits,
        space: AddressSpace,
    ) -> Result<ProcHandle, (Error, AddressSpace)> {
        if let Err(e) = limits.validate() {
            return Err((e, space));
        }
        match parent {
            Some(ph) => match self.get(ph) {
                Ok(pp) => {
                    if !matches!(pp.state, ProcState::Creating | ProcState::Running) {
                        return Err((Error::BadState, space));
                    }
                    if pp.children >= pp.limits.max_children {
                        return Err((Error::Limit(LimitKind::Children), space));
                    }
                }
                Err(e) => return Err((e, space)),
            },
            None => {
                if self.root.is_some() {
                    return Err((Error::NoParentSlot, space));
                }
            }
        }
        let Some(i) = self
            .slots
            .iter()
            .position(|s| s.proc.is_none() && !s.retired)
        else {
            return Err((Error::NoSlots, space));
        };
        let pid = Pid(self.next_pid);
        self.next_pid += 1;
        let slot = &mut self.slots[i];
        slot.proc = Some(Process {
            pid,
            parent,
            state: ProcState::Creating,
            exit: None,
            space: Some(space),
            threads: [None; MAX_THREADS_PER_PROCESS],
            limits,
            children: 0,
            events: 0,
            mask: !0,
        });
        let h = ProcHandle {
            index: i as u32,
            generation: slot.generation,
        };
        match parent {
            Some(ph) => {
                if let Ok(pp) = self.get_mut(ph) {
                    pp.children += 1;
                }
            }
            None => self.root = Some(h),
        }
        Ok(h)
    }

    /// Creates an address space, loads the ELF `image` into it and registers the process.
    /// All-or-nothing: on error every frame is returned.
    pub fn spawn_elf<E: ProcessEnv>(
        &mut self,
        env: &mut E,
        parent: Option<ProcHandle>,
        limits: Limits,
        kernel_pml4: u64,
        image: &[u8],
    ) -> Result<(ProcHandle, LoadedImage), Error> {
        limits.validate()?;
        let (mem, frames) = env.phys();
        let mut space = AddressSpace::new(mem, frames, kernel_pml4)?;
        let loaded = match elf::load(image, &mut space, mem, frames) {
            Ok(l) => l,
            Err(e) => {
                let _ = space.destroy(mem, frames);
                return Err(e.into());
            }
        };
        if space.mapped_pages() > limits.max_pages {
            let _ = space.destroy(mem, frames);
            return Err(Error::Limit(LimitKind::Pages));
        }
        match self.create(parent, limits, space) {
            Ok(h) => Ok((h, loaded)),
            Err((e, space)) => {
                let (mem, frames) = env.phys();
                let _ = space.destroy(mem, frames);
                Err(e)
            }
        }
    }

    /// Maps anonymous user memory for the process, charging the page limit first.
    pub fn map_anon<E: ProcessEnv>(
        &mut self,
        env: &mut E,
        h: ProcHandle,
        start: u64,
        len: u64,
        perms: Perms,
    ) -> Result<(), Error> {
        let p = self.get_mut(h)?;
        if matches!(p.state, ProcState::Exiting | ProcState::Zombie) {
            return Err(Error::BadState);
        }
        let space = p.space.as_mut().ok_or(Error::BadState)?;
        let pages = len
            .checked_add(PAGE - 1)
            .ok_or(Error::Space(SpaceError::OutOfUserRange))?
            / PAGE;
        let total = space
            .mapped_pages()
            .checked_add(pages)
            .ok_or(Error::Limit(LimitKind::Pages))?;
        if total > p.limits.max_pages {
            return Err(Error::Limit(LimitKind::Pages));
        }
        let (mem, frames) = env.phys();
        space.map_anon(mem, frames, start, len, perms)?;
        Ok(())
    }

    // ---- threads -----------------------------------------------------------------------

    /// Attaches a scheduler thread to the process. `tls_key` names its TLS block (if any).
    pub fn add_thread(
        &mut self,
        h: ProcHandle,
        thread: Handle,
        tls_key: Option<u64>,
        tls_ctx: TlsContext,
    ) -> Result<(), Error> {
        if self.owner_of(thread).is_some() {
            return Err(Error::ThreadOwned);
        }
        let p = self.get_mut(h)?;
        if !matches!(p.state, ProcState::Creating | ProcState::Running) {
            return Err(Error::BadState);
        }
        let live = p.threads.iter().flatten().filter(|t| !t.dead).count();
        if live >= p.limits.max_threads {
            return Err(Error::Limit(LimitKind::Threads));
        }
        let slot = p
            .threads
            .iter_mut()
            .find(|t| t.is_none())
            .ok_or(Error::Limit(LimitKind::Threads))?;
        *slot = Some(ThreadRec {
            handle: thread,
            tls_key,
            tls_ctx,
            launch: None,
            dead: false,
        });
        p.state = ProcState::Running;
        Ok(())
    }

    /// Records the first-entry user context of `thread`: entry point and stack from `img`, RSP at
    /// the 16-byte aligned initial stack pointer (argc = 0 there), RDI = `arg`. The frame is
    /// validated; a bad one never reaches `iretq`.
    pub fn prepare_launch(
        &mut self,
        h: ProcHandle,
        thread: Handle,
        img: &LoadedImage,
        arg: u64,
    ) -> Result<(), Error> {
        let mut f = usermode::build_user_frame(img.entry, img.initial_sp, arg)?;
        f.rsp = img.initial_sp;
        usermode::validate_return_frame(&f)?;
        let p = self.get_mut(h)?;
        let rec = p
            .threads
            .iter_mut()
            .flatten()
            .find(|r| r.handle == thread)
            .ok_or(Error::NoSuchThread)?;
        rec.launch = Some(f);
        Ok(())
    }

    /// First-entry data for `thread`, when it was prepared with [`prepare_launch`].
    pub fn launch_info(&self, thread: Handle) -> Option<Launch> {
        let h = self.owner_of(thread)?;
        let p = self.get(h).ok()?;
        let rec = p.threads.iter().flatten().find(|r| r.handle == thread)?;
        Some(Launch {
            pid: p.pid,
            cr3: p.space.as_ref()?.cr3_value(),
            frame: rec.launch?,
            tls: rec.tls_ctx,
        })
    }

    /// Address space and TLS to load when `thread` is switched in; `None` for kernel threads
    /// (they run on the kernel's own address space).
    pub fn switch_context_for(&self, thread: Handle) -> Option<SwitchContext> {
        let h = self.owner_of(thread)?;
        let p = self.get(h).ok()?;
        let rec = p.threads.iter().flatten().find(|r| r.handle == thread)?;
        Some(SwitchContext {
            cr3: p.space.as_ref()?.cr3_value(),
            tls: rec.tls_ctx,
        })
    }

    /// Stores the FS/GS state captured when `thread` was switched out.
    pub fn save_tls(&mut self, thread: Handle, ctx: TlsContext) -> Result<(), Error> {
        let h = self.owner_of(thread).ok_or(Error::NoSuchThread)?;
        let p = self.get_mut(h)?;
        let rec = p
            .threads
            .iter_mut()
            .flatten()
            .find(|r| r.handle == thread)
            .ok_or(Error::NoSuchThread)?;
        rec.tls_ctx = ctx;
        Ok(())
    }

    // ---- termination -------------------------------------------------------------------

    /// Begins (or continues) terminating the process with `status`. Idempotent: the first
    /// status wins. Kills every thread except the running one, then collects what it can.
    pub fn exit<E: ProcessEnv>(
        &mut self,
        env: &mut E,
        h: ProcHandle,
        status: ExitStatus,
    ) -> Result<(), Error> {
        let cur = env.current_thread();
        let p = self.get_mut(h)?;
        if p.state == ProcState::Zombie {
            return Ok(());
        }
        if p.state != ProcState::Exiting {
            p.state = ProcState::Exiting;
            p.exit = Some(status);
        }
        let code = p.exit.map_or(-1, ExitStatus::thread_status);
        for rec in p.threads.iter_mut().flatten() {
            if rec.dead || Some(rec.handle) == cur {
                continue;
            }
            match env.kill_thread(rec.handle, code) {
                Ok(()) | Err(EnvError::Gone) => rec.dead = true,
                Err(e) => return Err(Error::Env(e)),
            }
        }
        self.collect(env, h)
    }

    /// A thread of the process terminated by itself (after `terminate_current`). Releases its
    /// TLS; if it was the last live thread the process exits with the thread's status.
    pub fn thread_exited<E: ProcessEnv>(
        &mut self,
        env: &mut E,
        h: ProcHandle,
        thread: Handle,
        status: i32,
    ) -> Result<(), Error> {
        let p = self.get_mut(h)?;
        let rec = p
            .threads
            .iter_mut()
            .flatten()
            .find(|r| r.handle == thread)
            .ok_or(Error::NoSuchThread)?;
        rec.dead = true;
        let any_live = p.threads.iter().flatten().any(|t| !t.dead);
        if !any_live && p.state != ProcState::Exiting && p.state != ProcState::Zombie {
            return self.exit(env, h, ExitStatus::Code(status));
        }
        self.collect(env, h)
    }

    /// An unrecoverable user fault: terminate the whole process.
    pub fn fault_exit<E: ProcessEnv>(
        &mut self,
        env: &mut E,
        h: ProcHandle,
        reason: u32,
    ) -> Result<(), Error> {
        let p = self.get_mut(h)?;
        p.events |= EVENT_FAULT;
        self.exit(env, h, ExitStatus::Fault(reason))
    }

    /// Reclaims dead threads (and their TLS) and, once none is left and the address space is not
    /// active, releases the address space and turns the process into a zombie. Safe to call
    /// repeatedly; call it from the reaper (idle/kernel thread).
    pub fn collect<E: ProcessEnv>(&mut self, env: &mut E, h: ProcHandle) -> Result<(), Error> {
        let p = self.get_mut(h)?;
        for slot in p.threads.iter_mut() {
            let Some(rec) = slot else { continue };
            if !rec.dead {
                continue;
            }
            if let Some(key) = rec.tls_key {
                match env.free_tls(key) {
                    Ok(()) => rec.tls_key = None,
                    Err(e) => return Err(Error::Env(e)),
                }
            }
            match env.reap_thread(rec.handle) {
                Ok(()) | Err(EnvError::Gone) => *slot = None,
                Err(EnvError::StillOnCpu) => {}
                Err(e) => return Err(Error::Env(e)),
            }
        }
        if p.state != ProcState::Exiting || p.threads.iter().any(|t| t.is_some()) {
            return Ok(());
        }
        // No threads left: release the address space.
        if let Some(space) = &p.space {
            if env.active_cr3() == space.cr3_value() {
                return Err(Error::SpaceActive);
            }
        }
        if let Some(space) = p.space.take() {
            let (mem, frames) = env.phys();
            space.destroy(mem, frames)?;
        }
        self.finish(h)
    }

    fn finish(&mut self, h: ProcHandle) -> Result<(), Error> {
        // Re-parent children to "no parent".
        for i in 0..P {
            if let Some(c) = self.slots[i].proc.as_mut() {
                if c.parent == Some(h) {
                    c.parent = None;
                    if c.state == ProcState::Zombie {
                        // Nobody can wait for it any more.
                        self.slots[i].proc = None;
                        self.bump(i);
                    }
                }
            }
        }
        let p = self.get_mut(h)?;
        p.children = 0;
        p.state = ProcState::Zombie;
        let parent = p.parent;
        match parent {
            Some(ph) => {
                if let Ok(pp) = self.get_mut(ph) {
                    pp.events |= EVENT_CHILD_EXITED;
                }
            }
            None => {
                // Parentless zombies are released at once.
                self.slots[h.index()].proc = None;
                self.bump(h.index());
                if self.root == Some(h) {
                    self.root = None;
                }
            }
        }
        Ok(())
    }

    fn bump(&mut self, i: usize) {
        let s = &mut self.slots[i];
        match s.generation.checked_add(1) {
            Some(g) => s.generation = g,
            None => s.retired = true,
        }
    }

    /// Collects the exit status of a zombie child and frees its slot.
    pub fn wait_child(
        &mut self,
        parent: ProcHandle,
        child: ProcHandle,
    ) -> Result<(Pid, ExitStatus), Error> {
        self.get(parent)?;
        let c = self.get(child)?;
        if c.parent != Some(parent) {
            return Err(Error::NotChild);
        }
        if c.state != ProcState::Zombie {
            return Err(Error::NotExited);
        }
        let (pid, st) = (c.pid, c.exit.unwrap_or(ExitStatus::Code(0)));
        self.slots[child.index()].proc = None;
        self.bump(child.index());
        let pp = self.get_mut(parent)?;
        pp.children = pp.children.saturating_sub(1);
        Ok((pid, st))
    }

    /// Collects any zombie child.
    pub fn wait_any(&mut self, parent: ProcHandle) -> Result<Option<(Pid, ExitStatus)>, Error> {
        self.get(parent)?;
        for i in 0..P {
            let Some(c) = self.handle_at(i) else { continue };
            let cp = self.get(c)?;
            if cp.parent == Some(parent) && cp.state == ProcState::Zombie {
                return self.wait_child(parent, c).map(Some);
            }
        }
        Ok(None)
    }

    // ---- events ------------------------------------------------------------------------

    pub fn post_event(&mut self, h: ProcHandle, bits: u32) -> Result<(), Error> {
        let p = self.get_mut(h)?;
        if p.state == ProcState::Zombie {
            return Err(Error::BadState);
        }
        p.events |= bits;
        Ok(())
    }

    pub fn set_event_mask(&mut self, h: ProcHandle, mask: u32) -> Result<(), Error> {
        self.get_mut(h)?.mask = mask;
        Ok(())
    }

    pub fn pending_events(&self, h: ProcHandle) -> Result<u32, Error> {
        let p = self.get(h)?;
        Ok(p.events & (p.mask | EVENT_KILL))
    }

    /// Consumes and returns the deliverable (unmasked) events.
    pub fn take_events(&mut self, h: ProcHandle) -> Result<u32, Error> {
        let p = self.get_mut(h)?;
        let deliver = p.events & (p.mask | EVENT_KILL);
        p.events &= !deliver;
        Ok(deliver)
    }

    /// Safe-point check: a pending kill event terminates the process. Returns whether it did.
    pub fn deliver<E: ProcessEnv>(&mut self, env: &mut E, h: ProcHandle) -> Result<bool, Error> {
        let p = self.get_mut(h)?;
        if p.events & EVENT_KILL == 0 || p.state == ProcState::Zombie {
            return Ok(false);
        }
        p.events &= !EVENT_KILL;
        self.exit(env, h, ExitStatus::Killed)?;
        Ok(true)
    }
}

/// Loads the address space and FS/GS state of the thread being switched in: the process's own
/// CR3 (or `kernel_cr3` for kernel threads) and its TLS context. Call it from the switch-in hook
/// (`dispatch::install_switch_in_hook`), after the context switch bookkeeping is complete and
/// before returning to the thread's code. CR3 is only written when it changes.
///
/// # Safety
/// Ring 0, single CPU. Every address space in use maps the kernel half identically (the process
/// PML4s copy it from the kernel template), so the executing code and stack stay mapped across
/// the CR3 write.
#[cfg(all(target_arch = "x86_64", not(test)))]
pub unsafe fn apply_switch_context<M: crate::tls::TlsMsr>(
    ctx: Option<SwitchContext>,
    kernel_cr3: u64,
    msr: &mut M,
) -> Result<(), crate::tls::TlsError> {
    let (cr3, tls) = match ctx {
        Some(c) => (c.cr3, c.tls),
        None => (kernel_cr3, TlsContext::default()),
    };
    tls.validate()?;
    let cur: u64;
    // SAFETY: reading CR3 has no side effects.
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cur, options(nomem, nostack, preserves_flags))
    };
    if cur & !0xFFF != cr3 & !0xFFF {
        // SAFETY: forwarded to the caller (see above).
        unsafe { core::arch::asm!("mov cr3, {}", in(reg) cr3, options(nostack, preserves_flags)) };
    }
    crate::tls::switch_in(msr, &tls)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address_space::fakes::Fx;
    use crate::sched::Scheduler;
    use crate::thread::{Priority, State};
    extern crate std;
    use std::vec;
    use std::vec::Vec;

    struct Env {
        fx: Fx,
        sched: Scheduler<16>,
        freed_tls: Vec<u64>,
        cr3: u64,
        fail_free_tls: bool,
    }

    impl Env {
        fn new(pages: usize) -> Self {
            let mut sched = Scheduler::new();
            sched.init_idle(0, 0).unwrap();
            Env {
                fx: Fx::new(pages),
                sched,
                freed_tls: vec![],
                cr3: 0,
                fail_free_tls: false,
            }
        }
        fn spawn(&mut self, prio: u8) -> Handle {
            self.sched
                .spawn(Priority::new(prio).unwrap(), 0, 0)
                .unwrap()
        }
        fn run(&mut self) -> Handle {
            let sw = self.sched.schedule().unwrap();
            if sw.is_switch()
                && let Some(p) = sw.prev
            {
                self.sched.switch_complete(p).unwrap();
            }
            sw.next
        }
        fn run_until(&mut self, h: Handle) {
            for _ in 0..40 {
                if self.sched.current() == Some(h) {
                    return;
                }
                self.run();
            }
            panic!("never current");
        }
    }

    impl ProcessEnv for Env {
        fn phys(&mut self) -> (&mut dyn PhysMem, &mut dyn FrameAlloc) {
            (&mut self.fx.mem, &mut self.fx.frames)
        }
        fn current_thread(&self) -> Option<Handle> {
            self.sched.current()
        }
        fn active_cr3(&self) -> u64 {
            self.cr3
        }
        fn kill_thread(&mut self, h: Handle, status: i32) -> Result<(), EnvError> {
            self.sched.kill(h, status).map_err(|e| match e {
                crate::thread::Error::Stale => EnvError::Gone,
                _ => EnvError::Failed,
            })
        }
        fn reap_thread(&mut self, h: Handle) -> Result<(), EnvError> {
            self.sched.reap(h).map(|_| ()).map_err(|e| match e {
                crate::thread::Error::StillOnCpu => EnvError::StillOnCpu,
                crate::thread::Error::Stale => EnvError::Gone,
                _ => EnvError::Failed,
            })
        }
        fn free_tls(&mut self, key: u64) -> Result<(), EnvError> {
            if self.fail_free_tls {
                return Err(EnvError::Failed);
            }
            self.freed_tls.push(key);
            Ok(())
        }
    }

    fn tiny_elf() -> Vec<u8> {
        let mut v = vec![0u8; 0x1000];
        v[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        v[4] = 2;
        v[5] = 1;
        v[6] = 1;
        v[16..18].copy_from_slice(&2u16.to_le_bytes());
        v[18..20].copy_from_slice(&62u16.to_le_bytes());
        v[20..24].copy_from_slice(&1u32.to_le_bytes());
        v[24..32].copy_from_slice(&0x40_0000u64.to_le_bytes());
        v[32..40].copy_from_slice(&64u64.to_le_bytes());
        v[52..54].copy_from_slice(&64u16.to_le_bytes());
        v[54..56].copy_from_slice(&56u16.to_le_bytes());
        v[56..58].copy_from_slice(&1u16.to_le_bytes());
        let o = 64;
        v[o..o + 4].copy_from_slice(&1u32.to_le_bytes());
        v[o + 4..o + 8].copy_from_slice(&5u32.to_le_bytes()); // R+X
        v[o + 8..o + 16].copy_from_slice(&0x1000u64.to_le_bytes());
        v[o + 16..o + 24].copy_from_slice(&0x40_0000u64.to_le_bytes());
        v[o + 32..o + 40].copy_from_slice(&16u64.to_le_bytes());
        v[o + 40..o + 48].copy_from_slice(&16u64.to_le_bytes());
        v[o + 48..o + 56].copy_from_slice(&0x1000u64.to_le_bytes());
        v.extend_from_slice(&[0x90; 16]);
        v
    }

    fn root(env: &mut Env, t: &mut ProcessTable<8>) -> ProcHandle {
        let kernel = env.fx.kernel;
        let (h, _) = t
            .spawn_elf(env, None, Limits::DEFAULT, kernel, &tiny_elf())
            .unwrap();
        h
    }

    fn child(
        env: &mut Env,
        t: &mut ProcessTable<8>,
        parent: ProcHandle,
    ) -> (ProcHandle, LoadedImage) {
        let kernel = env.fx.kernel;
        t.spawn_elf(env, Some(parent), Limits::DEFAULT, kernel, &tiny_elf())
            .unwrap()
    }

    #[test]
    fn create_load_and_launch_a_real_process_image() {
        let mut env = Env::new(128);
        let mut t = ProcessTable::<8>::new();
        let baseline = env.fx.frames.in_use();
        let r = root(&mut env, &mut t);
        assert_eq!(t.state(r), Ok(ProcState::Creating));
        let (c, img) = child(&mut env, &mut t, r);
        assert_eq!(t.parent(c), Ok(Some(r)));
        assert_eq!(t.usage(r).unwrap().children, 1);
        let th = env.spawn(4);
        t.add_thread(c, th, Some(7), TlsContext::default()).unwrap();
        assert_eq!(t.state(c), Ok(ProcState::Running));
        t.prepare_launch(c, th, &img, 0).unwrap();
        let l = t.launch_info(th).unwrap();
        assert_eq!(l.frame.rip, 0x40_0000);
        assert_eq!(l.frame.rsp, img.initial_sp);
        assert_eq!(l.frame.cs & 3, 3);
        assert_eq!(l.cr3, t.space(c).unwrap().unwrap().cr3_value());
        assert_ne!(
            l.cr3,
            t.space(r).unwrap().unwrap().cr3_value(),
            "distinct address spaces"
        );
        assert_eq!(t.switch_context_for(th).unwrap().cr3, l.cr3);
        assert_eq!(
            t.switch_context_for(env.sched.idle().unwrap()),
            None,
            "kernel thread"
        );
        assert_eq!(t.owner_of(th), Some(c));
        // The pages really are in the process's own tables.
        let mut b = [0u8; 4];
        t.space(c)
            .unwrap()
            .unwrap()
            .copy_from_user(&mut env.fx.mem, 0x40_0000, &mut b)
            .unwrap();
        assert_eq!(b, [0x90; 4]);
        assert!(env.fx.frames.in_use() > baseline);
        // Tear everything down: no frame leaks.
        t.exit(&mut env, c, ExitStatus::Code(0)).unwrap();
        t.exit(&mut env, r, ExitStatus::Code(0)).unwrap();
        assert_eq!(env.fx.frames.in_use(), baseline, "all frames returned");
    }

    #[test]
    fn parent_child_lifecycle_zombie_wait_and_stale_handles() {
        let mut env = Env::new(128);
        let mut t = ProcessTable::<8>::new();
        let r = root(&mut env, &mut t);
        let (c, _) = child(&mut env, &mut t, r);
        assert_eq!(t.wait_child(r, c), Err(Error::NotExited));
        t.exit(&mut env, c, ExitStatus::Code(42)).unwrap();
        assert_eq!(t.state(c), Ok(ProcState::Zombie));
        assert_eq!(
            t.space(c).unwrap().is_none(),
            true,
            "address space already released"
        );
        assert_ne!(t.pending_events(r).unwrap() & EVENT_CHILD_EXITED, 0);
        let (pid, st) = t.wait_child(r, c).unwrap();
        assert_eq!(st, ExitStatus::Code(42));
        assert_eq!(pid.0, 2);
        assert_eq!(t.state(c), Err(Error::Stale), "stale after wait");
        assert_eq!(t.wait_child(r, c), Err(Error::Stale));
        assert_eq!(t.usage(r).unwrap().children, 0);
        // A non-child cannot be waited for.
        let (c2, _) = child(&mut env, &mut t, r);
        let (g, _) = child(&mut env, &mut t, c2);
        assert_eq!(t.wait_child(r, g), Err(Error::NotChild));
        // Pids are never reused even when slots are.
        t.exit(&mut env, g, ExitStatus::Code(0)).unwrap();
        t.wait_child(c2, g).unwrap();
        let (c3, _) = child(&mut env, &mut t, r);
        assert!(t.pid(c3).unwrap().0 > 3);
    }

    #[test]
    fn exit_status_is_first_wins_and_exit_is_idempotent() {
        let mut env = Env::new(128);
        let mut t = ProcessTable::<8>::new();
        let r = root(&mut env, &mut t);
        let (c, _) = child(&mut env, &mut t, r);
        t.exit(&mut env, c, ExitStatus::Code(1)).unwrap();
        t.exit(&mut env, c, ExitStatus::Code(2)).unwrap();
        assert_eq!(t.exit_status(c), Ok(Some(ExitStatus::Code(1))));
        assert_eq!(
            t.post_event(c, EVENT_USER_FIRST),
            Err(Error::BadState),
            "no events to zombies"
        );
    }

    #[test]
    fn orphans_are_reparented_and_parentless_zombies_released() {
        let mut env = Env::new(160);
        let mut t = ProcessTable::<8>::new();
        let r = root(&mut env, &mut t);
        let (mid, _) = child(&mut env, &mut t, r);
        let (leaf_z, _) = child(&mut env, &mut t, mid);
        let (leaf_r, _) = child(&mut env, &mut t, mid);
        t.exit(&mut env, leaf_z, ExitStatus::Code(0)).unwrap(); // zombie awaiting mid
        t.exit(&mut env, mid, ExitStatus::Code(0)).unwrap();
        assert_eq!(
            t.state(leaf_z),
            Err(Error::Stale),
            "zombie orphan released with its parent"
        );
        assert_eq!(t.parent(leaf_r), Ok(None), "running orphan reparented");
        t.exit(&mut env, leaf_r, ExitStatus::Code(0)).unwrap();
        assert_eq!(
            t.state(leaf_r),
            Err(Error::Stale),
            "parentless zombie released at once"
        );
        let mid_pid = t.pid(mid).unwrap();
        assert_eq!(t.wait_any(r).unwrap(), Some((mid_pid, ExitStatus::Code(0))));
        assert_eq!(t.wait_any(r).unwrap(), None);
    }

    #[test]
    fn exit_kills_runnable_blocked_and_sleeping_threads_and_frees_tls() {
        let mut env = Env::new(128);
        let mut t = ProcessTable::<8>::new();
        let top = root(&mut env, &mut t);
        let baseline = env.fx.frames.in_use();
        let (r, _) = child(&mut env, &mut t, top);
        let a = env.spawn(4);
        let b = env.spawn(4);
        let c = env.spawn(4);
        let d = env.spawn(4);
        for (i, th) in [a, b, c, d].into_iter().enumerate() {
            t.add_thread(r, th, Some(100 + i as u64), TlsContext::default())
                .unwrap();
        }
        // a: Ready, b: Blocked, c: Sleeping, d: the running thread (calls exit).
        env.run_until(b);
        env.sched
            .block_current(crate::thread::BlockReason::Event(9))
            .unwrap();
        env.run();
        env.run_until(c);
        env.sched.sleep_current(1000).unwrap();
        env.run();
        env.run_until(d);
        assert_eq!(env.sched.thread(a).unwrap().state(), State::Ready);
        assert_eq!(env.sched.thread(b).unwrap().state(), State::Blocked);
        assert_eq!(env.sched.thread(c).unwrap().state(), State::Sleeping);
        t.exit(&mut env, r, ExitStatus::Code(3)).unwrap();
        assert_eq!(
            t.state(r),
            Ok(ProcState::Exiting),
            "the running thread is still alive"
        );
        for th in [a, b, c] {
            assert_eq!(
                env.sched.thread(th).map(|x| x.state()),
                Err(crate::thread::Error::Stale)
            );
        }
        assert_eq!(t.usage(r).unwrap().threads, 1);
        // The running thread now exits itself; the process finishes once it is off the CPU.
        env.sched.terminate_current(3).unwrap();
        t.thread_exited(&mut env, r, d, 3).unwrap();
        assert_eq!(t.state(r), Ok(ProcState::Exiting), "d is still on the CPU");
        env.run(); // switch away completes
        t.collect(&mut env, r).unwrap();
        assert_eq!(t.state(r), Ok(ProcState::Zombie));
        assert_eq!(t.exit_status(r), Ok(Some(ExitStatus::Code(3))));
        let mut freed = env.freed_tls.clone();
        freed.sort();
        assert_eq!(freed, vec![100, 101, 102, 103]);
        assert_eq!(
            env.fx.frames.in_use(),
            baseline,
            "address space and tables returned"
        );
        env.sched.check_invariants().unwrap();
        let rpid = t.pid(r).unwrap();
        assert_eq!(t.wait_child(top, r), Ok((rpid, ExitStatus::Code(3))));
    }

    #[test]
    fn last_thread_exiting_ends_the_process_with_its_status() {
        let mut env = Env::new(128);
        let mut t = ProcessTable::<8>::new();
        let r = root(&mut env, &mut t);
        let (c, _) = child(&mut env, &mut t, r);
        let (a, b) = (env.spawn(4), env.spawn(4));
        t.add_thread(c, a, None, TlsContext::default()).unwrap();
        t.add_thread(c, b, None, TlsContext::default()).unwrap();
        env.run_until(a);
        env.sched.terminate_current(5).unwrap();
        t.thread_exited(&mut env, c, a, 5).unwrap();
        assert_eq!(
            t.state(c),
            Ok(ProcState::Running),
            "another thread is alive"
        );
        env.run_until(b);
        env.sched.terminate_current(7).unwrap();
        t.thread_exited(&mut env, c, b, 7).unwrap();
        env.run();
        t.collect(&mut env, c).unwrap();
        assert_eq!(t.state(c), Ok(ProcState::Zombie));
        assert_eq!(t.exit_status(c), Ok(Some(ExitStatus::Code(7))));
    }

    #[test]
    fn address_space_is_not_destroyed_while_active() {
        let mut env = Env::new(128);
        let mut t = ProcessTable::<8>::new();
        let r = root(&mut env, &mut t);
        let (c, _) = child(&mut env, &mut t, r);
        env.cr3 = t.space(c).unwrap().unwrap().cr3_value();
        assert_eq!(
            t.exit(&mut env, c, ExitStatus::Code(0)),
            Err(Error::SpaceActive)
        );
        assert_eq!(t.state(c), Ok(ProcState::Exiting));
        assert!(t.space(c).unwrap().is_some(), "nothing released yet");
        env.cr3 = env.fx.kernel; // switched to the kernel address space
        t.collect(&mut env, c).unwrap();
        assert_eq!(t.state(c), Ok(ProcState::Zombie));
    }

    #[test]
    fn kill_event_is_unmaskable_other_events_obey_the_mask() {
        let mut env = Env::new(128);
        let mut t = ProcessTable::<8>::new();
        let r = root(&mut env, &mut t);
        let (c, _) = child(&mut env, &mut t, r);
        t.set_event_mask(c, 0).unwrap();
        t.post_event(c, EVENT_USER_FIRST | EVENT_CHILD_EXITED)
            .unwrap();
        assert_eq!(t.take_events(c), Ok(0), "masked");
        t.set_event_mask(c, EVENT_USER_FIRST).unwrap();
        assert_eq!(t.take_events(c), Ok(EVENT_USER_FIRST));
        assert_eq!(t.take_events(c), Ok(0), "consumed");
        assert_eq!(t.deliver(&mut env, c), Ok(false));
        t.set_event_mask(c, 0).unwrap();
        t.post_event(c, EVENT_KILL).unwrap();
        assert_eq!(t.deliver(&mut env, c), Ok(true));
        assert_eq!(t.exit_status(c), Ok(Some(ExitStatus::Killed)));
        assert_eq!(t.state(c), Ok(ProcState::Zombie));
    }

    #[test]
    fn user_fault_terminates_the_process() {
        let mut env = Env::new(128);
        let mut t = ProcessTable::<8>::new();
        let r = root(&mut env, &mut t);
        let (c, _) = child(&mut env, &mut t, r);
        let th = env.spawn(4);
        t.add_thread(c, th, None, TlsContext::default()).unwrap();
        env.run_until(th);
        t.fault_exit(&mut env, c, 14).unwrap();
        assert_eq!(t.exit_status(c), Ok(Some(ExitStatus::Fault(14))));
        assert_eq!(
            t.state(c),
            Ok(ProcState::Exiting),
            "faulting thread is the running one"
        );
    }

    #[test]
    fn limits_are_enforced_before_resources_are_taken() {
        let mut env = Env::new(256);
        let mut t = ProcessTable::<8>::new();
        let kernel = env.fx.kernel;
        let lim = Limits {
            max_threads: 2,
            max_pages: 20,
            max_children: 1,
        };
        let (r, _) = t
            .spawn_elf(&mut env, None, lim, kernel, &tiny_elf())
            .unwrap();
        // Threads.
        let ths: Vec<Handle> = (0..3).map(|_| env.spawn(3)).collect();
        t.add_thread(r, ths[0], None, TlsContext::default())
            .unwrap();
        t.add_thread(r, ths[1], None, TlsContext::default())
            .unwrap();
        assert_eq!(
            t.add_thread(r, ths[2], None, TlsContext::default()),
            Err(Error::Limit(LimitKind::Threads))
        );
        assert_eq!(
            t.add_thread(r, ths[0], None, TlsContext::default()),
            Err(Error::ThreadOwned)
        );
        // Pages: the image + stack already use some; the limit is checked before mapping.
        assert_eq!(t.usage(r).unwrap().mapped_pages, 17);
        // Children.
        let (c, _) = t
            .spawn_elf(&mut env, Some(r), Limits::DEFAULT, kernel, &tiny_elf())
            .unwrap();
        let before = env.fx.frames.in_use();
        assert_eq!(
            t.spawn_elf(&mut env, Some(r), Limits::DEFAULT, kernel, &tiny_elf())
                .err(),
            Some(Error::Limit(LimitKind::Children))
        );
        assert_eq!(
            env.fx.frames.in_use(),
            before,
            "refused before allocating anything"
        );
        let _ = c;
        // Bad limits.
        for bad in [
            Limits {
                max_threads: 0,
                ..Limits::DEFAULT
            },
            Limits {
                max_threads: 99,
                ..Limits::DEFAULT
            },
            Limits {
                max_pages: 0,
                ..Limits::DEFAULT
            },
        ] {
            assert_eq!(
                t.spawn_elf(&mut env, Some(r), bad, kernel, &tiny_elf())
                    .err(),
                Some(Error::BadLimits)
            );
        }
    }

    #[test]
    fn page_limit_blocks_mapping_and_oversized_images() {
        let mut env = Env::new(256);
        let mut t = ProcessTable::<8>::new();
        let kernel = env.fx.kernel;
        // The image (1 page) + stack (16 pages) already exceed a 10-page limit: refused, no leak.
        let before = env.fx.frames.in_use();
        let small = Limits {
            max_pages: 10,
            ..Limits::DEFAULT
        };
        assert_eq!(
            t.spawn_elf(&mut env, None, small, kernel, &tiny_elf())
                .err(),
            Some(Error::Limit(LimitKind::Pages))
        );
        assert_eq!(env.fx.frames.in_use(), before);
        assert_eq!(t.live(), 0);
        let lim = Limits {
            max_pages: 20,
            ..Limits::DEFAULT
        };
        let (r, _) = t
            .spawn_elf(&mut env, None, lim, kernel, &tiny_elf())
            .unwrap();
        assert_eq!(t.usage(r).unwrap().mapped_pages, 17);
        t.map_anon(&mut env, r, 0x100_0000, 3 * PAGE, Perms::rw())
            .unwrap();
        assert_eq!(
            t.map_anon(&mut env, r, 0x200_0000, PAGE, Perms::rw()),
            Err(Error::Limit(LimitKind::Pages))
        );
        assert_eq!(
            t.map_anon(&mut env, r, 0x400_0000, u64::MAX, Perms::rw()),
            Err(Error::Space(SpaceError::OutOfUserRange))
        );
        assert_eq!(t.usage(r).unwrap().mapped_pages, 20);
    }

    #[test]
    fn spawn_failures_leave_no_frames_behind() {
        let mut env = Env::new(256);
        let mut t = ProcessTable::<8>::new();
        let kernel = env.fx.kernel;
        let img = tiny_elf();
        for fail_at in 0..40usize {
            let before = env.fx.frames.in_use();
            env.fx.frames.fail_at = Some(env.fx.frames.calls + fail_at);
            let r = t.spawn_elf(&mut env, None, Limits::DEFAULT, kernel, &img);
            env.fx.frames.fail_at = None;
            match r {
                Ok((h, _)) => {
                    t.exit(&mut env, h, ExitStatus::Code(0)).unwrap();
                    assert_eq!(env.fx.frames.in_use(), before, "teardown at {fail_at}");
                    break;
                }
                Err(_) => {
                    assert_eq!(
                        env.fx.frames.in_use(),
                        before,
                        "leak when allocation {fail_at} failed"
                    );
                    assert_eq!(t.live(), 0);
                }
            }
        }
        // Malformed executables are rejected without side effects.
        let mut bad = img.clone();
        bad[0] = 0;
        let before = env.fx.frames.in_use();
        assert_eq!(
            t.spawn_elf(&mut env, None, Limits::DEFAULT, kernel, &bad)
                .err(),
            Some(Error::Elf(elf::Error::BadMagic))
        );
        assert_eq!(env.fx.frames.in_use(), before);
    }

    #[test]
    fn table_exhaustion_and_a_single_root() {
        let mut env = Env::new(512);
        let mut t = ProcessTable::<4>::new();
        let kernel = env.fx.kernel;
        let (r, _) = t
            .spawn_elf(&mut env, None, Limits::DEFAULT, kernel, &tiny_elf())
            .unwrap();
        assert_eq!(
            t.spawn_elf(&mut env, None, Limits::DEFAULT, kernel, &tiny_elf())
                .err(),
            Some(Error::NoParentSlot),
            "only one root"
        );
        for _ in 0..3 {
            t.spawn_elf(&mut env, Some(r), Limits::DEFAULT, kernel, &tiny_elf())
                .unwrap();
        }
        let before = env.fx.frames.in_use();
        assert_eq!(
            t.spawn_elf(&mut env, Some(r), Limits::DEFAULT, kernel, &tiny_elf())
                .err(),
            Some(Error::NoSlots)
        );
        assert_eq!(
            env.fx.frames.in_use(),
            before,
            "no leak on table exhaustion"
        );
        // A zombie parent cannot have children.
        t.exit(&mut env, r, ExitStatus::Code(0)).unwrap();
        assert_eq!(t.state(r), Err(Error::Stale));
    }

    #[test]
    fn invalid_handles_are_rejected_everywhere() {
        let mut env = Env::new(128);
        let mut t = ProcessTable::<8>::new();
        let r = root(&mut env, &mut t);
        let (c, img) = child(&mut env, &mut t, r);
        t.exit(&mut env, c, ExitStatus::Code(0)).unwrap();
        t.wait_child(r, c).unwrap();
        let th = env.spawn(4);
        assert_eq!(
            t.add_thread(c, th, None, TlsContext::default()),
            Err(Error::Stale)
        );
        assert_eq!(t.prepare_launch(c, th, &img, 0), Err(Error::Stale));
        assert_eq!(t.exit(&mut env, c, ExitStatus::Code(0)), Err(Error::Stale));
        assert_eq!(t.post_event(c, 1), Err(Error::Stale));
        assert_eq!(t.take_events(c), Err(Error::Stale));
        assert_eq!(t.usage(c), Err(Error::Stale));
        assert_eq!(t.wait_child(r, c), Err(Error::Stale));
        let bogus = ProcHandle {
            index: 99,
            generation: 0,
        };
        assert_eq!(t.state(bogus), Err(Error::Stale));
        // A thread the process does not own.
        assert_eq!(t.prepare_launch(r, th, &img, 0), Err(Error::NoSuchThread));
        assert_eq!(
            t.save_tls(th, TlsContext::default()),
            Err(Error::NoSuchThread)
        );
    }

    #[test]
    fn launch_frames_are_validated() {
        let mut env = Env::new(128);
        let mut t = ProcessTable::<8>::new();
        let r = root(&mut env, &mut t);
        let (c, mut img) = child(&mut env, &mut t, r);
        let th = env.spawn(4);
        t.add_thread(c, th, None, TlsContext::default()).unwrap();
        let good = img;
        img.entry = 0xFFFF_8000_0000_0000;
        assert!(matches!(
            t.prepare_launch(c, th, &img, 0),
            Err(Error::User(_))
        ));
        img = good;
        img.initial_sp = 0xFFFF_8000_0000_1000;
        assert!(matches!(
            t.prepare_launch(c, th, &img, 0),
            Err(Error::User(_))
        ));
        img = good;
        img.initial_sp += 8; // unaligned
        assert!(matches!(
            t.prepare_launch(c, th, &img, 0),
            Err(Error::User(UserError::UnalignedStack))
        ));
        assert!(
            t.launch_info(th).is_none(),
            "nothing recorded for a rejected frame"
        );
    }

    #[test]
    fn tls_context_is_saved_and_returned_per_thread() {
        let mut env = Env::new(128);
        let mut t = ProcessTable::<8>::new();
        let r = root(&mut env, &mut t);
        let (a, b) = (env.spawn(4), env.spawn(4));
        t.add_thread(
            r,
            a,
            Some(1),
            TlsContext {
                fs_base: 0x1000,
                gs_base: 0,
            },
        )
        .unwrap();
        t.add_thread(
            r,
            b,
            Some(2),
            TlsContext {
                fs_base: 0x2000,
                gs_base: 0,
            },
        )
        .unwrap();
        assert_eq!(t.switch_context_for(a).unwrap().tls.fs_base, 0x1000);
        assert_eq!(t.switch_context_for(b).unwrap().tls.fs_base, 0x2000);
        t.save_tls(
            a,
            TlsContext {
                fs_base: 0x1234,
                gs_base: 9,
            },
        )
        .unwrap();
        assert_eq!(
            t.switch_context_for(a).unwrap().tls,
            TlsContext {
                fs_base: 0x1234,
                gs_base: 9
            }
        );
        assert_eq!(
            t.switch_context_for(b).unwrap().tls.fs_base,
            0x2000,
            "isolated"
        );
    }

    #[test]
    fn failed_tls_release_keeps_the_process_exiting_and_retryable() {
        let mut env = Env::new(128);
        let mut t = ProcessTable::<8>::new();
        let r = root(&mut env, &mut t);
        let (c, _) = child(&mut env, &mut t, r);
        let th = env.spawn(4);
        t.add_thread(c, th, Some(5), TlsContext::default()).unwrap();
        env.fail_free_tls = true;
        assert_eq!(
            t.exit(&mut env, c, ExitStatus::Code(0)),
            Err(Error::Env(EnvError::Failed))
        );
        assert_eq!(t.state(c), Ok(ProcState::Exiting));
        env.fail_free_tls = false;
        t.collect(&mut env, c).unwrap();
        assert_eq!(t.state(c), Ok(ProcState::Zombie));
        assert_eq!(env.freed_tls, vec![5]);
    }

    /// Repeated create/run/terminate cycles with a mixed thread population must not drift:
    /// frames, threads and process slots all return to their baseline every time.
    #[test]
    fn stress_repeated_creation_and_termination_does_not_leak() {
        let mut env = Env::new(256);
        let mut t = ProcessTable::<8>::new();
        let kernel = env.fx.kernel;
        let r = root(&mut env, &mut t);
        let baseline_frames = env.fx.frames.in_use();
        let baseline_threads = env.sched.live_threads();
        let mut rng = 0x1234_5678_9abc_def1u64;
        let mut next = move || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        for round in 0..300u64 {
            let (c, _) = t
                .spawn_elf(&mut env, Some(r), Limits::DEFAULT, kernel, &tiny_elf())
                .unwrap();
            let n = 1 + (next() % 4) as usize;
            let mut ths = vec![];
            for k in 0..n {
                let th = env.spawn(1 + (next() % 7) as u8);
                t.add_thread(c, th, Some(round * 10 + k as u64), TlsContext::default())
                    .unwrap();
                ths.push(th);
            }
            // Let the scheduler run a few steps so threads are in assorted states.
            for _ in 0..(next() % 6) {
                let cur = env.run();
                if ths.contains(&cur) {
                    match next() % 3 {
                        0 => {
                            if env
                                .sched
                                .block_current(crate::thread::BlockReason::Event(1))
                                .is_ok()
                            {
                                env.run();
                            }
                        }
                        1 => {
                            if env.sched.sleep_current(5).is_ok() {
                                env.run();
                            }
                        }
                        _ => {}
                    }
                }
            }
            if next() % 2 == 0 {
                t.exit(&mut env, c, ExitStatus::Code(round as i32)).unwrap();
            } else {
                t.post_event(c, EVENT_KILL).unwrap();
                t.deliver(&mut env, c).unwrap();
            }
            // If one of its threads is still on the CPU, finish its exit then collect.
            if let Some(cur) = env.sched.current() {
                if ths.contains(&cur) {
                    env.sched.terminate_current(0).unwrap();
                    t.thread_exited(&mut env, c, cur, 0).unwrap();
                }
            }
            env.run();
            t.collect(&mut env, c).unwrap();
            assert_eq!(t.state(c), Ok(ProcState::Zombie), "round {round}");
            t.wait_child(r, c).unwrap();
            assert_eq!(
                env.fx.frames.in_use(),
                baseline_frames,
                "frames drift at round {round}"
            );
            assert_eq!(
                env.sched.live_threads(),
                baseline_threads,
                "threads drift at round {round}"
            );
            assert_eq!(t.live(), 1);
            env.sched.check_invariants().unwrap();
        }
    }
}
