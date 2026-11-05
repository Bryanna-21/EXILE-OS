//! Batch 4 test root.
test {
    _ = @import("tests/basic.zig");
    _ = @import("tests/caps.zig");
    _ = @import("tests/wait.zig");
    _ = @import("tests/stress.zig");
}
