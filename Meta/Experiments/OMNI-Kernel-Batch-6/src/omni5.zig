//! Re-exports of the unmodified Batch 5 sources used by Batch 6.
//!
//! `build.zig` copies the listed files from `../OMNI-Kernel-Batch-5/src` next to this
//! file at build time (the same technique Batch 4 uses for the Batch 2 core). The
//! copies are byte-identical to the originals; nothing in Batch 5 is modified.

pub const kernel_ipc = @import("kernel_ipc.zig");
pub const syscall = @import("syscall.zig");
pub const wait_queue = @import("wait_queue.zig");
pub const accounting = @import("accounting.zig");
pub const hosted_user_memory = @import("hosted_user_memory.zig");
