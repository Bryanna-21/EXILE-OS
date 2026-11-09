//! Minimal ARM64 `virt` boot stub: sets a stack pointer, writes one line to the PL011 UART at 0x0900_0000, parks the CPU.
//! STATUS: SOURCE ONLY, NOT ASSEMBLED, NOT BOOTED. The authoring toolchain (Zig built without LLVM) could not assemble `bl <symbol>`, labels or `strb`, so the whole stub is one naked
//! function whose assembly is generated at compile time from the message. Even with these workarounds the last attempt still failed to assemble (`b .`), so treat this file as a draft to build with a full Zig/LLVM toolchain.
const msg = "EXILE-OS ARM64 virt: boot stub reached\n";

fn emit() []const u8 {
    comptime var out: []const u8 = "movz x2, #0x0900, lsl #16\n";
    inline for (msg) |c| {
        out = out ++ std.fmt.comptimePrint("movz w1, #{d}\nstr w1, [x2]\n", .{c});
    }
    return out;
}
const std = @import("std");

pub export fn _start() linksection(".text.boot") callconv(.naked) noreturn {
    asm volatile (
        \\ movz x0, #0x4010, lsl #16
        \\ mov sp, x0
        ++ "\n" ++ emit() ++
        \\ wfe
        \\ b .
    );
}
