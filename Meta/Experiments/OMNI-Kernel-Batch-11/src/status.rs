//! Wire status codes. Numbers are the Batch 2 `Status` values (checked by scripts/check_ipc_conformance.py);
//! FAULT and NO_SYS come from Batch 5.

pub const OK: i64 = 0;
pub const INVALID_ARGUMENT: i64 = -1;
pub const INVALID_HANDLE: i64 = -2;
pub const ACCESS_DENIED: i64 = -3;
pub const QUOTA_EXCEEDED: i64 = -6;
pub const WOULD_BLOCK: i64 = -7;
pub const TIMED_OUT: i64 = -8;
pub const CANCELLED: i64 = -9;
pub const PEER_DEAD: i64 = -10;
pub const NOT_SUPPORTED: i64 = -11;
pub const BAD_VERSION: i64 = -12;
pub const BUSY: i64 = -14;
pub const INTERNAL: i64 = -16;
pub const INVALID_STATE: i64 = -17;
pub const FAULT: i64 = -32;
pub const NO_SYS: i64 = -33;
