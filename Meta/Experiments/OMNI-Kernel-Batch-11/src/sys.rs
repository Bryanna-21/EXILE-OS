//! Syscalls 0x100..=0x107 (Batch 5 numbers and argument layouts) for the Rust kernel.
//!
//! `IpcKernel` wraps the Batch 10 `AbiKernel`: numbers outside 0x100..=0x107 are passed through
//! untouched. Every call: the caller thread and space come from the scheduler, never from arguments;
//! unused argument registers must be zero; handles above 32 bits are rejected, not truncated; user
//! structures are copied into kernel memory and decoded field by field; lengths are bounded before use;
//! output buffers are probed before anything irreversible happens; every capability use is checked.
//!
//! Not implemented (documented deviations, docs/IPC_DESIGN.md): cancellation objects (0x104, 0x105 return
//! NOT_SUPPORTED), blocking send (a full queue returns WOULD_BLOCK), partial delivery of descriptors
//! (receive is all-or-nothing), unbounded waits (INVALID_ARGUMENT, as in the model when no cancel handle
//! exists). Timeouts are scheduler ticks, not nanoseconds.

use crate::caps::{Handle, ObjectType, Rights};
use crate::ipc::{ABI_VERSION_WIRE, Desc, MAX_DESC, MAX_MSG_BYTES, Mode, State, WIRE_HEADER};
use crate::status;
use omni_kernel7::sched::{Reason, SchedError, SpaceId, ThreadId};
use omni_kernel7::sync::SpinLock;
use omni_kernel7::syscall::{EXIT_BAD_FRAME, EXIT_KILLED, Kernel, SyscallFrame, validate_frame};
use omni_kernel7::uaccess::{Access, UserBacking};
use omni10::abi::{AbiKernel, AbiMode, to_omni};

pub const SYS_ENDPOINT_CREATE: u64 = 0x100;
pub const SYS_ENDPOINT_CLOSE: u64 = 0x101;
pub const SYS_SEND: u64 = 0x102;
pub const SYS_RECV: u64 = 0x103;
pub const SYS_CANCEL_CREATE: u64 = 0x104;
pub const SYS_CANCEL_SIGNAL: u64 = 0x105;
pub const SYS_ENDPOINT_STATS: u64 = 0x106;
pub const SYS_HANDLE_RELEASE: u64 = 0x107;

pub const SEND_ARGS_SIZE: usize = 48;
pub const DESC_SIZE: usize = 16;
pub const RECV_ARGS_SIZE: usize = 48;
pub const RECV_RESULT_SIZE: usize = 88;
pub const STATS_SIZE: usize = 32;
pub const CREATE_OUT_SIZE: usize = 8;
/// Longest accepted wait, in scheduler ticks.
pub const MAX_WAIT_TICKS: u64 = 1 << 20;
pub const WAIT_FOREVER: u64 = u64::MAX;

const SLOT_DELIVERED: u32 = 1;
const SLOT_REVOKED: u32 = 2;

type R<T> = Result<T, i64>;

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u64le(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}
fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn handle_arg(v: u64) -> R<Handle> {
    if v > u32::MAX as u64 {
        Err(status::INVALID_ARGUMENT)
    } else {
        Ok(Handle(v as u32))
    }
}

fn require_zero(a: &[u64; 6], from: usize) -> R<()> {
    if a[from..].iter().any(|&v| v != 0) {
        Err(status::INVALID_ARGUMENT)
    } else {
        Ok(())
    }
}

