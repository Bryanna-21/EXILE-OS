//! x86-64 initial kernel-thread context. SPDX-License-Identifier: Apache-2.0
//!
//! Pure data construction: this module computes the bytes of a brand-new
//! thread's first stack frame. It executes nothing and writes no memory.
//!
//! # Context-switch contract (binding on the future switch assembly, D3)
//!
//! A suspended thread is described by one saved stack pointer. The switch
//! routine saves the callee-saved registers by pushing them in the order
//! `rbx, rbp, r12, r13, r14, r15`, stores `rsp`, loads the next thread's `rsp`,
//! pops `r15, r14, r13, r12, rbp, rbx` and executes `ret`. A new thread is made
//! to look exactly like a thread that was suspended by that routine, so the same
//! restore path starts it. Lowest address first:
//!
//! ```text
//! saved_sp + 0x00  r15 = 0
//! saved_sp + 0x08  r14 = 0
//! saved_sp + 0x10  r13 = argument passed to the thread function
//! saved_sp + 0x18  r12 = thread function (kernel entry point)
//! saved_sp + 0x20  rbp = 0            (terminates frame-pointer chains)
//! saved_sp + 0x28  rbx = 0
//! saved_sp + 0x30  ret = trampoline   (popped by `ret`)
//! saved_sp + 0x38  0                  (fake return address of the trampoline)
//! stack_top        (16-byte aligned, one past the last stack byte)
//! ```
//!
//! Alignment: `saved_sp` and `stack_top` are 16-byte aligned. The final `ret`
//! pops the trampoline slot, so the trampoline starts with
//! `rsp = stack_top - 8` (`rsp % 16 == 8`), exactly as if it had been `call`ed.
//! The trampoline therefore executes `sub rsp, 8` to reach `rsp % 16 == 0`
//! before each `call`, so the callee sees `rsp % 16 == 8` as System V requires.
//!
//! Trampoline (see `switch.rs`): `sub rsp, 8; call [first-run hook];
//! mov rdi, r13; call r12; mov edi, eax; call [exit hook]; ud2`. A thread
//! function that returns therefore ends in the exit hook with its return value
//! as the exit status; it can never run off the stack. The sentinel word and the
//! final `ud2` are traps in case that invariant is ever broken.
//!
//! Not part of this frame: RFLAGS/interrupt state (the switch path decides),
//! x87/SSE/AVX state, FS/GS base (TLS) and CR3. Those belong to later work.

pub const FRAME_WORDS: usize = 8;
pub const FRAME_BYTES: u64 = (FRAME_WORDS * 8) as u64;
pub const STACK_ALIGN: u64 = 16;

pub const WORD_R15: usize = 0;
pub const WORD_R14: usize = 1;
pub const WORD_ARG_R13: usize = 2;
pub const WORD_ENTRY_R12: usize = 3;
pub const WORD_RBP: usize = 4;
pub const WORD_RBX: usize = 5;
pub const WORD_RET_TRAMPOLINE: usize = 6;
pub const WORD_SENTINEL: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    StackMisaligned,
    StackTooSmall,
    NullPointer,
    NonCanonical,
}

