//! Batch 6 test root. Every test is HOSTED; tests of simulated hardware-facing behaviour
//! say so in their names (SIM) and in docs/TEST-PLAN.md.
test {
    _ = @import("tests/arch_tests.zig");
    _ = @import("tests/sched_tests.zig");
    _ = @import("tests/ipc_tests.zig");
    _ = @import("tests/entry_tests.zig");
    _ = @import("tests/mem_tests.zig");
    _ = @import("tests/proc_tests.zig");
}
