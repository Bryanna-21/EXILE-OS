//! OMNI-Kernel Batch 3 test root.

test {
    _ = @import("tests/arena_tests.zig");
    _ = @import("tests/lifecycle_tests.zig");
    _ = @import("tests/revocation_tests.zig");
    _ = @import("tests/accounting_tests.zig");
    _ = @import("tests/alloc_tests.zig");
    _ = @import("tests/concurrency_tests.zig");
    _ = @import("tests/model_tests.zig");
}
