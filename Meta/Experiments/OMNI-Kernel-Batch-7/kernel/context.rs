//! Context switching: the assembly lives in `arch/x86_64/context.S`; this file builds
//! initial stack frames and validates saved stack pointers before they are ever loaded.

#[cfg(not(target_arch = "x86_64"))]
compile_error!("OMNI-Kernel-Batch-7 is x86-64 only; there is no ARM64 implementation here");

use core::arch::global_asm;

global_asm!(include_str!("arch/x86_64/context.S"), options(att_syntax));

unsafe extern "C" {
    fn omni_switch_context(save_rsp: *mut usize, new_rsp: usize);
    fn omni_thread_trampoline();
}

pub const STACK_SIZE: usize = 16 * 1024;
/// Lowest 8 bytes of every stack hold this value; a changed value means overflow.
pub const CANARY: u64 = 0x4F4D_4E49_5354_4B21;
pub const CANARY_BYTES: usize = 8;
/// Words popped/returned by a switch: r15 r14 r13 r12 rbx rbp rflags ret.
pub const FRAME_WORDS: usize = 8;
pub const FRAME_BYTES: usize = FRAME_WORDS * 8;
/// IF set, reserved bit 1 set, IOPL 0.
pub const INITIAL_RFLAGS: usize = 0x202;
/// Slack left above the initial frame.
const TOP_SLACK: usize = 128;

#[repr(C, align(16))]
pub struct Stack(pub [u8; STACK_SIZE]);

impl Stack {
    pub const ZERO: Stack = Stack([0; STACK_SIZE]);
}

/// Address of the trampoline a new thread first returns into.
pub fn trampoline_address() -> usize {
    omni_thread_trampoline as *const () as usize
}

/// Build the first frame of a new thread. Returns the saved stack pointer to store.
///
/// # Safety
/// `base` must point to `size` writable bytes, 16-byte aligned, owned by the caller and
/// not in use by any thread.
pub unsafe fn init_stack(
    base: *mut u8,
    size: usize,
    entry: extern "C" fn(usize),
    arg: usize,
    hook: extern "C" fn(usize) -> !,
    hook_arg: usize,
) -> usize {
    assert!(base as usize % 16 == 0 && size % 16 == 0 && size >= 512);
    // SAFETY: caller guarantees the region; we stay inside it.
    unsafe {
        // Scrub: a new thread must never see another thread's stale data.
        core::ptr::write_bytes(base, 0, size);
        (base as *mut u64).write(CANARY);
    }
    let rsp = base as usize + size - TOP_SLACK;
    debug_assert!(rsp % 16 == 0);
    let w = rsp as *mut usize;
    // SAFETY: rsp..rsp+FRAME_BYTES is inside the region (TOP_SLACK > FRAME_BYTES).
    unsafe {
        w.add(0).write(hook_arg); // r15
        w.add(1).write(hook as usize); // r14
        w.add(2).write(arg); // r13
        w.add(3).write(entry as usize); // r12
        w.add(4).write(0); // rbx
        w.add(5).write(0); // rbp (ends frame-pointer walks)
        w.add(6).write(INITIAL_RFLAGS); // rflags
        w.add(7).write(trampoline_address()); // return address
    }
    rsp
}

/// A saved stack pointer is loadable only if it is 16-byte aligned and the whole
/// switch frame lies inside the thread's own stack, above the canary.
pub fn validate_saved_rsp(base: usize, size: usize, rsp: usize) -> bool {
    if rsp % 16 != 0 {
        return false;
    }
    let low = match base.checked_add(CANARY_BYTES) {
        Some(v) => v,
        None => return false,
    };
    let top = match base.checked_add(size) {
        Some(v) => v,
        None => return false,
    };
    match rsp.checked_add(FRAME_BYTES) {
        Some(end) => rsp >= low && end <= top,
        None => false,
    }
}

/// # Safety
/// `base` must point to a stack initialised by `init_stack`.
pub unsafe fn canary_ok(base: *const u8) -> bool {
    // SAFETY: caller guarantees `base` is a valid stack base.
    unsafe { (base as *const u64).read() == CANARY }
}

/// Save the current context into `*save`, load `to`, and resume it.
///
/// # Safety
/// `to` must be a stack pointer produced by `init_stack` or a previous `switch`, for a
/// stack that is alive and not currently executing; `save` must stay valid until the
/// matching resume.
pub unsafe fn switch(save: *mut usize, to: usize) {
    // SAFETY: forwarded contract.
    unsafe { omni_switch_context(save, to) }
}
