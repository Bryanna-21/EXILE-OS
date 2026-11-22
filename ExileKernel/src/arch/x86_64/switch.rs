//! x86-64 context switch and first-run trampoline. SPDX-License-Identifier: Apache-2.0
//!
//! This is the only assembly D3 adds. It contains no scheduler policy.
//!
//! # `euk_switch_context(prev_sp_slot: *mut u64, next_sp: u64)`
//! System V AMD64 call. Pushes `rbx, rbp, r12, r13, r14, r15` on the current
//! stack, stores `rsp` to `*prev_sp_slot`, loads `rsp = next_sp`, pops
//! `r15, r14, r13, r12, rbp, rbx` and `ret`s. The frame it leaves behind is
//! byte-for-byte the format `context::build_initial` writes for a new thread, so
//! one restore path starts new threads and resumes suspended ones.
//!
//! It is an ordinary function call from the compiler's point of view: the
//! caller-saved registers (rax, rcx, rdx, rsi, rdi, r8-r11, xmm/ymm, flags) are
//! assumed clobbered, which is why no FPU/SSE state is saved. RFLAGS (including
//! IF) is not saved or restored: the caller keeps interrupts disabled across the
//! switch (see `dispatch`). The red zone is disabled for kernel code.
//!
//! # `euk_thread_trampoline`
//! Reached by the `ret` of the first restore, with `rsp % 16 == 8` as if
//! `call`ed. It calls the FIRST-RUN hook (the new thread finishes the
//! scheduler's switch bookkeeping on its own stack), then the thread function
//! `r12(r13)`, then the EXIT hook with the function's return value as the exit
//! status. The exit hook must not return; `ud2` follows as a trap.
//!
//! The hooks are installed once with `install_hooks`. They are plain function
//! pointers, not Rust closures, and are read by the trampoline from statics.
//!
//! Unsafe: only the two `extern "C"` calls below. Their contracts are on the
//! wrappers.
use core::arch::global_asm;
use core::sync::atomic::{AtomicUsize, Ordering};

pub static FIRST_RUN_HOOK: AtomicUsize = AtomicUsize::new(0);
pub static EXIT_HOOK: AtomicUsize = AtomicUsize::new(0);

/// Runs on a new thread's own stack before its function starts.
pub type FirstRunHook = extern "C" fn();
/// Runs when a thread function returns; receives its return value. Never returns.
pub type ExitHook = extern "C" fn(i32) -> !;

/// Installs the trampoline hooks. Must happen before any thread created with
/// `trampoline_address()` is first switched to.
pub fn install_hooks(first_run: FirstRunHook, exit: ExitHook) {
    FIRST_RUN_HOOK.store(first_run as usize, Ordering::SeqCst);
    EXIT_HOOK.store(exit as usize, Ordering::SeqCst);
}

pub fn hooks_installed() -> bool {
    FIRST_RUN_HOOK.load(Ordering::SeqCst) != 0 && EXIT_HOOK.load(Ordering::SeqCst) != 0
}

global_asm!(
    ".text",
    ".p2align 4",
    ".global euk_switch_context",
    ".type euk_switch_context,@function",
    "euk_switch_context:",
    "    push rbx",
    "    push rbp",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    "    mov qword ptr [rdi], rsp",
    "    mov rsp, rsi",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop rbp",
    "    pop rbx",
    "    ret",
    ".size euk_switch_context, . - euk_switch_context",
    ".p2align 4",
    ".global euk_thread_trampoline",
    ".type euk_thread_trampoline,@function",
    "euk_thread_trampoline:",
    "    sub rsp, 8",
    "    mov rax, qword ptr [rip + {first}]",
    "    call rax",
    "    mov rdi, r13",
    "    call r12",
    "    mov edi, eax",
    "    mov rax, qword ptr [rip + {exit}]",
    "    call rax",
    "    ud2",
    ".size euk_thread_trampoline, . - euk_thread_trampoline",
    first = sym FIRST_RUN_HOOK,
    exit = sym EXIT_HOOK,
);