/// Canonical-address test for 4-level paging (48-bit virtual addresses).
pub const fn is_canonical(addr: u64) -> bool {
    let top = addr >> 47;
    top == 0 || top == 0x1_ffff
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitialContext {
    saved_sp: u64,
    words: [u64; FRAME_WORDS],
}

impl InitialContext {
    /// Value the switch routine must load into `rsp` to start the thread.
    pub fn saved_sp(&self) -> u64 {
        self.saved_sp
    }

    pub fn words(&self) -> &[u64; FRAME_WORDS] {
        &self.words
    }

    /// Frame image, little-endian, to be stored at `saved_sp`.
    pub fn to_bytes(&self) -> [u8; FRAME_BYTES as usize] {
        let mut out = [0u8; FRAME_BYTES as usize];
        for (i, w) in self.words.iter().enumerate() {
            out[i * 8..i * 8 + 8].copy_from_slice(&w.to_le_bytes());
        }
        out
    }
}

/// Builds the first frame for a thread whose stack is `[stack_base, stack_top)`.
pub fn build_initial(
    stack_base: u64,
    stack_top: u64,
    entry: u64,
    arg: u64,
    trampoline: u64,
) -> Result<InitialContext, Error> {
    if stack_top % STACK_ALIGN != 0 {
        return Err(Error::StackMisaligned);
    }
    let saved_sp = stack_top
        .checked_sub(FRAME_BYTES)
        .ok_or(Error::StackTooSmall)?;
    if saved_sp < stack_base {
        return Err(Error::StackTooSmall);
    }
    if entry == 0 || trampoline == 0 {
        return Err(Error::NullPointer);
    }
    if !is_canonical(entry)
        || !is_canonical(trampoline)
        || !is_canonical(saved_sp)
        || !is_canonical(stack_top - 8)
    {
        return Err(Error::NonCanonical);
    }
    let mut words = [0u64; FRAME_WORDS];
    words[WORD_ARG_R13] = arg;
    words[WORD_ENTRY_R12] = entry;
    words[WORD_RET_TRAMPOLINE] = trampoline;
    Ok(InitialContext { saved_sp, words })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOP: u64 = 0x40_0000;
    const BASE: u64 = TOP - 0x4000;

    #[test]
    fn frame_layout_matches_the_documented_contract() {
        let c = build_initial(BASE, TOP, 0x10_1000, 0xdead_beef, 0x10_2000).unwrap();
        assert_eq!(c.saved_sp(), TOP - 64);
        let w = c.words();
        assert_eq!(w[WORD_R15], 0);
        assert_eq!(w[WORD_R14], 0);
        assert_eq!(w[WORD_ARG_R13], 0xdead_beef);
        assert_eq!(w[WORD_ENTRY_R12], 0x10_1000);
        assert_eq!(w[WORD_RBP], 0);
        assert_eq!(w[WORD_RBX], 0);
        assert_eq!(w[WORD_RET_TRAMPOLINE], 0x10_2000);
        assert_eq!(w[WORD_SENTINEL], 0);
    }

    #[test]
    fn abi_alignment_holds_at_function_entry() {
        let c = build_initial(BASE, TOP, 0x10_1000, 0, 0x10_2000).unwrap();
        assert_eq!(c.saved_sp() % 16, 0);
        // `ret` pops the trampoline slot; rsp then points at the sentinel.
        let rsp_after_ret = c.saved_sp() + (WORD_RET_TRAMPOLINE as u64 + 1) * 8;
        assert_eq!(rsp_after_ret, TOP - 8);
        assert_eq!(rsp_after_ret % 16, 8, "System V: rsp % 16 == 8 on entry");
    }

    #[test]
    fn bytes_are_little_endian_words_in_address_order() {
        let c = build_initial(BASE, TOP, 0x10_1000, 0x0102_0304_0506_0708, 0x10_2000).unwrap();
        let b = c.to_bytes();
        assert_eq!(b.len(), 64);
        assert_eq!(&b[16..24], &[8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(u64::from_le_bytes(b[24..32].try_into().unwrap()), 0x10_1000);
        assert_eq!(u64::from_le_bytes(b[48..56].try_into().unwrap()), 0x10_2000);
    }

    #[test]
    fn rejects_misaligned_top_null_and_noncanonical_pointers() {
        assert_eq!(
            build_initial(BASE, TOP - 8, 1, 0, 1),
            Err(Error::StackMisaligned)
        );
        assert_eq!(
            build_initial(BASE, TOP, 0, 0, 0x10_2000),
            Err(Error::NullPointer)
        );
        assert_eq!(
            build_initial(BASE, TOP, 0x10_1000, 0, 0),
            Err(Error::NullPointer)
        );
        assert_eq!(
            build_initial(BASE, TOP, 0x0000_8000_0000_0000, 0, 0x10_2000),
            Err(Error::NonCanonical)
        );
        assert_eq!(
            build_initial(BASE, TOP, 0x10_1000, 0, 0x1234_0000_0000_0000),
            Err(Error::NonCanonical)
        );
    }

    #[test]
    fn rejects_stack_too_small_and_underflow() {
        assert_eq!(
            build_initial(TOP - 32, TOP, 1, 0, 1),
            Err(Error::StackTooSmall)
        );
        assert_eq!(build_initial(0, 16, 1, 0, 1), Err(Error::StackTooSmall));
        assert!(build_initial(TOP - 64, TOP, 1, 0, 1).is_ok(), "exact fit");
    }

    #[test]
    fn higher_half_canonical_addresses_are_accepted() {
        assert!(is_canonical(0xffff_8000_0000_0000));
        assert!(is_canonical(0x0000_7fff_ffff_ffff));
        assert!(!is_canonical(0x0001_0000_0000_0000));
    }
}
