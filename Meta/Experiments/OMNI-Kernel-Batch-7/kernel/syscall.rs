//! Syscall dispatch. Entered from the SYSCALL stub (`arch/x86_64/syscall_entry.S`) through
//! `omni_syscall_common`, or directly by hosted tests.
//!
//! Rules enforced here:
//!  * The caller is whatever the scheduler says is running. No register or user-supplied
//!    value names the caller.
//!  * The syscall number is the full 64-bit rax; anything above `u32::MAX` is NO_SYS, it
//!    is never truncated into a valid number.
//!  * The frame is validated before it can be returned to: a user rip or rsp outside the
//!    user half terminates the thread (SYSRET with a non-canonical rcx faults in ring 0
//!    on Intel parts), and r11 is sanitised (IOPL, TF, NT, RF, VM, AC cleared; IF forced on).
//!  * A killed thread exits at the syscall boundary and never returns to user mode.
//!
//! Syscall numbers 0x1F0..=0x1F4 are PROVISIONAL scheduler/identity calls. 0x1F5..=0x1F7 are
//! debug calls, off unless `debug_syscalls` is set. The Batch 5 IPC numbers (0x100..) are
//! not wired: Batches 3-5 are Zig and hosted.

use crate::sched::{KERNEL_SPACE, Reason, Sched, SchedError, ThreadId};
use crate::status;
use crate::sync::SpinLock;
use crate::uaccess::{Spaces, USER_TOP, UserBacking};
use core::arch::global_asm;
use core::sync::atomic::{AtomicPtr, Ordering};

global_asm!(
    include_str!("arch/x86_64/syscall_entry.S"),
    options(att_syntax)
);

unsafe extern "C" {
    pub fn omni_syscall_entry();
    fn omni_syscall_entry_end();
}

pub const SYS_YIELD: u64 = 0x1F0;
pub const SYS_EXIT: u64 = 0x1F1;
pub const SYS_SLEEP: u64 = 0x1F2;
pub const SYS_THREAD_SELF: u64 = 0x1F3;
pub const SYS_SPACE_SELF: u64 = 0x1F4;
pub const SYS_DEBUG_CHECKSUM: u64 = 0x1F5;
pub const SYS_DEBUG_FILL: u64 = 0x1F6;
pub const SYS_DEBUG_ARGS: u64 = 0x1F7;

pub const MAX_SLEEP_TICKS: u64 = 1 << 20;
pub const DEBUG_COPY_MAX: u64 = 1024;
pub const EXIT_KILLED: i64 = -128;
pub const EXIT_BAD_FRAME: i64 = -129;

/// RFLAGS bits a user thread may carry: CF PF AF ZF SF DF OF.
pub const USER_RFLAGS_KEEP: u64 = 0xCD5;
pub const RFLAGS_IF_RESERVED: u64 = 0x202;

/// Registers saved by the entry stub. Field order is the stack layout, lowest address
/// first, and is checked against the stub by tests.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct SyscallFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub r11: u64, // user rflags (hardware)
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rcx: u64, // user rip (hardware)
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rax: u64, // in: number, out: result
    pub user_rsp: u64,
}

impl SyscallFrame {
    /// Arguments: rdi, rsi, rdx, r10, r8, r9 (rcx is clobbered by SYSCALL).
    pub fn args(&self) -> [u64; 6] {
        [self.rdi, self.rsi, self.rdx, self.r10, self.r8, self.r9]
    }
}

/// Validate and sanitise the return state. False means the thread must not return.
pub fn validate_frame(f: &mut SyscallFrame) -> bool {
    if f.rcx >= USER_TOP || f.user_rsp >= USER_TOP {
        return false;
    }
    f.r11 = (f.r11 & USER_RFLAGS_KEEP) | RFLAGS_IF_RESERVED;
    true
}

pub struct Kernel {
    pub sched: Sched,
    pub spaces: SpinLock<Spaces>,
    backing: &'static dyn UserBacking,
    debug_syscalls: bool,
}

impl Kernel {
    pub const fn new(backing: &'static dyn UserBacking, debug_syscalls: bool) -> Self {
        Kernel {
            sched: Sched::new(),
            spaces: SpinLock::new(Spaces::new()),
            backing,
            debug_syscalls,
        }
    }

    /// Entry point for one syscall. Writes the result to `f.rax`. Does not return if the
    /// thread exits, is killed, or presented an invalid frame.
    pub fn syscall(&self, f: &mut SyscallFrame) {
        let caller = match self.sched.current() {
            Some(c) => c,
            None => {
                f.rax = status::INVALID_STATE as u64;
                return;
            }
        };
        if !validate_frame(f) {
            self.sched.exit_current(EXIT_BAD_FRAME);
        }
        let nr = f.rax;
        let args = f.args();
        f.rax = self.dispatch(caller, nr, args) as u64;
        if self.sched.is_killed(caller) {
            self.sched.exit_current(EXIT_KILLED);
        }
    }

