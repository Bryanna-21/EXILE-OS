//! Architecture boundary (Batch 2).
//!
//! The portable core must not contain machine-specific operations. This file
//! defines, as a COMPILE-TIME CONTRACT, the small set of facts and operations a
//! per-architecture backend has to provide. It contains no machine code, no
//! inline assembly and no privileged operations.
//!
//! STATUS: NO architecture backend exists in Batch 2. `src/arch/x86_64/` and
//! `src/arch/aarch64/` contain only README.md files describing future
//! requirements. Nothing here boots, and neither architecture is operational.
//! The only implementer in this batch is a test mock (see tests.zig), used to
//! exercise the contract on the host.
//!
//! A backend is a Zig type `T` with these declarations:
//!   pub const name: []const u8
//!   pub const page_size: u64              (power of two)
//!   pub const user_va_lo: u64             (inclusive)
//!   pub const user_va_hi: u64             (exclusive)
//!   pub fn irqSaveDisable() bool          disable interrupts, return previous state
//!   pub fn irqRestore(prev: bool) void
//!   pub fn fullBarrier() void             full memory barrier
//!   pub fn cpuRelax() void                spin-wait hint
//!
//! Interrupt-controller, MMU, boot-entry and exception-entry interfaces are
//! intentionally NOT defined yet; see the per-architecture READMEs and
//! docs/IMPLEMENTATION-PLAN.md. Defining them now without hardware or an
//! emulator to test against would be speculation.

const std = @import("std");
const memory = @import("../core/memory.zig");
const OmniError = @import("../core/error.zig").OmniError;

/// True for `[]const u8` or a pointer to a u8 array (string literals).
fn isString(comptime S: type) bool {
    if (S == []const u8) return true;
    const info = @typeInfo(S);
    if (info != .pointer) return false;
    const child = @typeInfo(info.pointer.child);
    return child == .array and child.array.child == u8;
}

/// True if `T` provides every declaration of the contract with the right type.
pub fn implements(comptime T: type) bool {
    if (!@hasDecl(T, "name") or !@hasDecl(T, "page_size") or
        !@hasDecl(T, "user_va_lo") or !@hasDecl(T, "user_va_hi") or
        !@hasDecl(T, "irqSaveDisable") or !@hasDecl(T, "irqRestore") or
        !@hasDecl(T, "fullBarrier") or !@hasDecl(T, "cpuRelax"))
        return false;

    if (!isString(@TypeOf(T.name))) return false;
    if (@TypeOf(T.page_size) != u64) return false;
    if (@TypeOf(T.user_va_lo) != u64 or @TypeOf(T.user_va_hi) != u64) return false;
    if (@TypeOf(T.irqSaveDisable) != fn () bool) return false;
    if (@TypeOf(T.irqRestore) != fn (bool) void) return false;
    if (@TypeOf(T.fullBarrier) != fn () void) return false;
    if (@TypeOf(T.cpuRelax) != fn () void) return false;
    return true;
}

/// Compile error unless `T` implements the contract.
pub fn assertImplements(comptime T: type) void {
    if (!implements(T)) @compileError("type " ++ @typeName(T) ++ " does not implement the OMNI architecture contract");
}

/// Check the numeric facts of a backend: page size is a power of two and the user
/// window is non-empty, page-aligned and ordered. Runtime-checkable so tests can
/// feed bad values.
pub fn validateFacts(page_size: u64, user_lo: u64, user_hi: u64) OmniError!memory.UserWindow {
    if (!memory.isPowerOfTwo(page_size)) return error.InvalidArgument;
    if (user_lo >= user_hi) return error.InvalidArgument;
    try memory.checkAligned(user_lo, page_size);
    try memory.checkAligned(user_hi, page_size);
    return .{ .lo = user_lo, .hi = user_hi };
}

/// Build the user window for a backend type, validating its facts.
pub fn userWindow(comptime T: type) OmniError!memory.UserWindow {
    comptime assertImplements(T);
    return validateFacts(T.page_size, T.user_va_lo, T.user_va_hi);
}