/// Header validation in the fixed order of the Batch 2 wire format. Returns (op, kind, request_id, handles).
fn decode_header(b: &[u8]) -> R<(u16, u16, u64, u32)> {
    if b.len() < WIRE_HEADER {
        return Err(status::INVALID_ARGUMENT);
    }
    let size = u32le(b, 0);
    let abi_ver = u16le(b, 4);
    let op = u16le(b, 6);
    let flags = u32le(b, 8);
    let kind = u16le(b, 12);
    let reserved = u16le(b, 14);
    let request_id = u64le(b, 16);
    let payload_len = u32le(b, 24);
    let handle_count = u32le(b, 28);
    if abi_ver != ABI_VERSION_WIRE {
        return Err(status::BAD_VERSION);
    }
    if flags != 0 || reserved != 0 || op == 0 {
        return Err(status::INVALID_ARGUMENT);
    }
    if !(1..=3).contains(&kind) {
        return Err(status::INVALID_ARGUMENT);
    }
    if size as usize != b.len() || b.len() > MAX_MSG_BYTES {
        return Err(status::INVALID_ARGUMENT);
    }
    if payload_len as usize != b.len() - WIRE_HEADER {
        return Err(status::INVALID_ARGUMENT);
    }
    if handle_count as usize > MAX_DESC {
        return Err(status::INVALID_ARGUMENT);
    }
    Ok((op, kind, request_id, handle_count))
}

pub struct IpcKernel<'k> {
    pub abi: AbiKernel<'k>,
    backing: &'static dyn UserBacking,
    pub state: SpinLock<State>,
}

impl<'k> IpcKernel<'k> {
    /// `backing` must be the same object the Batch 7 `Kernel` was built with.
    pub const fn new(kernel: &'k Kernel, backing: &'static dyn UserBacking) -> Self {
        IpcKernel {
            abi: AbiKernel::new(kernel),
            backing,
            state: SpinLock::new(State::new()),
        }
    }

    /// Syscall entry. IPC numbers are handled here; everything else goes to the Batch 10 layer.
    pub fn syscall(&self, mode: AbiMode, f: &mut SyscallFrame) {
        if !(SYS_ENDPOINT_CREATE..=SYS_HANDLE_RELEASE).contains(&f.rax) {
            self.abi.syscall(mode, f);
            return;
        }
        let mut g = to_omni(mode, f);
        self.ipc_entry(&mut g);
        f.rax = g.rax;
        f.r11 = g.r11;
    }

    fn ipc_entry(&self, g: &mut SyscallFrame) {
        let k = self.abi.kernel;
        let caller = match k.sched.current() {
            Some(c) => c,
            None => {
                g.rax = status::INVALID_STATE as u64;
                return;
            }
        };
        if !validate_frame(g) {
            k.sched.exit_current(EXIT_BAD_FRAME);
        }
        // A killed thread must not start new work: its space may already have been cleaned up.
        if k.sched.is_killed(caller) {
            k.sched.exit_current(EXIT_KILLED);
        }
        let space = match k.sched.space_of(caller) {
            Some(s) => s,
            None => {
                g.rax = status::INTERNAL as u64;
                return;
            }
        };
        let r = self.dispatch(caller, space, g.rax, g.args());
        g.rax = match r {
            Ok(v) => v,
            Err(e) => e,
        } as u64;
        if k.sched.is_killed(caller) {
            k.sched.exit_current(EXIT_KILLED);
        }
    }

    fn dispatch(&self, caller: ThreadId, space: SpaceId, nr: u64, a: [u64; 6]) -> R<i64> {
        match nr {
            SYS_ENDPOINT_CREATE => self.endpoint_create(space, &a),
            SYS_ENDPOINT_CLOSE => {
                require_zero(&a, 1)?;
                let h = handle_arg(a[0])?;
                let k = self.abi.kernel;
                let mut st = self.state.lock();
                let (oi, _) = st.resolve(space, h, ObjectType::Channel, Rights::ADMIN)?;
                st.close(oi, &k.sched);
                Ok(0)
            }
            SYS_HANDLE_RELEASE => {
                require_zero(&a, 1)?;
                let h = handle_arg(a[0])?;
                let mut st = self.state.lock();
                st.table_mut(space)
                    .and_then(|t| t.remove(h))
                    .ok_or(status::INVALID_HANDLE)?;
                st.gc(&self.abi.kernel.sched);
                Ok(0)
            }
            SYS_ENDPOINT_STATS => self.endpoint_stats(space, &a),
            SYS_SEND => {
                require_zero(&a, 2)?;
                self.send(space, handle_arg(a[0])?, a[1])
            }
            SYS_RECV => {
                require_zero(&a, 2)?;
                self.recv(caller, space, handle_arg(a[0])?, a[1])
            }
            SYS_CANCEL_CREATE | SYS_CANCEL_SIGNAL => Err(status::NOT_SUPPORTED),
            _ => Err(status::NO_SYS),
        }
    }

