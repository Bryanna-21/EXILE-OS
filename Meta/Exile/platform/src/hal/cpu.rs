//! CPU feature discovery and capability reporting (x86-64 CPUID).
//!
//! [`decode`] is pure and works on any [`CpuidSource`]. [`HardwareCpuid`] reads the CPU the code is running on; the host
//! tests run it against the sandbox's *virtual* CPU, which proves the decoder handles real CPUID output, not that any
//! particular physical CPU model is supported.

/// Source of CPUID results: `(eax, ebx, ecx, edx)` for `(leaf, subleaf)`.
pub trait CpuidSource {
    /// Executes (or emulates) CPUID.
    fn cpuid(&self, leaf: u32, subleaf: u32) -> [u32; 4];
}

/// CPUID on the running processor.
#[cfg(target_arch = "x86_64")]
pub struct HardwareCpuid;

#[cfg(target_arch = "x86_64")]
impl CpuidSource for HardwareCpuid {
    #[allow(unused_unsafe)] // `__cpuid_count` is a safe fn on newer compilers and unsafe on older ones.
    fn cpuid(&self, leaf: u32, subleaf: u32) -> [u32; 4] {
        // SAFETY: the CPUID instruction exists on every x86-64 processor and has no memory side effects.
        let r = unsafe { core::arch::x86_64::__cpuid_count(leaf, subleaf) };
        [r.eax, r.ebx, r.ecx, r.edx]
    }
}

