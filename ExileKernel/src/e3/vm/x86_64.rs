//! Privileged x86-64 paging controls. Host builds never execute these instructions.
use super::tlb::{TlbError, TlbInvalidator};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cr3(u64);
impl Cr3 {
    pub fn new(raw: u64) -> Result<Self, Cr3Error> {
        if raw & 0xfff != 0 {
            return Err(Cr3Error::Unaligned);
        }
        if raw & !0x000f_ffff_ffff_f000 != 0 {
            return Err(Cr3Error::ReservedBits);
        }
        Ok(Self(raw))
    }
    pub const fn raw(self) -> u64 {
        self.0
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cr3Error {
    Unaligned,
    ReservedBits,
}
pub struct LocalTlb;
impl LocalTlb {
    pub const fn new() -> Self {
        Self
    }
}
#[cfg(all(target_arch = "x86_64", target_os = "none"))]
impl LocalTlb {
    /// Reads CR3. Caller must execute at CPL0 with paging enabled.
    pub unsafe fn read_cr3() -> Cr3 {
        let value: u64; // SAFETY: privileged operation; caller guarantees CPL0.
        unsafe {
            core::arch::asm!("mov {}, cr3",out(reg)value,options(nomem,nostack,preserves_flags));
        }
        Cr3(value & !0xfff)
    }
    /// Loads CR3, switching the active root. Caller must ensure the root is valid and mapped.
    pub unsafe fn write_cr3(value: Cr3) {
        // SAFETY: privileged operation; caller guarantees CPL0 and a valid page-table root.
        unsafe {
            core::arch::asm!("mov cr3, {}",in(reg)value.raw(),options(nostack,preserves_flags));
        }
    }
    /// Invalidates one local translation. This is not an SMP shootdown.
    pub unsafe fn invlpg(address: u64) {
        // SAFETY: privileged instruction; caller guarantees CPL0 and canonical address.
        unsafe {
            core::arch::asm!("invlpg [{}]",in(reg)address,options(nostack,preserves_flags));
        }
    }
}
impl TlbInvalidator for LocalTlb {
    fn invalidate_page(&mut self, v: u64) -> Result<(), TlbError> {
        if !super::geometry::is_canonical(v) {
            return Err(TlbError::HardwareFault);
        }
        #[cfg(all(target_arch = "x86_64", target_os = "none"))]
        {
            // SAFETY: kernel-only implementation; this method is only available in the bare-metal build.
            unsafe { Self::invlpg(v) };
            return Ok(());
        }
        #[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
        {
            let _ = v;
            Err(TlbError::Unsupported)
        }
    }
    fn invalidate_all(&mut self) -> Result<(), TlbError> {
        #[cfg(all(target_arch = "x86_64", target_os = "none"))]
        {
            // SAFETY: reloading CR3 flushes non-global translations on this CPU; global pages remain.
            unsafe {
                let cr3 = Self::read_cr3();
                Self::write_cr3(cr3)
            };
            return Ok(());
        }
        #[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
        {
            Err(TlbError::Unsupported)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cr3_validation() {
        assert_eq!(Cr3::new(0x1001), Err(Cr3Error::Unaligned));
        assert_eq!(Cr3::new(0x1000).unwrap().raw(), 0x1000);
        assert_eq!(Cr3::new(1 << 63), Err(Cr3Error::ReservedBits));
    }
    #[test]
    fn host_never_executes_privileged_tlb_instructions() {
        let mut tlb = LocalTlb::new();
        assert_eq!(tlb.invalidate_page(0x1000), Err(TlbError::Unsupported));
        assert_eq!(tlb.invalidate_all(), Err(TlbError::Unsupported));
    }
}