    // ---- user memory ---------------------------------------------------------------

    fn copy_in(&self, space: SpaceId, addr: u64, out: &mut [u8]) -> R<()> {
        let k = self.abi.kernel;
        let sp = k.spaces.lock();
        sp.copy_from_user(self.backing, space, k.sched.loaded_space(), addr, out)
            .map_err(|e| e.status())
    }

    fn copy_out(&self, space: SpaceId, addr: u64, data: &[u8]) -> R<()> {
        let k = self.abi.kernel;
        let sp = k.spaces.lock();
        sp.copy_to_user(self.backing, space, k.sched.loaded_space(), addr, data)
            .map_err(|e| e.status())
    }

    fn probe_write(&self, space: SpaceId, addr: u64, len: u64) -> R<()> {
        let k = self.abi.kernel;
        let sp = k.spaces.lock();
        sp.get(space)
            .ok_or(status::FAULT)?
            .check_range(addr, len, Access::Write)
            .map_err(|e| e.status())
    }

    // ---- calls ---------------------------------------------------------------------

    fn endpoint_create(&self, space: SpaceId, a: &[u64; 6]) -> R<i64> {
        require_zero(a, 1)?;
        self.probe_write(space, a[0], CREATE_OUT_SIZE as u64)?;
        let (h0, h1) = self.state.lock().create_pair(space)?;
        let mut out = [0u8; CREATE_OUT_SIZE];
        out[0..4].copy_from_slice(&h0.0.to_le_bytes());
        out[4..8].copy_from_slice(&h1.0.to_le_bytes());
        if self.copy_out(space, a[0], &out).is_err() {
            // The caller can never learn the handles: take them back.
            let mut st = self.state.lock();
            if let Some(t) = st.table_mut(space) {
                t.remove(h0);
                t.remove(h1);
            }
            st.gc(&self.abi.kernel.sched);
            return Err(status::FAULT);
        }
        Ok(0)
    }

    fn endpoint_stats(&self, space: SpaceId, a: &[u64; 6]) -> R<i64> {
        require_zero(a, 2)?;
        let h = handle_arg(a[0])?;
        self.probe_write(space, a[1], STATS_SIZE as u64)?;
        let s = {
            let st = self.state.lock();
            let (oi, _) = st.resolve(space, h, ObjectType::Channel, Rights::INSPECT)?;
            st.stats(oi)
        };
        let mut out = [0u8; STATS_SIZE];
        out[0..4].copy_from_slice(&s.queued_messages.to_le_bytes());
        out[4..8].copy_from_slice(&(if s.peer_dead { 2u32 } else { 0 }).to_le_bytes());
        out[8..16].copy_from_slice(&s.queued_bytes.to_le_bytes());
        out[20..24].copy_from_slice(&s.users.to_le_bytes());
        self.copy_out(space, a[1], &out)?;
        Ok(0)
    }