unsafe extern "C" {
    fn euk_switch_context(prev_sp_slot: *mut u64, next_sp: u64);
    fn euk_thread_trampoline();
}

/// Address to pass to `context::build_initial` as the trampoline.
pub fn trampoline_address() -> u64 {
    euk_thread_trampoline as *const () as usize as u64
}

/// Suspends the current context into `*prev_sp_slot` and resumes `next_sp`.
/// Returns when something later switches back to the suspended context.
///
/// # Safety
/// - `prev_sp_slot` is valid for a `u64` write and not otherwise accessed until
///   the context is resumed.
/// - `next_sp` is a stack pointer produced by this routine or by
///   `context::build_initial` for a stack that is mapped, owned by the thread
///   being resumed, and not executing anywhere.
/// - Interrupts are disabled and no spinlock is held across the call.
/// - Hooks are installed if `next_sp` is a never-run thread.
pub unsafe fn switch_context(prev_sp_slot: *mut u64, next_sp: u64) {
    // SAFETY: forwarded to the caller; see the contract above.
    unsafe { euk_switch_context(prev_sp_slot, next_sp) }
}

#[cfg(test)]
pub(crate) static HOST_SWITCH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    //! These tests EXECUTE the assembly on the x86-64 host, in user mode, on
    //! host-allocated stacks. They prove register save/restore, the frame
    //! format, first-run entry, alignment and the exit path of the real routine.
    //! They do not exercise privilege levels, interrupts or the kernel image.
    use super::*;
    use crate::context;
    use core::sync::atomic::AtomicU64;

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        HOST_SWITCH_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    struct HostStack {
        _buf: Vec<u8>,
        base: u64,
        top: u64,
    }

    impl HostStack {
        fn new(size: usize) -> Self {
            let mut buf = vec![0u8; size + 4096];
            let base = (buf.as_mut_ptr() as u64 + 4095) & !4095;
            Self {
                _buf: buf,
                base,
                top: base + size as u64,
            }
        }
        /// Writes a fresh initial frame and returns the saved sp.
        fn init(&self, entry: u64, arg: u64) -> u64 {
            let c = context::build_initial(self.base, self.top, entry, arg, trampoline_address())
                .unwrap();
            let bytes = c.to_bytes();
            // SAFETY: saved_sp..saved_sp+64 lies inside this live host buffer
            // (build_initial checked saved_sp >= base and saved_sp+64 <= top).
            unsafe {
                core::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    c.saved_sp() as *mut u8,
                    bytes.len(),
                );
            }
            c.saved_sp()
        }
    }

    unsafe extern "C" {
        fn euk_test_regcheck(slot: *mut u64, next_sp: u64) -> u32;
        fn euk_test_clobber_and_switch(slot: *mut u64, next_sp: u64);
        fn euk_test_probe_entry();
        fn euk_test_probe_hook();
    }

    static PROBE_ENTRY_RSP: AtomicU64 = AtomicU64::new(0);
    static PROBE_ENTRY_ARG: AtomicU64 = AtomicU64::new(0);
    static PROBE_HOOK_RSP: AtomicU64 = AtomicU64::new(0);
    static PROBE_SEQ: AtomicU64 = AtomicU64::new(0);
    static PROBE_HOOK_SEQ: AtomicU64 = AtomicU64::new(0);
    static PROBE_ENTRY_SEQ: AtomicU64 = AtomicU64::new(0);

    global_asm!(
        ".text",
        // rdi=slot, rsi=next_sp. Sets every callee-saved register to a known
        // value, switches away, and after being resumed checks they are intact.
        ".global euk_test_regcheck",
        "euk_test_regcheck:",
        "    push rbx", "    push rbp", "    push r12", "    push r13", "    push r14", "    push r15",
        "    sub rsp, 8",
        "    mov rbx, 0x1111111111111111",
        "    mov rbp, 0x2222222222222222",
        "    mov r12, 0x3333333333333333",
        "    mov r13, 0x4444444444444444",
        "    mov r14, 0x5555555555555555",
        "    mov r15, 0x6666666666666666",
        "    call euk_switch_context",
        "    mov eax, 1",
        "    mov rcx, 0x1111111111111111", "    cmp rbx, rcx", "    jne 2f",
        "    mov rcx, 0x2222222222222222", "    cmp rbp, rcx", "    jne 2f",
        "    mov rcx, 0x3333333333333333", "    cmp r12, rcx", "    jne 2f",
        "    mov rcx, 0x4444444444444444", "    cmp r13, rcx", "    jne 2f",
        "    mov rcx, 0x5555555555555555", "    cmp r14, rcx", "    jne 2f",
        "    mov rcx, 0x6666666666666666", "    cmp r15, rcx", "    jne 2f",
        "    jmp 3f",
        "2:  xor eax, eax",
        "3:  add rsp, 8",
        "    pop r15", "    pop r14", "    pop r13", "    pop r12", "    pop rbp", "    pop rbx",
        "    ret",
        // Trashes all callee-saved registers, then switches away (tail call).
        ".global euk_test_clobber_and_switch",
        "euk_test_clobber_and_switch:",
        "    mov rbx, 0xdeadbeefdeadbeef",
        "    mov rbp, 0xdeadbeefdeadbeef",
        "    mov r12, 0xdeadbeefdeadbeef",
        "    mov r13, 0xdeadbeefdeadbeef",
        "    mov r14, 0xdeadbeefdeadbeef",
        "    mov r15, 0xdeadbeefdeadbeef",
        "    jmp euk_switch_context",
        // Thread function: records rsp at entry and its argument, returns 42.
        ".global euk_test_probe_entry",
        "euk_test_probe_entry:",
        "    mov qword ptr [rip + {entry_rsp}], rsp",
        "    mov qword ptr [rip + {entry_arg}], rdi",
        "    lock inc qword ptr [rip + {seq}]",
        "    mov rax, qword ptr [rip + {seq}]",
        "    mov qword ptr [rip + {entry_seq}], rax",
        "    mov eax, 42",
        "    ret",
        // First-run hook: records rsp at hook entry and its order.
        ".global euk_test_probe_hook",
        "euk_test_probe_hook:",
        "    mov qword ptr [rip + {hook_rsp}], rsp",
        "    lock inc qword ptr [rip + {seq}]",
        "    mov rax, qword ptr [rip + {seq}]",
        "    mov qword ptr [rip + {hook_seq}], rax",
        "    ret",
        entry_rsp = sym PROBE_ENTRY_RSP,
        entry_arg = sym PROBE_ENTRY_ARG,
        hook_rsp = sym PROBE_HOOK_RSP,
        seq = sym PROBE_SEQ,
        hook_seq = sym PROBE_HOOK_SEQ,
        entry_seq = sym PROBE_ENTRY_SEQ,
    );

    static BOOT_SP: AtomicU64 = AtomicU64::new(0);
    static SCRATCH_SP: AtomicU64 = AtomicU64::new(0);
    static EXIT_STATUS: AtomicU64 = AtomicU64::new(u64::MAX);

    extern "C" fn exit_back_to_boot(status: i32) -> ! {
        EXIT_STATUS.store(status as u32 as u64, Ordering::SeqCst);
        // SAFETY: BOOT_SP was written by the test's own switch into this thread
        // and that context is suspended; SCRATCH_SP is a valid slot.
        unsafe { switch_context(SCRATCH_SP.as_ptr(), BOOT_SP.load(Ordering::SeqCst)) };
        unreachable!("boot context never resumes an exited thread");
    }

    extern "C" fn noop_hook() {}

    #[test]
    fn trampoline_address_is_a_real_nonnull_text_address() {
        let a = trampoline_address();
        assert_ne!(a, 0);
        assert!(context::is_canonical(a));
        let f: extern "C" fn() = noop_hook;
        assert_ne!(a, f as usize as u64);
    }

    #[test]
    fn hooks_must_be_installed_explicitly() {
        let _g = guard();
        FIRST_RUN_HOOK.store(0, Ordering::SeqCst);
        EXIT_HOOK.store(0, Ordering::SeqCst);
        assert!(!hooks_installed());
        install_hooks(noop_hook, exit_back_to_boot);
        assert!(hooks_installed());
    }

    #[test]
    fn callee_saved_registers_survive_a_switch_and_the_frame_has_the_documented_layout() {
        let _g = guard();
        install_hooks(noop_hook, exit_back_to_boot);
        // B inspects A's suspended frame and then trashes every callee-saved reg.
        static FRAME_SEEN: [AtomicU64; 7] = [const { AtomicU64::new(0) }; 7];
        static A_SLOT: AtomicU64 = AtomicU64::new(0);
        extern "C" fn b_entry(_arg: u64) -> i32 {
            let sp = A_SLOT.load(Ordering::SeqCst);
            for (i, w) in FRAME_SEEN.iter().enumerate() {
                // SAFETY: A's frame (7 words) is live on A's host stack; A is
                // suspended so nothing else writes it.
                let v = unsafe { core::ptr::read((sp + i as u64 * 8) as *const u64) };
                w.store(v, Ordering::SeqCst);
            }
            // SAFETY: SCRATCH_SP is a valid slot; A_SLOT holds A's saved sp.
            unsafe {
                euk_test_clobber_and_switch(SCRATCH_SP.as_ptr(), A_SLOT.load(Ordering::SeqCst))
            };
            unreachable!()
        }
        // A is the test thread's own stack; only B needs a host stack.
        let b_stack = HostStack::new(64 * 1024);
        let b_sp = b_stack.init(b_entry as *const () as usize as u64, 0);
        A_SLOT.store(0, Ordering::SeqCst);
        // SAFETY: A_SLOT is a valid slot; b_sp is a fresh initial frame on a live
        // host stack; hooks are installed. B resumes us through A_SLOT.
        let ok = unsafe { euk_test_regcheck(A_SLOT.as_ptr(), b_sp) };
        assert_eq!(ok, 1, "callee-saved registers were not restored");
        let f: Vec<u64> = FRAME_SEEN
            .iter()
            .map(|w| w.load(Ordering::SeqCst))
            .collect();
        assert_eq!(f[context::WORD_R15], 0x6666_6666_6666_6666);
        assert_eq!(f[context::WORD_R14], 0x5555_5555_5555_5555);
        assert_eq!(f[context::WORD_ARG_R13], 0x4444_4444_4444_4444);
        assert_eq!(f[context::WORD_ENTRY_R12], 0x3333_3333_3333_3333);
        assert_eq!(f[context::WORD_RBP], 0x2222_2222_2222_2222);
        assert_eq!(f[context::WORD_RBX], 0x1111_1111_1111_1111);
        assert_ne!(f[context::WORD_RET_TRAMPOLINE], 0, "resume address saved");
    }

    #[test]
    fn first_run_calls_hook_then_entry_with_abi_alignment_then_exit_hook() {
        let _g = guard();
        install_hooks(
            // SAFETY: euk_test_probe_hook is an asm function with the extern "C"
            // `fn()` signature (it only records rsp and returns).
            unsafe {
                core::mem::transmute::<unsafe extern "C" fn(), FirstRunHook>(euk_test_probe_hook)
            },
            exit_back_to_boot,
        );
        for v in [
            &PROBE_ENTRY_RSP,
            &PROBE_ENTRY_ARG,
            &PROBE_HOOK_RSP,
            &PROBE_SEQ,
            &PROBE_HOOK_SEQ,
            &PROBE_ENTRY_SEQ,
        ] {
            v.store(0, Ordering::SeqCst);
        }
        EXIT_STATUS.store(u64::MAX, Ordering::SeqCst);
        let stack = HostStack::new(64 * 1024);
        let sp = stack.init(
            euk_test_probe_entry as *const () as usize as u64,
            0xfeed_f00d,
        );
        // SAFETY: fresh frame on a live host stack; the thread exits back to
        // BOOT_SP, which this call fills in.
        unsafe { switch_context(BOOT_SP.as_ptr(), sp) };
        assert_eq!(
            PROBE_ENTRY_ARG.load(Ordering::SeqCst),
            0xfeed_f00d,
            "arg in rdi"
        );
        assert_eq!(
            EXIT_STATUS.load(Ordering::SeqCst),
            42,
            "return value is the exit status"
        );
        assert_eq!(
            PROBE_HOOK_SEQ.load(Ordering::SeqCst),
            1,
            "hook before entry"
        );
        assert_eq!(PROBE_ENTRY_SEQ.load(Ordering::SeqCst), 2);
        let entry_rsp = PROBE_ENTRY_RSP.load(Ordering::SeqCst);
        let hook_rsp = PROBE_HOOK_RSP.load(Ordering::SeqCst);
        assert_eq!(
            entry_rsp % 16,
            8,
            "System V: rsp % 16 == 8 on function entry"
        );
        assert_eq!(hook_rsp % 16, 8, "hook is called with the same convention");
        // Placement: `ret` leaves rsp = top-8, `sub rsp, 8` gives top-16, and the
        // `call` pushes its return address, so both callees start at top-24.
        assert_eq!(entry_rsp, stack.top - 24);
        assert_eq!(hook_rsp, stack.top - 24);
    }

    #[test]
    fn many_switch_cycles_between_two_contexts_keep_registers_and_stacks_intact() {
        let _g = guard();
        install_hooks(noop_hook, exit_back_to_boot);
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        static PEER_SP: AtomicU64 = AtomicU64::new(0);
        extern "C" fn peer(_a: u64) -> i32 {
            // Locals that must survive each suspension on this stack.
            let mut local = [0u64; 8];
            for i in 0..50_000u64 {
                local[(i % 8) as usize] = local[(i % 8) as usize].wrapping_add(i ^ 0x5a5a);
                COUNTER.fetch_add(1, Ordering::SeqCst);
                // SAFETY: both slots are valid; BOOT_SP holds the suspended main context.
                unsafe { switch_context(PEER_SP.as_ptr(), BOOT_SP.load(Ordering::SeqCst)) };
            }
            let mut expect = [0u64; 8];
            for i in 0..50_000u64 {
                expect[(i % 8) as usize] = expect[(i % 8) as usize].wrapping_add(i ^ 0x5a5a);
            }
            (local == expect) as i32
        }
        COUNTER.store(0, Ordering::SeqCst);
        EXIT_STATUS.store(u64::MAX, Ordering::SeqCst);
        let stack = HostStack::new(64 * 1024);
        BOOT_SP.store(0, Ordering::SeqCst);
        PEER_SP.store(
            stack.init(peer as *const () as usize as u64, 0),
            Ordering::SeqCst,
        );
        let mut seen = 0u64;
        while EXIT_STATUS.load(Ordering::SeqCst) == u64::MAX {
            // SAFETY: BOOT_SP is a valid slot; PEER_SP is a fresh frame or the
            // peer's last suspension; the peer stack is live.
            unsafe { switch_context(BOOT_SP.as_ptr(), PEER_SP.load(Ordering::SeqCst)) };
            seen += 1;
        }
        // The peer's final return goes through the trampoline to exit_back_to_boot,
        // which resumes BOOT_SP: the same slot the loop above suspends into.
        assert_eq!(COUNTER.load(Ordering::SeqCst), 50_000);
        assert_eq!(
            EXIT_STATUS.load(Ordering::SeqCst),
            1,
            "peer locals survived 50k switches"
        );
        assert_eq!(seen, 50_001);
    }
}
