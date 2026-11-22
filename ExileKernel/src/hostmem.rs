//! Freestanding `mem*` primitives. SPDX-License-Identifier: Apache-2.0
//!
//! Only compiled with `--cfg euk_host_mem`, which `scripts/build.py` sets when
//! `EUK_TARGET=host` (a build against the host target's `core`, used where the
//! `x86_64-unknown-none` standard library is unavailable). The pinned
//! `x86_64-unknown-none` build gets these from `compiler_builtins` and does not
//! compile this module.
//!
//! Implemented with x86 string instructions so the compiler cannot turn a loop
//! back into a call to the very function being defined.
use core::arch::asm;

/// # Safety
/// `dst..dst+n` and `src..src+n` must be valid and must not overlap.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    // SAFETY: forwarded to the caller (C `memcpy` contract). DF is clear per the ABI.
    unsafe {
        asm!("rep movsb", inout("rcx") n => _, inout("rdi") dst => _, inout("rsi") src => _,
             options(nostack, preserves_flags));
    }
    dst
}

/// # Safety
/// `dst..dst+n` and `src..src+n` must be valid; they may overlap.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if (dst as usize) <= (src as usize) || (dst as usize) >= (src as usize).wrapping_add(n) {
        // SAFETY: forward copy is correct when dst is below src or past its end.
        return unsafe { memcpy(dst, src, n) };
    }
    if n != 0 {
        // SAFETY: overlapping with dst above src: copy backwards, then clear DF
        // again to restore the ABI state.
        unsafe {
            asm!("std", "rep movsb", "cld",
                 inout("rcx") n => _, inout("rdi") dst.add(n - 1) => _,
                 inout("rsi") src.add(n - 1) => _, options(nostack));
        }
    }
    dst
}

/// # Safety
/// `dst..dst+n` must be valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memset(dst: *mut u8, c: i32, n: usize) -> *mut u8 {
    // SAFETY: forwarded to the caller (C `memset` contract).
    unsafe {
        asm!("rep stosb", inout("rcx") n => _, inout("rdi") dst => _, in("al") c as u8,
             options(nostack, preserves_flags));
    }
    dst
}

/// # Safety
/// Both ranges must be valid for `n` bytes of reads.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    if n == 0 {
        return 0;
    }
    let (mut pa, mut pb, mut cnt) = (a, b, n);
    // SAFETY: forwarded to the caller. `repe cmpsb` stops after the first
    // differing byte (or after n equal bytes); both pointers then sit one past
    // the last byte compared, so the final pair decides the result.
    unsafe {
        asm!("repe cmpsb", inout("rcx") cnt, inout("rsi") pa, inout("rdi") pb,
             options(nostack));
        let _ = cnt;
        i32::from(*pa.sub(1)) - i32::from(*pb.sub(1))
    }
}

/// # Safety
/// Both ranges must be valid for `n` bytes of reads.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    // SAFETY: forwarded to the caller.
    unsafe { memcmp(a, b, n) }
}