macro_rules! flags {
    ($($(#[$m:meta])* $name:ident = $bit:expr;)*) => {
        $( $(#[$m])* pub const $name: u64 = 1 << $bit; )*
    };
}

/// Feature bits in [`CpuFeatures::flags`].
pub mod feature {
    flags! {
        /// x87 FPU.
        FPU = 0;
        /// Physical address extension.
        PAE = 1;
        /// Time-stamp counter.
        TSC = 2;
        /// Model-specific registers.
        MSR = 3;
        /// On-chip local APIC.
        APIC = 4;
        /// Global pages.
        PGE = 5;
        /// FXSAVE/FXRSTOR.
        FXSR = 6;
        /// SSE.
        SSE = 7;
        /// SSE2.
        SSE2 = 8;
        /// x2APIC mode.
        X2APIC = 9;
        /// XSAVE.
        XSAVE = 10;
        /// AVX.
        AVX = 11;
        /// RDRAND.
        RDRAND = 12;
        /// Running under a hypervisor.
        HYPERVISOR = 13;
        /// Process-context identifiers.
        PCID = 14;
        /// SYSCALL/SYSRET in long mode.
        SYSCALL = 15;
        /// No-execute page bit.
        NX = 16;
        /// 1 GiB pages.
        PAGE1GB = 17;
        /// Long mode (64-bit).
        LONG_MODE = 18;
        /// Supervisor-mode execution prevention.
        SMEP = 19;
        /// Supervisor-mode access prevention.
        SMAP = 20;
        /// User-mode instruction prevention.
        UMIP = 21;
        /// RDSEED.
        RDSEED = 22;
        /// FSGSBASE instructions.
        FSGSBASE = 23;
        /// TSC runs at a constant rate in all power states.
        INVARIANT_TSC = 24;
    }
}

/// Decoded processor description.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuFeatures {
    /// Vendor string such as `GenuineIntel`.
    pub vendor: [u8; 12],
    /// Display family (extended family added when base family is 0xF).
    pub family: u16,
    /// Display model (extended model included for family 6 and 0xF).
    pub model: u16,
    /// Stepping.
    pub stepping: u8,
    /// Highest basic leaf.
    pub max_basic_leaf: u32,
    /// Highest extended leaf.
    pub max_extended_leaf: u32,
    /// Physical address width in bits (0 if unreported).
    pub phys_bits: u8,
    /// Virtual address width in bits (0 if unreported).
    pub virt_bits: u8,
    /// Bitset of [`feature`] constants.
    pub flags: u64,
}

impl CpuFeatures {
    /// True if every bit of `mask` is present.
    pub fn has(&self, mask: u64) -> bool {
        self.flags & mask == mask
    }

    /// The vendor as text, or `"?"` if it is not ASCII.
    pub fn vendor_str(&self) -> &str {
        core::str::from_utf8(&self.vendor).unwrap_or("?")
    }
}

fn bit(v: u32, n: u32) -> bool {
    v >> n & 1 == 1
}

/// Decodes CPUID. Leaves above the reported maximum are never queried.
pub fn decode<S: CpuidSource>(src: &S) -> CpuFeatures {
    use feature::*;
    let l0 = src.cpuid(0, 0);
    let max_basic = l0[0];
    let mut vendor = [0u8; 12];
    vendor[0..4].copy_from_slice(&l0[1].to_le_bytes());
    vendor[4..8].copy_from_slice(&l0[3].to_le_bytes());
    vendor[8..12].copy_from_slice(&l0[2].to_le_bytes());
    let mut f = CpuFeatures {
        vendor,
        family: 0,
        model: 0,
        stepping: 0,
        max_basic_leaf: max_basic,
        max_extended_leaf: 0,
        phys_bits: 0,
        virt_bits: 0,
        flags: 0,
    };
    let mut set = |cond: bool, mask: u64| {
        if cond {
            f.flags |= mask;
        }
    };
    let mut family = 0u16;
    let mut model = 0u16;
    let mut stepping = 0u8;
    let (mut edx1, mut ecx1) = (0u32, 0u32);
    if max_basic >= 1 {
        let l1 = src.cpuid(1, 0);
        let eax = l1[0];
        let base_family = (eax >> 8 & 0xF) as u16;
        family = base_family;
        model = (eax >> 4 & 0xF) as u16;
        if base_family == 0xF {
            family += (eax >> 20 & 0xFF) as u16;
        }
        if base_family == 0x6 || base_family == 0xF {
            model |= ((eax >> 16 & 0xF) as u16) << 4;
        }
        stepping = (eax & 0xF) as u8;
        ecx1 = l1[2];
        edx1 = l1[3];
    }
    let mut ebx7 = 0u32;
    let mut ecx7 = 0u32;
    if max_basic >= 7 {
        let l7 = src.cpuid(7, 0);
        ebx7 = l7[1];
        ecx7 = l7[2];
    }
    let l8 = src.cpuid(0x8000_0000, 0);
    let max_ext = if l8[0] >= 0x8000_0000 { l8[0] } else { 0 };
    let (mut edx81, mut edx87, mut eax88) = (0u32, 0u32, 0u32);
    if max_ext >= 0x8000_0001 {
        edx81 = src.cpuid(0x8000_0001, 0)[3];
    }
    if max_ext >= 0x8000_0007 {
        edx87 = src.cpuid(0x8000_0007, 0)[3];
    }
    if max_ext >= 0x8000_0008 {
        eax88 = src.cpuid(0x8000_0008, 0)[0];
    }
    set(bit(edx1, 0), FPU);
    set(bit(edx1, 6), PAE);
    set(bit(edx1, 4), TSC);
    set(bit(edx1, 5), MSR);
    set(bit(edx1, 9), APIC);
    set(bit(edx1, 13), PGE);
    set(bit(edx1, 24), FXSR);
    set(bit(edx1, 25), SSE);
    set(bit(edx1, 26), SSE2);
    set(bit(ecx1, 21), X2APIC);
    set(bit(ecx1, 26), XSAVE);
    set(bit(ecx1, 28), AVX);
    set(bit(ecx1, 30), RDRAND);
    set(bit(ecx1, 31), HYPERVISOR);
    set(bit(ecx1, 17), PCID);
    set(bit(ebx7, 0), FSGSBASE);
    set(bit(ebx7, 7), SMEP);
    set(bit(ebx7, 20), SMAP);
    set(bit(ebx7, 18), RDSEED);
    set(bit(ecx7, 2), UMIP);
    set(bit(edx81, 11), SYSCALL);
    set(bit(edx81, 20), NX);
    set(bit(edx81, 26), PAGE1GB);
    set(bit(edx81, 29), LONG_MODE);
    set(bit(edx87, 8), INVARIANT_TSC);
    f.family = family;
    f.model = model;
    f.stepping = stepping;
    f.max_extended_leaf = max_ext;
    if max_ext >= 0x8000_0008 {
        f.phys_bits = (eax88 & 0xFF) as u8;
        f.virt_bits = (eax88 >> 8 & 0xFF) as u8;
    }
    f
}

/// What the EXILE kernel baseline requires: 64-bit mode, NX, SYSCALL, PAE, MSRs, a local APIC, FXSR and SSE2.
pub const EXILE_REQUIRED: u64 = feature::LONG_MODE
    | feature::NX
    | feature::SYSCALL
    | feature::PAE
    | feature::MSR
    | feature::APIC
    | feature::FXSR
    | feature::SSE2;

/// Hardening features the kernel wants when present (their absence is reported, not fatal).
pub const EXILE_HARDENING: u64 = feature::SMEP | feature::SMAP | feature::UMIP;

/// Returns the required features that are missing (0 means the CPU is acceptable).
pub fn missing_required(f: &CpuFeatures) -> u64 {
    EXILE_REQUIRED & !f.flags
}

/// Human-readable name of a single feature bit.
pub fn feature_name(mask: u64) -> &'static str {
    use feature::*;
    match mask {
        FPU => "fpu",
        PAE => "pae",
        TSC => "tsc",
        MSR => "msr",
        APIC => "apic",
        PGE => "pge",
        FXSR => "fxsr",
        SSE => "sse",
        SSE2 => "sse2",
        X2APIC => "x2apic",
        XSAVE => "xsave",
        AVX => "avx",
        RDRAND => "rdrand",
        HYPERVISOR => "hypervisor",
        PCID => "pcid",
        SYSCALL => "syscall",
        NX => "nx",
        PAGE1GB => "1gb-pages",
        LONG_MODE => "long-mode",
        SMEP => "smep",
        SMAP => "smap",
        UMIP => "umip",
        RDSEED => "rdseed",
        FSGSBASE => "fsgsbase",
        INVARIANT_TSC => "invariant-tsc",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        max_basic: u32,
        max_ext: u32,
        edx1: u32,
        ecx1: u32,
        ebx7: u32,
        ecx7: u32,
        edx81: u32,
        eax88: u32,
        edx87: u32,
        eax1: u32,
    }

    impl CpuidSource for Fake {
        fn cpuid(&self, leaf: u32, sub: u32) -> [u32; 4] {
            match (leaf, sub) {
                (0, _) => [
                    self.max_basic,
                    u32::from_le_bytes(*b"Genu"),
                    u32::from_le_bytes(*b"ntel"),
                    u32::from_le_bytes(*b"ineI"),
                ],
                (1, _) => [self.eax1, 0, self.ecx1, self.edx1],
                (7, 0) => [0, self.ebx7, self.ecx7, 0],
                (0x8000_0000, _) => [self.max_ext, 0, 0, 0],
                (0x8000_0001, _) => [0, 0, 0, self.edx81],
                (0x8000_0007, _) => [0, 0, 0, self.edx87],
                (0x8000_0008, _) => [self.eax88, 0, 0, 0],
                _ => [0; 4],
            }
        }
    }

    fn modern() -> Fake {
        Fake {
            max_basic: 0x16,
            max_ext: 0x8000_0008,
            // FPU, MSR, TSC, PAE, APIC, PGE, FXSR, SSE, SSE2
            edx1: 1 | 1 << 4 | 1 << 5 | 1 << 6 | 1 << 9 | 1 << 13 | 1 << 24 | 1 << 25 | 1 << 26,
            ecx1: 1 << 21 | 1 << 26 | 1 << 28 | 1 << 30 | 1 << 17,
            ebx7: 1 | 1 << 7 | 1 << 20 | 1 << 18,
            ecx7: 1 << 2,
            edx81: 1 << 11 | 1 << 20 | 1 << 26 | 1 << 29,
            eax88: 46 | 48 << 8,
            edx87: 1 << 8,
            // family 6, model 0x9E (ext model 9, model 0xE), stepping 0xA
            eax1: 0x0009_06EA,
        }
    }

    #[test]
    fn decodes_a_modern_cpu() {
        let f = decode(&modern());
        assert_eq!(f.vendor_str(), "GenuineIntel");
        assert_eq!((f.family, f.model, f.stepping), (6, 0x9E, 0xA));
        assert_eq!((f.phys_bits, f.virt_bits), (46, 48));
        assert!(f.has(EXILE_REQUIRED));
        assert!(f.has(EXILE_HARDENING));
        assert!(f.has(
            feature::X2APIC
                | feature::PCID
                | feature::INVARIANT_TSC
                | feature::PAGE1GB
                | feature::RDSEED
                | feature::FSGSBASE
        ));
        assert!(!f.has(feature::HYPERVISOR));
        assert_eq!(missing_required(&f), 0);
    }

    #[test]
    fn extended_family_and_model_rules() {
        let mut m = modern();
        m.eax1 = 0x00A2_0F15; // AMD-style: base family 0xF, ext family 0x0A => 0x19; ext model 2, model 1 => 0x21
        let f = decode(&m);
        assert_eq!(f.family, 0x19);
        assert_eq!(f.model, 0x21);
        m.eax1 = 0x0000_0540; // family 5: ext fields ignored
        let f = decode(&m);
        assert_eq!((f.family, f.model), (5, 4));
    }

    #[test]
    fn missing_features_are_reported_not_guessed() {
        let mut m = modern();
        m.edx81 = 1 << 11; // SYSCALL only: no NX, no long mode
        let f = decode(&m);
        assert_eq!(missing_required(&f), feature::NX | feature::LONG_MODE);
        assert_eq!(feature_name(feature::NX), "nx");
    }

    #[test]
    fn leaves_above_the_reported_maximum_are_not_trusted() {
        let mut m = modern();
        m.max_basic = 6; // leaf 7 must not be read even though the fake would answer it
        m.max_ext = 0x8000_0001;
        let f = decode(&m);
        assert!(!f.has(feature::SMEP | feature::SMAP | feature::UMIP));
        assert!(!f.has(feature::INVARIANT_TSC));
        assert_eq!((f.phys_bits, f.virt_bits), (0, 0));
        m.max_ext = 0x1234; // below 0x8000_0000: no extended leaves at all
        let f = decode(&m);
        assert_eq!(f.max_extended_leaf, 0);
        assert!(!f.has(feature::LONG_MODE));
    }

    #[test]
    fn empty_cpu_has_no_features() {
        let f = decode(&Fake {
            max_basic: 0,
            max_ext: 0,
            edx1: 0,
            ecx1: 0,
            ebx7: 0,
            ecx7: 0,
            edx81: 0,
            eax88: 0,
            edx87: 0,
            eax1: 0,
        });
        assert_eq!(f.flags, 0);
        assert_eq!(missing_required(&f), EXILE_REQUIRED);
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn decodes_the_processor_running_the_tests() {
        // Real CPUID output from the (virtual) CPU of the machine running the tests.
        let f = decode(&HardwareCpuid);
        assert!(
            f.vendor.iter().all(|b| b.is_ascii_graphic()),
            "vendor {:?}",
            f.vendor
        );
        assert!(f.max_basic_leaf >= 1);
        assert!(
            f.has(feature::LONG_MODE),
            "a 64-bit process cannot be running without long mode"
        );
        assert!(f.has(feature::SSE2));
        assert!(
            f.phys_bits >= 32 && f.virt_bits >= 32,
            "phys={} virt={}",
            f.phys_bits,
            f.virt_bits
        );
        std::eprintln!(
            "host CPU: {} family {} model {:#x} flags {:#x} (missing required: {:#x})",
            f.vendor_str(),
            f.family,
            f.model,
            f.flags,
            missing_required(&f)
        );
    }
}
