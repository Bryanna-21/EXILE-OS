//! Ring-3 entry: validation of the entry state and construction of the `iretq` frame.

use crate::desc::{SEL_USER_CODE, SEL_USER_DATA};
use crate::is_canonical;
use crate::paging::UserWindow;

/// IF set, reserved bit 1 set, IOPL 0, everything else clear.
pub const USER_RFLAGS: u64 = 0x202;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UserEntryError {
    NonCanonical,
    OutsideWindow,
    MisalignedStack,
    NullStack,
}

/// `iretq` pops, lowest address first: RIP, CS, RFLAGS, RSP, SS.
pub type IretFrame = [u64; 5];

/// Validate and build the frame that drops to CPL 3 at `rip` with stack pointer `rsp`.
/// `rip` must lie in the user window; `rsp` must be 8-aligned and `rsp - 1` must be in the
/// window (the stack grows down from `rsp`).
pub fn iret_frame(rip: u64, rsp: u64, window: UserWindow) -> Result<IretFrame, UserEntryError> {
    if !is_canonical(rip) || !is_canonical(rsp) {
        return Err(UserEntryError::NonCanonical);
    }
    if rsp == 0 {
        return Err(UserEntryError::NullStack);
    }
    if rsp % 8 != 0 {
        return Err(UserEntryError::MisalignedStack);
    }
    if !window.contains(rip, 1) || !window.contains(rsp - 1, 1) {
        return Err(UserEntryError::OutsideWindow);
    }
    Ok([
        rip,
        SEL_USER_CODE as u64,
        USER_RFLAGS,
        rsp,
        SEL_USER_DATA as u64,
    ])
}
