//! Batch 5 test root.
test {
    _ = @import("tests/units.zig");
    _ = @import("tests/syscall_tests.zig");
    _ = @import("tests/wait_tests.zig");
    _ = @import("tests/resource_tests.zig");
    _ = @import("tests/stress_tests.zig");
}
