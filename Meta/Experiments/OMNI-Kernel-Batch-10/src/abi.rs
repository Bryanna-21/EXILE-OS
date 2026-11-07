//! Versioned syscall layer (SEC-18) and register-convention adaptor (SEC-20).
//!
//! * `SYS_ABI_QUERY` returns the kernel ABI version and lets a program require a major version.
//! * `AbiMode` selects how the six syscall registers are read: the OMNI convention (rax number;
//!   rdi, rsi, rdx, r10, r8, r9) or the convention of the existing `Kernel/` (rax number; rdx, rdi,
//!   rbx, rsi). The adaptor converts only the REGISTER convention. It does not translate syscall
//!   NUMBERS: `Kernel/` numbers come from its own enumeration and have no OMNI equivalents.
//!
//! Which convention the real system uses is an owner decision (docs/ARCHITECTURE_DECISION.md).
//! Hosted-tested only; no syscall instruction was executed.

use omni_kernel7::sched::ThreadId;
use omni_kernel7::status;
use omni_kernel7::syscall::{EXIT_BAD_FRAME, EXIT_KILLED, Kernel, SyscallFrame, validate_frame};

pub const ABI_MAJOR: u16 = 1;
pub const ABI_MINOR: u16 = 0;
/// Free in Batch 7 (0x1F0..=0x1F7 used) and Batch 5 (0x100..=0x107 used).
pub const SYS_ABI_QUERY: u64 = 0x1F8;
/// Version word returned by a query: `major << 16 | minor`.
pub const fn version_word() -> u64 {
    ((ABI_MAJOR as u64) << 16) | ABI_MINOR as u64
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AbiMode {
    /// rax; rdi, rsi, rdx, r10, r8, r9 (OMNI Batches 5 to 7).
    Omni,
    /// rax; rdx, rdi, rbx, rsi (existing `Kernel/`); only four arguments exist.
    Exile,
}

/// Copy of `f` with the arguments moved into the OMNI registers.
pub fn to_omni(mode: AbiMode, f: &SyscallFrame) -> SyscallFrame {
    match mode {
        AbiMode::Omni => *f,
        AbiMode::Exile => {
            let mut g = *f;
            g.rdi = f.rdx; // arg1
            g.rsi = f.rdi; // arg2
            g.rdx = f.rbx; // arg3
            g.r10 = f.rsi; // arg4
            g.r8 = 0;
            g.r9 = 0;
            g
        }
    }
}

pub struct AbiKernel<'k> {
    pub kernel: &'k Kernel,
}

impl<'k> AbiKernel<'k> {
    pub const fn new(kernel: &'k Kernel) -> Self {
        Self { kernel }
    }

    /// Entry point: like `Kernel::syscall`, but argument registers are interpreted per `mode`
    /// and the user's argument registers are left untouched. Only `rax` (result) and `r11`
    /// (sanitised flags) are written back.
    pub fn syscall(&self, mode: AbiMode, f: &mut SyscallFrame) {
        let mut g = to_omni(mode, f);
        if g.rax == SYS_ABI_QUERY {
            self.query(&mut g);
        } else {
            self.kernel.syscall(&mut g);
        }
        f.rax = g.rax;
        f.r11 = g.r11;
    }

    /// `rdi` = required major version (0 = just ask), `rsi` = flags (must be 0).
    /// Result: version word, or INVALID_ARGUMENT for a reserved flag or a major mismatch.
    fn query(&self, g: &mut SyscallFrame) {
        let k = self.kernel;
        let caller: ThreadId = match k.sched.current() {
            Some(c) => c,
            None => {
                g.rax = status::INVALID_STATE as u64;
                return;
            }
        };
        if !validate_frame(g) {
            k.sched.exit_current(EXIT_BAD_FRAME);
        }
        let [want_major, flags, ..] = g.args();
        g.rax = if flags != 0 || (want_major != 0 && want_major != ABI_MAJOR as u64) {
            status::INVALID_ARGUMENT as u64
        } else {
            version_word()
        };
        if k.sched.is_killed(caller) {
            k.sched.exit_current(EXIT_KILLED);
        }
    }
}
