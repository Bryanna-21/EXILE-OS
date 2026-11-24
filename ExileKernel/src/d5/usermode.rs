//! x86-64 user-mode (ring 3) support: initial contexts, syscall ABI, fault policy.
//! SPDX-License-Identifier: Apache-2.0
//!
//! What is pure and host-tested here:
//! * `build_user_frame` / `validate_return_frame`: the iretq frame a thread enters or resumes
//!   user mode with. Selectors are fixed to the GDT user selectors, RFLAGS must keep IF set,
//!   IOPL/NT/VM/VIF/VIP clear, and RIP/RSP must be canonical *user* addresses
//!   (`USER_START..USER_END`). A frame that fails validation is never loaded.
//! * `star_value`/`program_syscall`: the SYSCALL/SYSRET MSR setup, derived from `gdt.rs`.
//! * The syscall ABI (`SyscallArgs`, `SyscallError`, `dispatch`, `handle_syscall`): number in
//!   RAX, arguments in RDI, RSI, RDX, R10, R8, R9, result in RAX, negative errno-style values for
//!   errors. User pointers are validated (`user_range_ok`, then the `UserMemory` page-permission
//!   check) before any byte is read or written, and data is copied through a small bounce buffer.
//! * `classify_user_fault`: which CPU exceptions from ring 3 terminate the process, which are
//!   resumed, and which indicate a kernel/system problem (fatal).
//!
//! What is bare-metal only (assembled for x86-64 but NEVER executed by the host tests):
//! `euk_enter_user` (swapgs + iretq), `euk_syscall_entry` (swapgs, stack switch, frame build,
//! sysretq/iretq), `HwUserMsr` (rdmsr/wrmsr). They are compiled out of `cfg(test)` builds.
//!
//! GS convention: while in the kernel `IA32_GS_BASE` holds the kernel's per-CPU pointer
//! (`SYSCALL_CPU`) and `IA32_KERNEL_GS_BASE` holds the user's GS base (see `tls.rs`).
//!
//! INTEGRATION REQUIREMENTS (not provided by this file, see docs): `exceptions.S` currently has
//! no `swapgs`; it must swapgs on entry from and exit to ring 3 (CS.RPL == 3), otherwise a trap
//! taken from user mode leaves the GS bases swapped the wrong way. The TSS RSP0 must be set to the
//! same kernel stack as `set_kernel_stack`. Single CPU only: `SYSCALL_CPU` is one global.
#![allow(dead_code)]

use crate::frame::{RFLAGS_DF, RFLAGS_IF, TrapFrame};
use crate::gdt::{KERNEL_CODE, USER_CODE, USER_DATA, is_canonical};

/// Lowest address user code/pointers may use (page 0 is never valid).
pub const USER_START: u64 = 0x1000;
/// One past the highest user address (exclusive). The last canonical user page is excluded so a
/// two-byte `syscall` at the top can never produce a non-canonical return RIP.
pub const USER_END: u64 = 0x0000_7FFF_FFFF_F000;

pub const RFLAGS_RESERVED1: u64 = 1 << 1;
pub const RFLAGS_TF: u64 = 1 << 8;
pub const RFLAGS_IOPL: u64 = 3 << 12;
pub const RFLAGS_NT: u64 = 1 << 14;
pub const RFLAGS_RF: u64 = 1 << 16;
pub const RFLAGS_VM: u64 = 1 << 17;
pub const RFLAGS_AC: u64 = 1 << 18;
pub const RFLAGS_VIF: u64 = 1 << 19;
pub const RFLAGS_VIP: u64 = 1 << 20;
/// RFLAGS of a fresh user context: IF set, reserved bit 1 set, everything else clear.
pub const USER_RFLAGS: u64 = RFLAGS_IF | RFLAGS_RESERVED1;
/// Bits that must be clear in any frame returned to ring 3. TF, DF, AC, ID and the arithmetic
/// flags are the user's own business and are allowed (a user may legitimately be preempted with
/// DF set); the syscall entry clears DF on its own side via FMASK.
const RFLAGS_FORBIDDEN_ON_RETURN: u64 =
    RFLAGS_IOPL | RFLAGS_NT | RFLAGS_VM | RFLAGS_VIF | RFLAGS_VIP;

pub const IA32_EFER: u32 = 0xC000_0080;
pub const IA32_STAR: u32 = 0xC000_0081;
pub const IA32_LSTAR: u32 = 0xC000_0082;
pub const IA32_FMASK: u32 = 0xC000_0084;
pub const IA32_GS_BASE: u32 = 0xC000_0101;
pub const EFER_SCE: u64 = 1;
/// RFLAGS bits cleared on SYSCALL entry: IF, TF, DF, NT, RF, AC.
pub const SYSCALL_FMASK: u64 =
    RFLAGS_IF | RFLAGS_TF | RFLAGS_DF | RFLAGS_NT | RFLAGS_RF | RFLAGS_AC;
/// Pseudo vector stored in the TrapFrame built by the syscall entry.
pub const SYSCALL_VECTOR: u64 = 0x80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserError {
    NullAddress,
    NonCanonical,
    KernelAddress,
    UnalignedStack,
    BadStack,
    BadSelector,
    BadFlags,
    BadVector,
    Msr,
}

/// True for addresses user code may execute at or pass as pointers' start.
pub const fn is_user_address(addr: u64) -> bool {
    addr >= USER_START && addr < USER_END
}

fn check_user_addr(addr: u64) -> Result<(), UserError> {
    if addr == 0 {
        Err(UserError::NullAddress)
    } else if !is_canonical(addr) {
        Err(UserError::NonCanonical)
    } else if !is_user_address(addr) {
        Err(UserError::KernelAddress)
    } else {
        Ok(())
    }
}

/// Parameters of a new user context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserContext {
    pub entry: u64,
    /// Top of the user stack: 16-byte aligned, `USER_START + 8 ..= USER_END`.
    pub stack_top: u64,
    /// Delivered in RDI.
    pub arg: u64,
}

impl UserContext {
    pub fn frame(&self) -> Result<TrapFrame, UserError> {
        build_user_frame(self.entry, self.stack_top, self.arg)
    }
}

