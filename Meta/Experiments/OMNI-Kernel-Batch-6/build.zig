//! Build script for the OMNI-Kernel Batch 6 hosted test harness.
//!
//! Builds ONLY hosted test executables for the machine running the build. It
//! builds no kernel image, no boot code and no freestanding target.
//!
//! Batch 6 reuses Batches 2, 3 and 4 WITHOUT modifying them:
//!   * a WriteFile step copies `../OMNI-Kernel-Batch-2/src/core` and Batch 4's
//!     `src/batch2.zig` into a generated directory: the module `batch2`;
//!   * `../OMNI-Kernel-Batch-3/src/kernel.zig` is the module `kernel3`;
//!   * `../OMNI-Kernel-Batch-4/src/ipc.zig` is the module `ipc4`;
//!   * a WriteFile step copies the five Batch 5 sources the dispatcher needs from
//!     `../OMNI-Kernel-Batch-5/src` next to `src/omni5.zig`: the module `omni5`;
//!   * the Batch 5, 4, 3 and 2 test suites are compiled from their own, unmodified
//!     root files as regression checks.
//!
//! Steps:
//!   zig build test    run the Batch 6 tests AND the Batch 5, 4, 3 and 2 regression tests
//!   zig build fmt     check formatting of this package's Zig sources

const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const wf = b.addWriteFiles();
    _ = wf.addCopyDirectory(b.path("../OMNI-Kernel-Batch-2/src/core"), "core", .{});
    const batch2_root = wf.addCopyFile(b.path("../OMNI-Kernel-Batch-4/src/batch2.zig"), "batch2.zig");
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

    const ipc4 = b.createModule(.{
        .root_source_file = b.path("../OMNI-Kernel-Batch-4/src/ipc.zig"),
        .target = target,
        .optimize = optimize,
    });
    ipc4.addImport("batch2", batch2);
    ipc4.addImport("kernel3", kernel3);

    const wf5 = b.addWriteFiles();
    inline for (.{ "kernel_ipc.zig", "syscall.zig", "wait_queue.zig", "accounting.zig", "hosted_user_memory.zig" }) |name| {
        _ = wf5.addCopyFile(b.path("../OMNI-Kernel-Batch-5/src/" ++ name), name);
    }
    const omni5_root = wf5.addCopyFile(b.path("src/omni5.zig"), "omni5.zig");
    const omni5 = b.createModule(.{
        .root_source_file = omni5_root,
        .target = target,
        .optimize = optimize,
    });
    omni5.addImport("batch2", batch2);
    omni5.addImport("kernel3", kernel3);
    omni5.addImport("ipc4", ipc4);

    const t6_module = b.createModule(.{
        .root_source_file = b.path("src/tests.zig"),
        .target = target,
        .optimize = optimize,
    });
    t6_module.addImport("batch2", batch2);
    t6_module.addImport("kernel3", kernel3);
    t6_module.addImport("ipc4", ipc4);
    t6_module.addImport("omni5", omni5);
    const t6 = b.addTest(.{ .root_module = t6_module });
    const run_t6 = b.addRunArtifact(t6);

    const t5_module = b.createModule(.{
        .root_source_file = b.path("../OMNI-Kernel-Batch-5/src/tests.zig"),
        .target = target,
        .optimize = optimize,
    });
    t5_module.addImport("batch2", batch2);
    t5_module.addImport("kernel3", kernel3);
    t5_module.addImport("ipc4", ipc4);
    const t5 = b.addTest(.{ .root_module = t5_module });
    const run_t5 = b.addRunArtifact(t5);

    const t4_module = b.createModule(.{
        .root_source_file = b.path("../OMNI-Kernel-Batch-4/src/tests.zig"),
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

    const batch6_test_step = b.step("test-batch6", "Run only the Batch 6 hosted tests");
    batch6_test_step.dependOn(&run_t6.step);

    const test_step = b.step("test", "Run Batch 6 tests and the Batch 5, 4, 3 and 2 regression tests");
    test_step.dependOn(&run_t6.step);
    test_step.dependOn(&run_t5.step);
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
