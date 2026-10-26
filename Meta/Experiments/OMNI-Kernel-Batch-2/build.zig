//! Build script for the OMNI-Kernel Batch 2 hosted test harness.
//!
//! Targets the Zig version recorded in build.zig.zon (`minimum_zig_version`) and
//! in README.md / REPORT.md. It builds ONLY a hosted test executable for the
//! machine running the build. It builds no kernel image, no boot code and no
//! freestanding target.
//!
//! Steps:
//!   zig build test    compile and run all tests in src/tests.zig
//!   zig build fmt     check formatting (fails if any file needs reformatting)

const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const test_module = b.createModule(.{
        .root_source_file = b.path("src/tests.zig"),
        .target = target,
        .optimize = optimize,
    });
    const tests = b.addTest(.{ .root_module = test_module });
    const run_tests = b.addRunArtifact(tests);

    const test_step = b.step("test", "Run the hosted test suite");
    test_step.dependOn(&run_tests.step);

    const fmt_step = b.step("fmt", "Check source formatting");
    const fmt = b.addFmt(.{
        .paths = &.{ "build.zig", "build.zig.zon", "src" },
        .check = true,
    });
    fmt_step.dependOn(&fmt.step);
}
