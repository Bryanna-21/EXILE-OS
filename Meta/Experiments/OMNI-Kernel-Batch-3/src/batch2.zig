//! Re-exports of the unmodified Batch 2 core modules.
//!
//! `build.zig` copies `../OMNI-Kernel-Batch-2/src/core` next to this file at build
//! time (see build.zig), so Batch 3 reuses Batch 2 without modifying or duplicating
//! it. Only the modules Batch 3 needs are exported.

pub const err = @import("core/error.zig");
pub const ids = @import("core/ids.zig");
pub const caps = @import("core/capabilities.zig");
pub const res = @import("core/resources.zig");
