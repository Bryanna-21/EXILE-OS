//! Re-exports of the unmodified Batch 2 core modules used by Batch 3 and Batch 4.
//!
//! `build.zig` copies `../OMNI-Kernel-Batch-2/src/core` next to this file at build
//! time. Batch 3's `kernel.zig` imports a module named `batch2`; it is satisfied
//! by this superset (Batch 3 needs `err`, `ids`, `caps`, `res`). Batch 4 also
//! uses the Batch 2 wire format in `ipc`.

pub const err = @import("core/error.zig");
pub const ids = @import("core/ids.zig");
pub const caps = @import("core/capabilities.zig");
pub const res = @import("core/resources.zig");
pub const ipc = @import("core/ipc.zig");
