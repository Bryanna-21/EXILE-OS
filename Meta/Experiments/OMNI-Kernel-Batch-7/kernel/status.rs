//! Status codes. They are the codes of Batch 2 / Batch 5 (provisional), so a later
//! integration of the IPC layer does not renumber anything.

pub const OK: i64 = 0;
pub const INVALID_ARGUMENT: i64 = -1;
pub const INVALID_HANDLE: i64 = -2;
pub const ACCESS_DENIED: i64 = -3;
pub const QUOTA_EXCEEDED: i64 = -6;
pub const TIMED_OUT: i64 = -8;
pub const CANCELLED: i64 = -9;
pub const BUSY: i64 = -14;
pub const INTERNAL: i64 = -16;
pub const INVALID_STATE: i64 = -17;
pub const FAULT: i64 = -32;
pub const NO_SYS: i64 = -33;