    fn send(&self, space: SpaceId, ep: Handle, args_ptr: u64) -> R<i64> {
        let k = self.abi.kernel;
        let mut ab = [0u8; SEND_ARGS_SIZE];
        self.copy_in(space, args_ptr, &mut ab)?;
        let msg_ptr = u64le(&ab, 0);
        let msg_len = u32le(&ab, 8) as usize;
        let desc_count = u32le(&ab, 12) as usize;
        let desc_ptr = u64le(&ab, 16);
        let timeout = u64le(&ab, 24);
        let cancel = u32le(&ab, 32);
        let flags = u32le(&ab, 36);
        let reserved = u64le(&ab, 40);
        if flags != 0 || reserved != 0 {
            return Err(status::INVALID_ARGUMENT);
        }
        if !(WIRE_HEADER..=MAX_MSG_BYTES).contains(&msg_len) || desc_count > MAX_DESC {
            return Err(status::INVALID_ARGUMENT);
        }
        if timeout != 0 || cancel != 0 {
            return Err(status::NOT_SUPPORTED);
        }
        let mut buf = [0u8; MAX_MSG_BYTES];
        self.copy_in(space, msg_ptr, &mut buf[..msg_len])?;
        let (op, kind, request_id, handle_count) = decode_header(&buf[..msg_len])?;
        if handle_count as usize != desc_count {
            return Err(status::INVALID_ARGUMENT);
        }
        let mut descs = [Desc {
            handle: Handle::INVALID,
            otype: ObjectType::Channel,
            mode: Mode::Move,
            rights: Rights::NONE,
        }; MAX_DESC];
        if desc_count > 0 {
            let mut raw = [0u8; MAX_DESC * DESC_SIZE];
            self.copy_in(space, desc_ptr, &mut raw[..desc_count * DESC_SIZE])?;
            for (i, d) in descs.iter_mut().take(desc_count).enumerate() {
                let o = i * DESC_SIZE;
                let otype = ObjectType::from_raw(raw[o + 4]).ok_or(status::INVALID_ARGUMENT)?;
                let mode = match raw[o + 5] {
                    1 => Mode::Move,
                    2 => Mode::Copy,
                    _ => return Err(status::INVALID_ARGUMENT),
                };
                if u16le(&raw, o + 6) != 0 || u32le(&raw, o + 12) != 0 {
                    return Err(status::INVALID_ARGUMENT);
                }
                let rights =
                    Rights::from_raw(u32le(&raw, o + 8)).ok_or(status::INVALID_ARGUMENT)?;
                *d = Desc {
                    handle: Handle(u32le(&raw, o)),
                    otype,
                    mode,
                    rights,
                };
            }
        }
        let descs = &descs[..desc_count];
        let mut st = self.state.lock();
        let (oi, _) = st.resolve(space, ep, ObjectType::Channel, Rights::WRITE)?;
        let dst = st.peer_of(oi).ok_or(status::PEER_DEAD)?;
        st.validate_descs(space, descs, dst)?;
        if st.queue_len(dst) >= crate::ipc::QUEUE_DEPTH {
            return Err(status::WOULD_BLOCK);
        }
        st.commit_send(space, dst, &buf[..msg_len], op, kind, request_id, descs)?;
        st.wake_one(dst, &k.sched);
        Ok(0)
    }

