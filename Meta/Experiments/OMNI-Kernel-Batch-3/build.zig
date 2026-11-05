//! Build script for the OMNI-Kernel Batch 3 hosted test harness.
//!
//! Builds ONLY hosted test executables for the machine running the build. It
//! builds no kernel image, no boot code and no freestanding target.
//!
//! Batch 3 reuses the Batch 2 core modules WITHOUT modifying them: a WriteFile step
//! copies `../OMNI-Kernel-Batch-2/src/core` and `src/batch2.zig` into a generated
//! directory, and that directory is the module `batch2`. The Batch 2 test suite is
//! compiled from its own, unmodified root file as a regression check.
//!
//! Steps:
//!   zig build test    run the Batch 3 tests AND the unmodified Batch 2 regression tests
//!   zig build fmt     check formatting of this package's Zig sources

const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const wf = b.addWriteFiles();
    _ = wf.addCopyDirectory(b.path("../OMNI-Kernel-Batch-2/src/core"), "core", .{});
    const batch2_root = wf.addCopyFile(b.path("src/batch2.zig"), "batch2.zig");
    const batch2 = b.createModule(.{
        .root_source_file = batch2_root,
        .target = target,
        .optimize = optimize,
    });

    const t3_module = b.createModule(.{
        .root_source_file = b.path("src/tests.zig"),
        .target = target,
        .optimize = optimize,
    });
    t3_module.addImport("batch2", batch2);
    const t3 = b.addTest(.{ .root_module = t3_module });
    const run_t3 = b.addRunArtifact(t3);

    const t2_module = b.createModule(.{
        .root_source_file = b.path("../OMNI-Kernel-Batch-2/src/tests.zig"),
        .target = target,
        .optimize = optimize,
    });
    const t2 = b.addTest(.{ .root_module = t2_module });
    const run_t2 = b.addRunArtifact(t2);

    const test_step = b.step("test", "Run Batch 3 tests and the Batch 2 regression tests");
    test_step.dependOn(&run_t3.step);
    test_step.dependOn(&run_t2.step);

    const fmt_step = b.step("fmt", "Check source formatting");
    const fmt = b.addFmt(.{
        .paths = &.{ "build.zig", "build.zig.zon", "src" },
        .check = true,
    });
    fmt_step.dependOn(&fmt.step);
}