    fn dispatch(&self, caller: ThreadId, nr: u64, a: [u64; 6]) -> i64 {
        if nr > u32::MAX as u64 {
            return status::NO_SYS;
        }
        match nr {
            SYS_YIELD => match self.sched.yield_now() {
                Ok(()) => status::OK,
                Err(_) => status::INVALID_STATE,
            },
            SYS_EXIT => self.sched.exit_current(a[0] as i64),
            SYS_SLEEP => {
                if a[0] > MAX_SLEEP_TICKS {
                    return status::INVALID_ARGUMENT;
                }
                match self.sched.sleep_ticks(a[0]) {
                    Ok(Reason::Timeout) => status::OK,
                    Ok(Reason::Woken) => 1,
                    Ok(_) => status::CANCELLED,
                    Err(SchedError::InvalidState) => status::INVALID_STATE,
                    Err(_) => status::INTERNAL,
                }
            }
            SYS_THREAD_SELF => caller.raw() as i64,
            SYS_SPACE_SELF => self
                .sched
                .space_of(caller)
                .map(|s| s as i64)
                .unwrap_or(status::INTERNAL),
            SYS_DEBUG_CHECKSUM | SYS_DEBUG_FILL | SYS_DEBUG_ARGS if !self.debug_syscalls => {
                status::NO_SYS
            }
            SYS_DEBUG_CHECKSUM => self.debug_checksum(caller, a[0], a[1]),
            SYS_DEBUG_FILL => self.debug_fill(caller, a[0], a[1], a[2]),
            SYS_DEBUG_ARGS => {
                let mut sum = 0u64;
                for (i, v) in a.iter().enumerate() {
                    sum = sum.wrapping_add(v.wrapping_mul(i as u64 + 1));
                }
                (sum & 0x7FFF_FFFF_FFFF_FFFF) as i64
            }
            _ => status::NO_SYS,
        }
    }

    fn space_pair(&self, caller: ThreadId) -> Option<(u16, u16)> {
        let s = self.sched.space_of(caller)?;
        if s == KERNEL_SPACE {
            return None;
        }
        Some((s, self.sched.loaded_space()))
    }

    fn debug_checksum(&self, caller: ThreadId, addr: u64, len: u64) -> i64 {
        if len > DEBUG_COPY_MAX {
            return status::INVALID_ARGUMENT;
        }
        let Some((space, loaded)) = self.space_pair(caller) else {
            return status::ACCESS_DENIED;
        };
        let mut buf = [0u8; DEBUG_COPY_MAX as usize];
        let out = &mut buf[..len as usize];
        let spaces = self.spaces.lock();
        match spaces.copy_from_user(self.backing, space, loaded, addr, out) {
            Ok(()) => out.iter().map(|&b| b as i64).sum(),
            Err(e) => e.status(),
        }
    }

    fn debug_fill(&self, caller: ThreadId, addr: u64, len: u64, byte: u64) -> i64 {
        if len > DEBUG_COPY_MAX || byte > 255 {
            return status::INVALID_ARGUMENT;
        }
        let Some((space, loaded)) = self.space_pair(caller) else {
            return status::ACCESS_DENIED;
        };
        let buf = [byte as u8; DEBUG_COPY_MAX as usize];
        let spaces = self.spaces.lock();
        match spaces.copy_to_user(self.backing, space, loaded, addr, &buf[..len as usize]) {
            Ok(()) => status::OK,
            Err(e) => e.status(),
        }
    }
}

static KERNEL: AtomicPtr<Kernel> = AtomicPtr::new(core::ptr::null_mut());

/// Make `k` the kernel the SYSCALL stub dispatches into.
pub fn install_kernel(k: &'static Kernel) {
    KERNEL.store(k as *const Kernel as *mut Kernel, Ordering::Release);
}

/// Called by the entry stub with a pointer to the frame it pushed.
///
/// # Safety
/// `frame` must be null or point to a valid, exclusively owned `SyscallFrame`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omni_syscall_common(frame: *mut SyscallFrame) {
    if frame.is_null() {
        return;
    }
    // SAFETY: caller contract.
    let f = unsafe { &mut *frame };
    let k = KERNEL.load(Ordering::Acquire);
    if k.is_null() {
        f.rax = status::NO_SYS as u64;
        return;
    }
    // SAFETY: only `install_kernel` stores here, and it takes a 'static reference.
    unsafe { (*k).syscall(f) }
}

/// Bytes of the entry stub, for static checks.
pub fn entry_stub_bytes() -> &'static [u8] {
    let start = omni_syscall_entry as *const () as usize;
    let end = omni_syscall_entry_end as *const () as usize;
    // SAFETY: both symbols delimit the stub in the same text section.
    unsafe { core::slice::from_raw_parts(start as *const u8, end - start) }
}

/// Per-CPU block the stub addresses through %gs. Offsets are fixed by the stub.
#[repr(C)]
pub struct PerCpu {
    pub kernel_rsp: u64,
    pub user_rsp: u64,
}