    fn recv(&self, caller: ThreadId, space: SpaceId, ep: Handle, args_ptr: u64) -> R<i64> {
        let k = self.abi.kernel;
        let mut ab = [0u8; RECV_ARGS_SIZE];
        self.copy_in(space, args_ptr, &mut ab)?;
        let buf_ptr = u64le(&ab, 0);
        let buf_cap = u32le(&ab, 8) as usize;
        let flags = u32le(&ab, 12);
        let result_ptr = u64le(&ab, 16);
        let timeout = u64le(&ab, 24);
        let cancel = u32le(&ab, 32);
        let res32 = u32le(&ab, 36);
        let res64 = u64le(&ab, 40);
        if flags != 0 || res32 != 0 || res64 != 0 {
            return Err(status::INVALID_ARGUMENT);
        }
        // The buffer must hold the largest possible payload: a message is never truncated and never
        // consumed without being deliverable.
        let max_payload = MAX_MSG_BYTES - WIRE_HEADER;
        if buf_cap < max_payload {
            return Err(status::INVALID_ARGUMENT);
        }
        if cancel != 0 {
            return Err(status::NOT_SUPPORTED);
        }
        if timeout == WAIT_FOREVER || (timeout != 0 && timeout > MAX_WAIT_TICKS) {
            return Err(status::INVALID_ARGUMENT);
        }
        // Probe before consuming anything.
        self.probe_write(space, buf_ptr, max_payload as u64)?;
        self.probe_write(space, result_ptr, RECV_RESULT_SIZE as u64)?;
        let deadline = (timeout != 0).then(|| k.sched.ticks().saturating_add(timeout));

        let mut expired = false;
        let mut payload = [0u8; MAX_MSG_BYTES];
        let msg = loop {
            let mut st = self.state.lock();
            let (oi, _) = st.resolve(space, ep, ObjectType::Channel, Rights::READ)?;
            if let Some(x) = st.take(space, oi, &mut payload)? {
                break x;
            }
            if st.is_peer_dead(oi) {
                return Err(status::PEER_DEAD);
            }
            if timeout == 0 {
                return Err(status::WOULD_BLOCK);
            }
            if expired {
                return Err(status::TIMED_OUT);
            }
            // The ticket is taken while the state lock is held: a sender cannot enqueue and wake us
            // between the emptiness check and the block.
            let ticket = k.sched.prepare_block().map_err(|_| status::INVALID_STATE)?;
            if !st.add_waiter(oi, caller, &k.sched) {
                return Err(status::BUSY);
            }
            drop(st);
            let r = k.sched.block(ticket, deadline);
            self.state.lock().remove_waiter(oi, caller);
            match r {
                Ok(Reason::Timeout) => expired = true,
                Ok(Reason::Woken) | Ok(Reason::None) => {}
                Ok(Reason::Cancelled) | Ok(Reason::Killed) => return Err(status::CANCELLED),
                Err(SchedError::Stale) | Err(SchedError::InvalidState) | Err(SchedError::Full) => {
                    return Err(status::INVALID_STATE);
                }
            }
        };

        let n = msg.ndesc;
        let handles = msg.handles;
        let mut rb = [0u8; RECV_RESULT_SIZE];
        rb[0..4].copy_from_slice(&((msg.len - WIRE_HEADER) as u32).to_le_bytes());
        rb[4..6].copy_from_slice(&msg.op.to_le_bytes());
        rb[6..8].copy_from_slice(&msg.kind.to_le_bytes());
        rb[8..16].copy_from_slice(&msg.request_id.to_le_bytes());
        rb[20..24].copy_from_slice(&(n as u32).to_le_bytes());
        for (i, h) in handles.iter().take(n).enumerate() {
            let st = if *h == Handle::INVALID {
                SLOT_REVOKED
            } else {
                SLOT_DELIVERED
            };
            rb[24 + i * 8..28 + i * 8].copy_from_slice(&st.to_le_bytes());
            rb[28 + i * 8..32 + i * 8].copy_from_slice(&h.0.to_le_bytes());
        }
        let ok = self
            .copy_out(space, buf_ptr, &payload[WIRE_HEADER..msg.len])
            .is_ok()
            && self.copy_out(space, result_ptr, &rb).is_ok();
        if !ok {
            // The message was consumed and cannot be returned. Release what was installed so the
            // caller is not left holding authority it can never name.
            let mut st = self.state.lock();
            if let Some(t) = st.table_mut(space) {
                for h in handles.iter().take(n).filter(|h| **h != Handle::INVALID) {
                    t.remove(*h);
                }
            }
            st.gc(&k.sched);
            return Err(status::FAULT);
        }
        Ok(0)
    }

    // ---- lifecycle -----------------------------------------------------------------

    /// Release every capability of `space` (idempotent). Must be called when a space is destroyed.
    pub fn cleanup_space(&self, space: SpaceId) {
        let k = self.abi.kernel;
        self.state.lock().cleanup_space(space, &k.sched);
    }

    /// Kill all threads of `space` and release its capabilities.
    pub fn terminate_space(&self, space: SpaceId) -> Result<usize, SchedError> {
        let n = self.abi.kernel.sched.terminate_space(space)?;
        self.cleanup_space(space);
        Ok(n)
    }
}