/// Builds the iretq frame for a thread's first entry to ring 3. All other registers are zero so
/// no kernel value can leak. RSP is `stack_top - 8`, i.e. as if a `call` had pushed a return
/// address (SysV: `(rsp + 8) % 16 == 0` at function entry); that slot is the caller's to map.
pub fn build_user_frame(entry: u64, stack_top: u64, arg: u64) -> Result<TrapFrame, UserError> {
    check_user_addr(entry)?;
    if stack_top == 0 {
        return Err(UserError::NullAddress);
    }
    if !is_canonical(stack_top) {
        return Err(UserError::NonCanonical);
    }
    if stack_top > USER_END {
        return Err(UserError::KernelAddress);
    }
    if stack_top % 16 != 0 {
        return Err(UserError::UnalignedStack);
    }
    if stack_top < USER_START + 8 {
        return Err(UserError::BadStack);
    }
    let mut f = TrapFrame::default();
    f.rip = entry;
    f.cs = USER_CODE as u64;
    f.ss = USER_DATA as u64;
    f.rflags = USER_RFLAGS;
    f.rsp = stack_top - 8;
    f.rdi = arg;
    Ok(f)
}

/// Re-checks a saved user frame immediately before it is loaded with `iretq`/`sysretq`. A frame
/// that passes cannot yield ring 0, IOPL > 0, a non-canonical RIP, or a kernel-half RIP/RSP.
pub fn validate_return_frame(f: &TrapFrame) -> Result<(), UserError> {
    if f.cs != USER_CODE as u64 || f.ss != USER_DATA as u64 {
        return Err(UserError::BadSelector);
    }
    if f.rflags & RFLAGS_RESERVED1 == 0
        || f.rflags & RFLAGS_IF == 0
        || f.rflags & RFLAGS_FORBIDDEN_ON_RETURN != 0
    {
        return Err(UserError::BadFlags);
    }
    check_user_addr(f.rip)?;
    if f.rsp == 0 {
        return Err(UserError::NullAddress);
    }
    if !is_canonical(f.rsp) {
        return Err(UserError::NonCanonical);
    }
    if f.rsp < USER_START || f.rsp > USER_END {
        return Err(UserError::KernelAddress);
    }
    if f.vector > 255 {
        return Err(UserError::BadVector);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// SYSCALL / SYSRET setup

/// `IA32_STAR`: bits 47:32 = kernel CS (SYSCALL loads CS from it and SS = +8); bits 63:48 = the
/// base from which SYSRET derives user SS (+8) and 64-bit user CS (+16). The GDT order (user data
/// before user code) makes the base `USER_DATA - 8`.
pub const fn star_value() -> u64 {
    (((USER_DATA - 8) as u64) << 48) | ((KERNEL_CODE as u64) << 32)
}

/// Selectors the CPU derives from a STAR value: (syscall CS, syscall SS, sysret CS, sysret SS).
pub const fn star_selectors(star: u64) -> (u16, u16, u16, u16) {
    let k = ((star >> 32) & 0xFFFF) as u16;
    let u = ((star >> 48) & 0xFFFF) as u16;
    (k, k + 8, (u + 16) | 3, (u + 8) | 3)
}

pub trait UserMsr {
    fn rdmsr(&mut self, msr: u32) -> u64;
    fn wrmsr(&mut self, msr: u32, value: u64);
}

/// Per-CPU data read by the syscall entry through GS. Layout is fixed: offset 0 = kernel stack
/// top, offset 8 = scratch for the user RSP.
#[repr(C)]
pub struct SyscallCpu {
    kernel_rsp: core::sync::atomic::AtomicU64,
    user_rsp: core::sync::atomic::AtomicU64,
}

const _: () = assert!(core::mem::offset_of!(SyscallCpu, kernel_rsp) == 0);
const _: () = assert!(core::mem::offset_of!(SyscallCpu, user_rsp) == 8);

pub static SYSCALL_CPU: SyscallCpu = SyscallCpu {
    kernel_rsp: core::sync::atomic::AtomicU64::new(0),
    user_rsp: core::sync::atomic::AtomicU64::new(0),
};

impl SyscallCpu {
    /// Sets the stack the syscall entry switches to. Call on every context switch to a thread
    /// that can enter user mode (and keep TSS RSP0 equal to it).
    pub fn set_kernel_stack(&self, top: u64) -> Result<(), UserError> {
        if top == 0 {
            return Err(UserError::NullAddress);
        }
        if !is_canonical(top) {
            return Err(UserError::NonCanonical);
        }
        if top % 16 != 0 {
            return Err(UserError::UnalignedStack);
        }
        self.kernel_rsp
            .store(top, core::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
    pub fn kernel_stack(&self) -> u64 {
        self.kernel_rsp.load(core::sync::atomic::Ordering::Relaxed)
    }
    pub fn address(&self) -> u64 {
        self as *const Self as usize as u64
    }
}

/// Programs EFER.SCE, STAR, LSTAR, FMASK and the kernel GS base. `entry` is the address of
/// `euk_syscall_entry`; `percpu` is `SYSCALL_CPU.address()`. Verifies EFER.SCE by read-back.
pub fn program_syscall<M: UserMsr>(msr: &mut M, entry: u64, percpu: u64) -> Result<(), UserError> {
    if entry == 0 || percpu == 0 {
        return Err(UserError::NullAddress);
    }
    if !is_canonical(entry) || !is_canonical(percpu) {
        return Err(UserError::NonCanonical);
    }
    let efer = msr.rdmsr(IA32_EFER);
    msr.wrmsr(IA32_EFER, efer | EFER_SCE);
    msr.wrmsr(IA32_STAR, star_value());
    msr.wrmsr(IA32_LSTAR, entry);
    msr.wrmsr(IA32_FMASK, SYSCALL_FMASK);
    msr.wrmsr(IA32_GS_BASE, percpu);
    if msr.rdmsr(IA32_EFER) & EFER_SCE == 0 {
        return Err(UserError::Msr);
    }
    Ok(())
}

#[cfg(target_arch = "x86_64")]
pub struct HwUserMsr(());

#[cfg(target_arch = "x86_64")]
impl HwUserMsr {
    /// # Safety
    /// Ring 0 only; the MSRs used by `program_syscall` exist on every x86-64 CPU with SYSCALL.
    pub const unsafe fn new() -> Self {
        Self(())
    }
}

#[cfg(target_arch = "x86_64")]
impl UserMsr for HwUserMsr {
    fn rdmsr(&mut self, msr: u32) -> u64 {
        let (lo, hi): (u32, u32);
        // SAFETY: `HwUserMsr::new` contract.
        unsafe {
            core::arch::asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi,
                options(nomem, nostack, preserves_flags));
        }
        (hi as u64) << 32 | lo as u64
    }
    fn wrmsr(&mut self, msr: u32, value: u64) {
        // SAFETY: as above; `program_syscall` writes only validated values.
        unsafe {
            core::arch::asm!("wrmsr", in("ecx") msr, in("eax") value as u32,
                in("edx") (value >> 32) as u32, options(nomem, nostack, preserves_flags));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Syscall ABI

pub const MAX_WRITE_LEN: u64 = 4096;
pub const MAX_SLEEP_TICKS: u64 = 1 << 32;
const BOUNCE: usize = 256;

#[repr(i64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallError {
    Perm = -1,
    Again = -11,
    Fault = -14,
    Inval = -22,
    NoSys = -38,
}

#[repr(u64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sysno {
    Exit = 0,
    Yield = 1,
    SleepTicks = 2,
    GetPid = 3,
    Write = 4,
}

impl Sysno {
    pub const fn from_u64(n: u64) -> Option<Self> {
        Some(match n {
            0 => Self::Exit,
            1 => Self::Yield,
            2 => Self::SleepTicks,
            3 => Self::GetPid,
            4 => Self::Write,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyscallArgs {
    pub nr: u64,
    pub a: [u64; 6],
}

impl SyscallArgs {
    pub fn from_frame(f: &TrapFrame) -> Self {
        Self {
            nr: f.rax,
            a: [f.rdi, f.rsi, f.rdx, f.r10, f.r8, f.r9],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallOutcome {
    Return(i64),
    /// The process asked to exit; the frame must not be returned to.
    Exit(i32),
}

/// Encodes a result for RAX. Success values above `i64::MAX` would read as errors in user space
/// and are turned into `Inval` instead.
pub fn encode_result(r: Result<u64, SyscallError>) -> i64 {
    match r {
        Ok(v) if v <= i64::MAX as u64 => v as i64,
        Ok(_) => SyscallError::Inval as i64,
        Err(e) => e as i64,
    }
}

/// Kernel services the syscalls need; implemented by the process layer, faked in tests.
pub trait SyscallHost {
    fn exit(&mut self, status: i32);
    fn yield_now(&mut self) -> Result<(), SyscallError>;
    fn sleep_ticks(&mut self, ticks: u64) -> Result<(), SyscallError>;
    fn getpid(&mut self) -> u64;
    fn debug_write(&mut self, data: &[u8]) -> Result<usize, SyscallError>;
}

/// Access to the calling process's address space. `check` must verify that every page of the
/// range is mapped with user permission (and writable when `write`); the copies must fail with
/// `Fault` rather than touch anything the check would reject.
pub trait UserMemory {
    fn check(&self, start: u64, len: u64, write: bool) -> Result<(), SyscallError>;
    fn copy_from_user(&self, src: u64, dst: &mut [u8]) -> Result<(), SyscallError>;
    fn copy_to_user(&mut self, dst: u64, src: &[u8]) -> Result<(), SyscallError>;
}

/// Pure range policy applied before any `UserMemory` call: non-empty ranges must start in user
/// space and end at or below `USER_END` without wrapping.
pub fn user_range_ok(start: u64, len: u64) -> bool {
    if len == 0 {
        return true;
    }
    match start.checked_add(len) {
        Some(end) => start >= USER_START && end <= USER_END,
        None => false,
    }
}

pub fn dispatch<H: SyscallHost, M: UserMemory>(
    host: &mut H,
    mem: &M,
    args: &SyscallArgs,
) -> SyscallOutcome {
    let Some(nr) = Sysno::from_u64(args.nr) else {
        return SyscallOutcome::Return(SyscallError::NoSys as i64);
    };
    let r: Result<u64, SyscallError> = match nr {
        Sysno::Exit => {
            let status = args.a[0] as i32; // low 32 bits, like exit(2)
            host.exit(status);
            return SyscallOutcome::Exit(status);
        }
        Sysno::Yield => host.yield_now().map(|()| 0),
        Sysno::SleepTicks => {
            if args.a[0] > MAX_SLEEP_TICKS {
                Err(SyscallError::Inval)
            } else {
                host.sleep_ticks(args.a[0]).map(|()| 0)
            }
        }
        Sysno::GetPid => Ok(host.getpid()),
        Sysno::Write => sys_write(host, mem, args.a[0], args.a[1]),
    };
    SyscallOutcome::Return(encode_result(r))
}

fn sys_write<H: SyscallHost, M: UserMemory>(
    host: &mut H,
    mem: &M,
    buf: u64,
    len: u64,
) -> Result<u64, SyscallError> {
    if len > MAX_WRITE_LEN {
        return Err(SyscallError::Inval);
    }
    if len == 0 {
        return Ok(0);
    }
    if !user_range_ok(buf, len) {
        return Err(SyscallError::Fault);
    }
    mem.check(buf, len, false)?;
    let mut tmp = [0u8; BOUNCE];
    let mut done = 0u64;
    while done < len {
        let n = (len - done).min(BOUNCE as u64) as usize;
        let step = mem
            .copy_from_user(buf + done, &mut tmp[..n])
            .and_then(|()| host.debug_write(&tmp[..n]));
        match step {
            Ok(w) => {
                done += w.min(n) as u64;
                if w < n {
                    break; // sink took a short write
                }
            }
            Err(e) if done == 0 => return Err(e),
            Err(_) => break, // report the partial count, like write(2)
        }
    }
    Ok(done)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnPath {
    Sysret = 0,
    Iret = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDisposition {
    Return(ReturnPath),
    Exit(i32),
    /// The frame to return to is invalid: terminate the process, never load it.
    Kill(UserError),
}

/// Full syscall handling on the saved frame: decode, dispatch, store RAX, re-validate. `Iret` is
/// chosen instead of `Sysret` when TF is set (iretq has exact single-step semantics).
pub fn handle_syscall<H: SyscallHost, M: UserMemory>(
    frame: &mut TrapFrame,
    host: &mut H,
    mem: &M,
) -> FrameDisposition {
    if let Err(e) = validate_return_frame(frame) {
        return FrameDisposition::Kill(e);
    }
    match dispatch(host, mem, &SyscallArgs::from_frame(frame)) {
        SyscallOutcome::Exit(s) => FrameDisposition::Exit(s),
        SyscallOutcome::Return(v) => {
            frame.rax = v as u64;
            match validate_return_frame(frame) {
                Err(e) => FrameDisposition::Kill(e),
                Ok(()) if frame.rflags & RFLAGS_TF != 0 => {
                    FrameDisposition::Return(ReturnPath::Iret)
                }
                Ok(()) => FrameDisposition::Return(ReturnPath::Sysret),
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Fault policy

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultReason {
    DivideError,
    Debug,
    Overflow,
    BoundRange,
    InvalidOpcode,
    DeviceNotAvailable,
    InvalidTss,
    SegmentNotPresent,
    StackFault,
    GeneralProtection,
    PageFault {
        write: bool,
        instruction_fetch: bool,
        protection_violation: bool,
        reserved_bit: bool,
    },
    FpuError,
    AlignmentCheck,
    SimdError,
    Other(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultAction {
    /// Not the process's fault (breakpoint, NMI, external interrupt): return to it.
    Resume,
    /// Unrecoverable user fault: terminate the process. `address` is CR2 for #PF, else RIP.
    TerminateProcess { reason: FaultReason, address: u64 },
    /// Not attributable to user code (fault in the kernel, double fault, machine check, corrupt
    /// frame): the kernel must not continue as if a process had misbehaved.
    Fatal,
}

/// Maps a trap frame (and CR2, read by the caller for #PF) to a policy decision.
pub fn classify_user_fault(f: &TrapFrame, cr2: u64) -> FaultAction {
    if !f.came_from_user() || f.cs != USER_CODE as u64 {
        return FaultAction::Fatal;
    }
    let Some(v) = f.vector_u8() else {
        return FaultAction::Fatal;
    };
    let term = |reason| FaultAction::TerminateProcess {
        reason,
        address: f.rip,
    };
    match v {
        2 | 3 | 32.. => FaultAction::Resume,
        8 | 18 => FaultAction::Fatal,
        0 => term(FaultReason::DivideError),
        1 => term(FaultReason::Debug),
        4 => term(FaultReason::Overflow),
        5 => term(FaultReason::BoundRange),
        6 => term(FaultReason::InvalidOpcode),
        7 => term(FaultReason::DeviceNotAvailable),
        10 => term(FaultReason::InvalidTss),
        11 => term(FaultReason::SegmentNotPresent),
        12 => term(FaultReason::StackFault),
        13 => term(FaultReason::GeneralProtection),
        14 => {
            let e = f
                .page_fault_error()
                .unwrap_or(crate::frame::PageFaultError::decode(0));
            FaultAction::TerminateProcess {
                reason: FaultReason::PageFault {
                    write: e.write,
                    instruction_fetch: e.instruction_fetch,
                    protection_violation: e.protection_violation,
                    reserved_bit: e.reserved_bit_set,
                },
                address: cr2,
            }
        }
        16 => term(FaultReason::FpuError),
        17 => term(FaultReason::AlignmentCheck),
        19 => term(FaultReason::SimdError),
        other => term(FaultReason::Other(other)),
    }
}

// ---------------------------------------------------------------------------------------------
// Bare-metal entry/exit code (never executed on the host)

#[cfg(all(target_arch = "x86_64", not(test)))]
mod bare {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    pub type SyscallHandler = fn(&mut TrapFrame);
    pub type ReturnFailure = fn(UserError) -> !;

    static HANDLER: AtomicUsize = AtomicUsize::new(0);
    static FAILURE: AtomicUsize = AtomicUsize::new(0);

    /// Installs the syscall handler (it fills `frame.rax`, or terminates via the scheduler) and
    /// the function that terminates the process when a return frame is invalid.
    pub fn install_syscall_hooks(handler: SyscallHandler, failure: ReturnFailure) {
        HANDLER.store(handler as usize, Ordering::Release);
        FAILURE.store(failure as usize, Ordering::Release);
    }

    fn fail(e: UserError) -> ! {
        let f = FAILURE.load(Ordering::Acquire);
        if f != 0 {
            // SAFETY: only `install_syscall_hooks` stores here, with a `ReturnFailure`.
            let f: ReturnFailure = unsafe { core::mem::transmute::<usize, ReturnFailure>(f) };
            f(e)
        }
        loop {
            // SAFETY: ring 0; nothing can safely continue.
            unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
        }
    }

    /// Called by `euk_syscall_entry` on the kernel stack with the saved frame. Returns the
    /// `ReturnPath` the stub must take; an invalid frame never returns.
    #[unsafe(no_mangle)]
    extern "C" fn euk_syscall_rust(frame: *mut TrapFrame) -> u64 {
        // SAFETY: the stub passes the frame it just built on the kernel stack.
        let frame = unsafe { &mut *frame };
        let h = HANDLER.load(Ordering::Acquire);
        if h == 0 {
            frame.rax = SyscallError::NoSys as i64 as u64;
        } else {
            // SAFETY: only `install_syscall_hooks` stores here, with a `SyscallHandler`.
            let h: SyscallHandler = unsafe { core::mem::transmute::<usize, SyscallHandler>(h) };
            h(frame);
        }
        if let Err(e) = validate_return_frame(frame) {
            fail(e);
        }
        if frame.rflags & RFLAGS_TF != 0 {
            ReturnPath::Iret as u64
        } else {
            ReturnPath::Sysret as u64
        }
    }

    /// Enters ring 3 through `frame` (validated first). Interrupts are disabled until `iretq`
    /// loads the frame's RFLAGS. `frame` must be writable memory owned by the caller that stays
    /// valid until the iretq (it is used as the stack while popping).
    ///
    /// # Safety
    /// Ring 0, GDT/TSS loaded, the user address space active, `SYSCALL_CPU` kernel stack set, and
    /// the GS base convention of the module docs established.
    pub unsafe fn enter_user(frame: &TrapFrame) -> Result<core::convert::Infallible, UserError> {
        validate_return_frame(frame)?;
        // SAFETY: forwarded; the frame was validated above.
        unsafe { euk_enter_user(frame) }
    }

    unsafe extern "C" {
        fn euk_enter_user(frame: *const TrapFrame) -> !;
        pub fn euk_syscall_entry();
    }

    /// Address to program into IA32_LSTAR.
    pub fn syscall_entry_address() -> u64 {
        euk_syscall_entry as *const () as usize as u64
    }

    // Intel syntax. Frame layout = frame.rs (r15 lowest ... rax, vector, error, rip, cs, rflags,
    // rsp, ss). The syscall stub builds the same layout so Rust sees an ordinary TrapFrame.
    core::arch::global_asm!(
        ".section .text.usermode,\"ax\",@progbits",
        ".p2align 4",
        ".global euk_enter_user",
        "euk_enter_user:",
        "cli",
        "mov rsp, rdi",
        "pop r15", "pop r14", "pop r13", "pop r12", "pop r11", "pop r10", "pop r9", "pop r8",
        "pop rbp", "pop rdi", "pop rsi", "pop rdx", "pop rcx", "pop rbx", "pop rax",
        "add rsp, 16",
        "swapgs",
        "iretq",
        "",
        ".p2align 4",
        ".global euk_syscall_entry",
        "euk_syscall_entry:",
        "swapgs",
        "mov qword ptr gs:[8], rsp",
        "mov rsp, qword ptr gs:[0]",
        "push {ss}",
        "push qword ptr gs:[8]",
        "push r11",
        "push {cs}",
        "push rcx",
        "push 0",
        "push {vec}",
        "push rax", "push rbx", "push rcx", "push rdx", "push rsi", "push rdi", "push rbp",
        "push r8", "push r9", "push r10", "push r11", "push r12", "push r13", "push r14",
        "push r15",
        "mov rdi, rsp",
        "cld",
        "call euk_syscall_rust",
        "mov rbx, rax",
        "pop r15", "pop r14", "pop r13", "pop r12", "pop r11", "pop r10", "pop r9", "pop r8",
        "pop rbp", "pop rdi", "pop rsi", "pop rdx", "pop rcx",
        "add rsp, 8",           // saved rbx slot: rbx is scratch for the path flag until now
        "pop rax",
        "add rsp, 16",
        "test rbx, rbx",
        "jnz 2f",
        // sysretq path: rcx = user rip, r11 = user rflags, then the user stack.
        "mov r11, qword ptr [rsp + 16]",
        "mov rcx, qword ptr [rsp]",
        "mov rsp, qword ptr [rsp + 24]",
        "swapgs",
        "sysretq",
        "2:",
        "swapgs",
        "iretq",
        ss = const USER_DATA as u32,
        cs = const USER_CODE as u32,
        vec = const SYSCALL_VECTOR as u32,
    );
}

#[cfg(all(target_arch = "x86_64", not(test)))]
#[allow(unused_imports)]
pub use bare::{
    ReturnFailure, SyscallHandler, enter_user, install_syscall_hooks, syscall_entry_address,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::PageFaultError;
    use crate::gdt::{KERNEL_DATA, USER_CODE, USER_DATA};

    fn good() -> TrapFrame {
        build_user_frame(0x40_0000, 0x7000_0000, 7).unwrap()
    }

    #[test]
    fn initial_frame_fields() {
        let f = good();
        assert_eq!(f.rip, 0x40_0000);
        assert_eq!(f.cs, 0x23);
        assert_eq!(f.ss, 0x1b);
        assert_eq!(f.cs & 3, 3);
        assert_eq!(f.rflags, 0x202);
        assert_eq!(f.rsp, 0x7000_0000 - 8);
        assert_eq!((f.rsp + 8) % 16, 0, "SysV entry alignment");
        assert_eq!(f.rdi, 7);
        assert!(f.came_from_user());
        let mut z = f;
        (z.rip, z.cs, z.ss, z.rflags, z.rsp, z.rdi) = (0, 0, 0, 0, 0, 0);
        assert_eq!(z, TrapFrame::default(), "every other register is zero");
        validate_return_frame(&f).unwrap();
        let c = UserContext {
            entry: 0x40_0000,
            stack_top: 0x7000_0000,
            arg: 7,
        };
        assert_eq!(c.frame().unwrap(), f);
        // Boundaries.
        assert!(build_user_frame(USER_START, USER_END, 0).is_ok());
        assert!(build_user_frame(USER_END - 1, USER_START + 16 - 8 + 8, 0).is_ok());
    }

    #[test]
    fn initial_frame_rejections() {
        let e = |entry, sp| build_user_frame(entry, sp, 0).unwrap_err();
        assert_eq!(e(0, 0x7000_0000), UserError::NullAddress);
        assert_eq!(e(0x40_0000, 0), UserError::NullAddress);
        assert_eq!(e(0xfff, 0x7000_0000), UserError::KernelAddress, "page zero");
        assert_eq!(e(USER_END, 0x7000_0000), UserError::KernelAddress);
        assert_eq!(
            e(0xFFFF_8000_0010_0000, 0x7000_0000),
            UserError::KernelAddress
        );
        assert_eq!(
            e(0x0000_8000_0000_0000, 0x7000_0000),
            UserError::NonCanonical
        );
        assert_eq!(
            e(0x1234_0000_0000_0000, 0x7000_0000),
            UserError::NonCanonical
        );
        assert_eq!(e(0x40_0000, 0x0000_8000_0000_0000), UserError::NonCanonical);
        assert_eq!(
            e(0x40_0000, 0xFFFF_8000_0000_0000),
            UserError::KernelAddress
        );
        assert_eq!(e(0x40_0000, USER_END + 16), UserError::KernelAddress);
        assert_eq!(e(0x40_0000, 0x7000_0008), UserError::UnalignedStack);
        assert_eq!(e(0x40_0000, 0x7000_0001), UserError::UnalignedStack);
        assert_eq!(e(0x40_0000, 0x10), UserError::BadStack);
    }

    #[test]
    fn return_frame_validation_mutants() {
        let ok = good();
        validate_return_frame(&ok).unwrap();
        let mut f = ok;
        f.cs = KERNEL_CODE_SEL;
        assert_eq!(validate_return_frame(&f), Err(UserError::BadSelector));
        f = ok;
        f.cs = 0x20; // right GDT slot, RPL 0
        assert_eq!(validate_return_frame(&f), Err(UserError::BadSelector));
        f = ok;
        f.ss = KERNEL_DATA as u64;
        assert_eq!(validate_return_frame(&f), Err(UserError::BadSelector));
        for bit in [12u32, 13, 14, 17, 19, 20] {
            f = ok;
            f.rflags |= 1 << bit;
            assert_eq!(
                validate_return_frame(&f),
                Err(UserError::BadFlags),
                "bit {bit}"
            );
        }
        f = ok;
        f.rflags &= !RFLAGS_IF;
        assert_eq!(
            validate_return_frame(&f),
            Err(UserError::BadFlags),
            "IF must stay set"
        );
        f = ok;
        f.rflags &= !RFLAGS_RESERVED1;
        assert_eq!(validate_return_frame(&f), Err(UserError::BadFlags));
        f = ok;
        f.rip = 0x0000_8000_0000_0000;
        assert_eq!(validate_return_frame(&f), Err(UserError::NonCanonical));
        f.rip = 0xFFFF_8000_0000_1000;
        assert_eq!(validate_return_frame(&f), Err(UserError::KernelAddress));
        f.rip = 0;
        assert_eq!(validate_return_frame(&f), Err(UserError::NullAddress));
        f = ok;
        f.rsp = 0xFFFF_8000_0000_0000;
        assert_eq!(validate_return_frame(&f), Err(UserError::KernelAddress));
        f.rsp = 0x0000_8000_0000_0000;
        assert_eq!(validate_return_frame(&f), Err(UserError::NonCanonical));
        f.rsp = 0;
        assert_eq!(validate_return_frame(&f), Err(UserError::NullAddress));
        f = ok;
        f.vector = 256;
        assert_eq!(validate_return_frame(&f), Err(UserError::BadVector));
        // Allowed user-owned flags do not cause a false positive.
        f = ok;
        f.rflags |= RFLAGS_DF | RFLAGS_TF | RFLAGS_AC | 0x8D5;
        validate_return_frame(&f).unwrap();
        f.rsp = 0x7000_0003; // user may have any alignment while running
        validate_return_frame(&f).unwrap();
    }
    const KERNEL_CODE_SEL: u64 = KERNEL_CODE as u64;

    #[test]
    fn star_matches_gdt_layout_and_sysret_rules() {
        let (scs, sss, ucs, uss) = star_selectors(star_value());
        assert_eq!(scs, KERNEL_CODE);
        assert_eq!(sss, KERNEL_DATA);
        assert_eq!(ucs, USER_CODE);
        assert_eq!(uss, USER_DATA);
        assert_eq!(star_value(), 0x0013_0008_0000_0000);
        // SYSRET to 64-bit mode needs CS = base+16 and SS = base+8 with the data slot first.
        assert_eq!(USER_CODE, USER_DATA + 8);
    }

    struct FakeMsr {
        regs: std::collections::HashMap<u32, u64>,
        sce_sticks: bool,
    }
    impl UserMsr for FakeMsr {
        fn rdmsr(&mut self, msr: u32) -> u64 {
            *self.regs.get(&msr).unwrap_or(&0)
        }
        fn wrmsr(&mut self, msr: u32, v: u64) {
            let v = if msr == IA32_EFER && !self.sce_sticks {
                v & !EFER_SCE
            } else {
                v
            };
            self.regs.insert(msr, v);
        }
    }

    #[test]
    fn syscall_msr_programming() {
        let mut m = FakeMsr {
            regs: [(IA32_EFER, 0x500)].into(),
            sce_sticks: true,
        };
        program_syscall(&mut m, 0xFFFF_8000_0020_0000, 0xFFFF_8000_0030_0000).unwrap();
        assert_eq!(m.regs[&IA32_EFER], 0x501, "SCE added, LME/LMA kept");
        assert_eq!(m.regs[&IA32_STAR], star_value());
        assert_eq!(m.regs[&IA32_LSTAR], 0xFFFF_8000_0020_0000);
        assert_eq!(m.regs[&IA32_FMASK], SYSCALL_FMASK);
        assert_eq!(
            SYSCALL_FMASK & RFLAGS_IF,
            RFLAGS_IF,
            "interrupts masked in the entry window"
        );
        assert_eq!(m.regs[&IA32_GS_BASE], 0xFFFF_8000_0030_0000);
        let mut m = FakeMsr {
            regs: Default::default(),
            sce_sticks: false,
        };
        assert_eq!(
            program_syscall(&mut m, 0x10_0000, 0x20_0000),
            Err(UserError::Msr)
        );
        let mut m = FakeMsr {
            regs: Default::default(),
            sce_sticks: true,
        };
        assert_eq!(program_syscall(&mut m, 0, 1), Err(UserError::NullAddress));
        assert_eq!(
            program_syscall(&mut m, 1 << 60, 1),
            Err(UserError::NonCanonical)
        );
        assert!(m.regs.is_empty(), "nothing written on rejection");
    }

    #[test]
    fn syscall_cpu_kernel_stack_validation() {
        let c = SyscallCpu {
            kernel_rsp: 0.into(),
            user_rsp: 0.into(),
        };
        assert_eq!(c.set_kernel_stack(0), Err(UserError::NullAddress));
        assert_eq!(c.set_kernel_stack(1 << 63), Err(UserError::NonCanonical));
        assert_eq!(c.set_kernel_stack(0x1008), Err(UserError::UnalignedStack));
        c.set_kernel_stack(0xFFFF_8000_0001_0000).unwrap();
        assert_eq!(c.kernel_stack(), 0xFFFF_8000_0001_0000);
    }

    // ---- syscall dispatch ----

    #[derive(Default)]
    struct Host {
        exited: Option<i32>,
        yields: u32,
        slept: Vec<u64>,
        sink: Vec<u8>,
        sink_cap: Option<usize>,
        fail_sink_after: Option<usize>,
        chunks: usize,
    }
    impl SyscallHost for Host {
        fn exit(&mut self, s: i32) {
            self.exited = Some(s);
        }
        fn yield_now(&mut self) -> Result<(), SyscallError> {
            self.yields += 1;
            Ok(())
        }
        fn sleep_ticks(&mut self, t: u64) -> Result<(), SyscallError> {
            self.slept.push(t);
            Ok(())
        }
        fn getpid(&mut self) -> u64 {
            42
        }
        fn debug_write(&mut self, d: &[u8]) -> Result<usize, SyscallError> {
            self.chunks += 1;
            if self.fail_sink_after.is_some_and(|n| self.chunks > n) {
                return Err(SyscallError::Again);
            }
            let n = self.sink_cap.map_or(d.len(), |c| c.min(d.len()));
            self.sink.extend_from_slice(&d[..n]);
            Ok(n)
        }
    }

    /// One mapped user region [base, base+len); read-only when `ro`.
    struct Mem {
        base: u64,
        bytes: Vec<u8>,
        ro: bool,
        copies: std::cell::Cell<u32>,
    }
    impl Mem {
        fn new(len: usize) -> Self {
            let mut bytes = vec![0u8; len];
            for (i, b) in bytes.iter_mut().enumerate() {
                *b = (i % 251) as u8;
            }
            Self {
                base: 0x10_0000,
                bytes,
                ro: false,
                copies: 0.into(),
            }
        }
        fn contains(&self, s: u64, l: u64) -> bool {
            s >= self.base
                && s.checked_add(l)
                    .is_some_and(|e| e <= self.base + self.bytes.len() as u64)
        }
    }
    impl UserMemory for Mem {
        fn check(&self, s: u64, l: u64, write: bool) -> Result<(), SyscallError> {
            if !self.contains(s, l) || (write && self.ro) {
                return Err(SyscallError::Fault);
            }
            Ok(())
        }
        fn copy_from_user(&self, s: u64, d: &mut [u8]) -> Result<(), SyscallError> {
            self.copies.set(self.copies.get() + 1);
            self.check(s, d.len() as u64, false)?;
            let o = (s - self.base) as usize;
            d.copy_from_slice(&self.bytes[o..o + d.len()]);
            Ok(())
        }
        fn copy_to_user(&mut self, d: u64, s: &[u8]) -> Result<(), SyscallError> {
            self.check(d, s.len() as u64, true)?;
            let o = (d - self.base) as usize;
            self.bytes[o..o + s.len()].copy_from_slice(s);
            Ok(())
        }
    }

    fn call(h: &mut Host, m: &Mem, nr: u64, a: [u64; 6]) -> SyscallOutcome {
        dispatch(h, m, &SyscallArgs { nr, a })
    }
    fn ret(o: SyscallOutcome) -> i64 {
        match o {
            SyscallOutcome::Return(v) => v,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn args_decode_from_frame_registers() {
        let mut f = good();
        (f.rax, f.rdi, f.rsi, f.rdx, f.r10, f.r8, f.r9) = (4, 1, 2, 3, 4, 5, 6);
        let a = SyscallArgs::from_frame(&f);
        assert_eq!((a.nr, a.a), (4, [1, 2, 3, 4, 5, 6]));
    }

    #[test]
    fn dispatch_table_basics() {
        let (mut h, m) = (Host::default(), Mem::new(64));
        assert_eq!(ret(call(&mut h, &m, Sysno::GetPid as u64, [0; 6])), 42);
        assert_eq!(ret(call(&mut h, &m, Sysno::Yield as u64, [0; 6])), 0);
        assert_eq!(h.yields, 1);
        assert_eq!(
            ret(call(
                &mut h,
                &m,
                Sysno::SleepTicks as u64,
                [5, 0, 0, 0, 0, 0]
            )),
            0
        );
        assert_eq!(h.slept, [5]);
        assert_eq!(
            ret(call(
                &mut h,
                &m,
                Sysno::SleepTicks as u64,
                [MAX_SLEEP_TICKS + 1, 0, 0, 0, 0, 0]
            )),
            SyscallError::Inval as i64
        );
        assert_eq!(h.slept, [5], "oversized sleep never reached the host");
        for bad in [5, 99, u64::MAX] {
            assert_eq!(
                ret(call(&mut h, &m, bad, [0; 6])),
                SyscallError::NoSys as i64
            );
        }
        assert_eq!(
            call(
                &mut h,
                &m,
                Sysno::Exit as u64,
                [0x1_0000_0007, 0, 0, 0, 0, 0]
            ),
            SyscallOutcome::Exit(7)
        );
        assert_eq!(h.exited, Some(7));
    }

    #[test]
    fn write_copies_through_validated_pointers() {
        let (mut h, m) = (Host::default(), Mem::new(1000));
        assert_eq!(
            ret(call(&mut h, &m, 4, [m.base + 10, 600, 0, 0, 0, 0])),
            600
        );
        assert_eq!(h.sink, m.bytes[10..610]);
        assert_eq!(h.chunks, 3, "bounce buffer used");
        // Zero length succeeds without touching memory, even with a null pointer.
        let before = m.copies.get();
        assert_eq!(ret(call(&mut h, &m, 4, [0, 0, 0, 0, 0, 0])), 0);
        assert_eq!(m.copies.get(), before);
    }

    #[test]
    fn invalid_user_pointers_fault_without_reading() {
        let m = Mem::new(1000);
        let fault = SyscallError::Fault as i64;
        let cases: &[(u64, u64)] = &[
            (0, 1),                      // null
            (8, 4),                      // page zero
            (0xFFFF_8000_0000_0000, 16), // kernel half
            (0x0000_8000_0000_0000, 16), // non-canonical
            (u64::MAX - 7, 16),          // wraps
            (USER_END - 8, 16),          // straddles the user limit
            (USER_END, 1),
            (m.base + 990, 100), // runs past the mapped region (page check)
            (m.base - 1, 4),     // starts before it
        ];
        for &(p, l) in cases {
            let mut h = Host::default();
            assert_eq!(
                ret(call(&mut h, &m, 4, [p, l, 0, 0, 0, 0])),
                fault,
                "ptr {p:#x} len {l}"
            );
            assert!(h.sink.is_empty());
        }
        assert_eq!(
            m.copies.get(),
            0,
            "no copy was attempted for any invalid range"
        );
        let mut h = Host::default();
        assert_eq!(
            ret(call(&mut h, &m, 4, [m.base, MAX_WRITE_LEN + 1, 0, 0, 0, 0])),
            SyscallError::Inval as i64
        );
        assert_eq!(
            ret(call(&mut h, &m, 4, [m.base, u64::MAX, 0, 0, 0, 0])),
            SyscallError::Inval as i64
        );
        // Unaligned pointers are fine.
        assert_eq!(ret(call(&mut h, &m, 4, [m.base + 3, 5, 0, 0, 0, 0])), 5);
    }

    #[test]
    fn write_partial_and_short_sink_behaviour() {
        let m = Mem::new(1000);
        let mut h = Host {
            fail_sink_after: Some(1),
            ..Default::default()
        };
        // First chunk (256) succeeds, second fails: the partial count is reported.
        assert_eq!(ret(call(&mut h, &m, 4, [m.base, 600, 0, 0, 0, 0])), 256);
        let mut h = Host {
            fail_sink_after: Some(0),
            ..Default::default()
        };
        assert_eq!(
            ret(call(&mut h, &m, 4, [m.base, 10, 0, 0, 0, 0])),
            SyscallError::Again as i64
        );
        let mut h = Host {
            sink_cap: Some(100),
            ..Default::default()
        };
        assert_eq!(
            ret(call(&mut h, &m, 4, [m.base, 600, 0, 0, 0, 0])),
            100,
            "short write stops"
        );
    }

    #[test]
    fn result_encoding_never_looks_like_an_error_by_accident() {
        assert_eq!(encode_result(Ok(5)), 5);
        assert_eq!(encode_result(Ok(i64::MAX as u64)), i64::MAX);
        assert_eq!(encode_result(Ok(1 << 63)), SyscallError::Inval as i64);
        assert_eq!(encode_result(Err(SyscallError::Perm)), -1);
        assert_eq!(SyscallError::Fault as i64, -14);
    }

    #[test]
    fn handle_syscall_sets_rax_and_revalidates() {
        let m = Mem::new(64);
        let mut h = Host::default();
        let mut f = good();
        f.rax = Sysno::GetPid as u64;
        assert_eq!(
            handle_syscall(&mut f, &mut h, &m),
            FrameDisposition::Return(ReturnPath::Sysret)
        );
        assert_eq!(f.rax, 42);
        f.rax = Sysno::GetPid as u64;
        f.rflags |= RFLAGS_TF;
        assert_eq!(
            handle_syscall(&mut f, &mut h, &m),
            FrameDisposition::Return(ReturnPath::Iret)
        );
        f.rax = Sysno::Exit as u64;
        f.rdi = 3;
        assert_eq!(
            handle_syscall(&mut f, &mut h, &m),
            FrameDisposition::Exit(3)
        );
        // A corrupted saved frame is never returned to.
        let mut bad = good();
        bad.cs = 0x08;
        assert_eq!(
            handle_syscall(&mut bad, &mut h, &m),
            FrameDisposition::Kill(UserError::BadSelector)
        );
        let mut bad = good();
        bad.rip = 0x0000_8000_0000_0000; // syscall at the very top of user space
        assert_eq!(
            handle_syscall(&mut bad, &mut h, &m),
            FrameDisposition::Kill(UserError::NonCanonical)
        );
    }

    // ---- fault classification ----

    fn trap(vector: u64, code: u64, user: bool) -> TrapFrame {
        let mut f = good();
        f.vector = vector;
        f.error_code = code;
        f.rip = 0x40_1234;
        if !user {
            f.cs = KERNEL_CODE as u64;
            f.ss = KERNEL_DATA as u64;
        }
        f
    }

    #[test]
    fn user_faults_terminate_the_process() {
        use FaultAction::*;
        let t = |v, c| classify_user_fault(&trap(v, c, true), 0xdead_b000);
        assert_eq!(
            t(6, 0),
            TerminateProcess {
                reason: FaultReason::InvalidOpcode,
                address: 0x40_1234
            }
        );
        assert_eq!(
            t(13, 0),
            TerminateProcess {
                reason: FaultReason::GeneralProtection,
                address: 0x40_1234
            }
        );
        assert_eq!(
            t(0, 0),
            TerminateProcess {
                reason: FaultReason::DivideError,
                address: 0x40_1234
            }
        );
        assert_eq!(
            t(12, 0).clone(),
            TerminateProcess {
                reason: FaultReason::StackFault,
                address: 0x40_1234
            }
        );
        assert_eq!(
            t(17, 0),
            TerminateProcess {
                reason: FaultReason::AlignmentCheck,
                address: 0x40_1234
            }
        );
        assert_eq!(
            t(9, 0),
            TerminateProcess {
                reason: FaultReason::Other(9),
                address: 0x40_1234
            }
        );
        // #PF: user write to a present (read-only) page, and a not-present instruction fetch.
        assert_eq!(
            t(14, 0b00111),
            TerminateProcess {
                reason: FaultReason::PageFault {
                    write: true,
                    instruction_fetch: false,
                    protection_violation: true,
                    reserved_bit: false
                },
                address: 0xdead_b000
            }
        );
        let pf = PageFaultError::decode(0b10100);
        assert!(pf.instruction_fetch && pf.user);
        assert_eq!(
            t(14, 0b10100),
            TerminateProcess {
                reason: FaultReason::PageFault {
                    write: false,
                    instruction_fetch: true,
                    protection_violation: false,
                    reserved_bit: false
                },
                address: 0xdead_b000
            }
        );
    }

    #[test]
    fn non_process_faults_and_kernel_faults() {
        use FaultAction::*;
        // Same exceptions from the kernel (CPL 0) are never blamed on a process.
        for v in [0u64, 6, 13, 14, 3, 32, 0x80] {
            assert_eq!(
                classify_user_fault(&trap(v, 0, false), 0),
                Fatal,
                "vector {v}"
            );
        }
        let u = |v| classify_user_fault(&trap(v, 0, true), 0);
        assert_eq!(u(3), Resume, "breakpoint");
        assert_eq!(u(2), Resume, "NMI is not the process's fault");
        assert_eq!(u(32), Resume);
        assert_eq!(u(255), Resume);
        assert_eq!(u(8), Fatal, "double fault");
        assert_eq!(u(18), Fatal, "machine check");
        // Corrupt frames: out-of-range vector, RPL 3 with a non-user selector.
        let mut f = trap(13, 0, true);
        f.vector = 300;
        assert_eq!(classify_user_fault(&f, 0), Fatal);
        let mut f = trap(13, 0, true);
        f.cs = 0x1b; // RPL 3 but the data slot
        assert_eq!(classify_user_fault(&f, 0), Fatal);
    }

    #[test]
    fn user_range_policy() {
        assert!(user_range_ok(0, 0));
        assert!(!user_range_ok(0, 1));
        assert!(user_range_ok(USER_START, 1));
        assert!(user_range_ok(USER_END - 1, 1));
        assert!(!user_range_ok(USER_END - 1, 2));
        assert!(!user_range_ok(u64::MAX, 2));
        assert!(!user_range_ok(1 << 63, 1));
    }
}
