//! Build script for the OMNI-Kernel Batch 4 hosted test harness.
//!
//! Builds ONLY hosted test executables for the machine running the build. It
//! builds no kernel image, no boot code and no freestanding target.
//!
//! Batch 4 reuses Batch 2 and Batch 3 WITHOUT modifying them:
//!   * a WriteFile step copies `../OMNI-Kernel-Batch-2/src/core` and `src/batch2.zig`
//!     into a generated directory, which is the module `batch2`;
//!   * `../OMNI-Kernel-Batch-3/src/kernel.zig` is the module `kernel3`;
//!   * the Batch 3 and Batch 2 test suites are compiled from their own,
//!     unmodified root files as regression checks.
//!
//! Steps:
//!   zig build test    run the Batch 4 tests AND the Batch 3 and Batch 2 regression tests
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

    const kernel3 = b.createModule(.{
        .root_source_file = b.path("../OMNI-Kernel-Batch-3/src/kernel.zig"),
        .target = target,
        .optimize = optimize,
    });
    kernel3.addImport("batch2", batch2);

    const t4_module = b.createModule(.{
        .root_source_file = b.path("src/tests.zig"),
        .target = target,
        .optimize = optimize,
    });
    t4_module.addImport("batch2", batch2);
    t4_module.addImport("kernel3", kernel3);
    const t4 = b.addTest(.{ .root_module = t4_module });
    const run_t4 = b.addRunArtifact(t4);

    const t3_module = b.createModule(.{
        .root_source_file = b.path("../OMNI-Kernel-Batch-3/src/tests.zig"),
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

    const test_step = b.step("test", "Run Batch 4 tests and the Batch 3 and Batch 2 regression tests");
    test_step.dependOn(&run_t4.step);
    test_step.dependOn(&run_t3.step);
    test_step.dependOn(&run_t2.step);

    const fmt_step = b.step("fmt", "Check source formatting");
    const fmt = b.addFmt(.{
        .paths = &.{ "build.zig", "build.zig.zon", "src" },
        .check = true,
    });
    fmt_step.dependOn(&fmt.step);
}
