//! CPU identification and capability policy (x86-64). SPDX-License-Identifier: Apache-2.0
//!
//! Detection is separated from execution: `Features::detect_with` takes a CPUID reader so the
//! decode logic is host-testable with synthetic CPUs, and it never asks for a leaf above the
//! maximum the CPU reports. *Detected* features (what CPUID advertises) are kept apart from
//! *enabled* features (what the kernel has configured); `CpuCapabilities::mark_enabled` refuses
//! to enable something that was not detected. Nothing here writes a control register.
#![allow(dead_code)]

/// Raw CPUID output registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CpuidLeaf {
    pub eax: u32,
    pub ebx: u32,
    pub ecx: u32,
    pub edx: u32,
}

/// Executes CPUID for `leaf`/`subleaf`.
///
/// # Safety
/// CPUID must exist. Every x86-64 CPU has it; the EUK bootstrap additionally verifies it before
/// entering long mode. Callers must not request leaves above the maximum reported by leaf 0 or
/// 0x8000_0000; `Features::detect_with` enforces that.
#[cfg(target_arch = "x86_64")]
#[allow(unused_unsafe)]
pub unsafe fn cpuid(leaf: u32, subleaf: u32) -> CpuidLeaf {
    // SAFETY: see function contract. core::arch preserves RBX, which LLVM reserves.
    let r = unsafe { core::arch::x86_64::__cpuid_count(leaf, subleaf) };
    CpuidLeaf {
        eax: r.eax,
        ebx: r.ebx,
        ecx: r.ecx,
        edx: r.edx,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Intel,
    Amd,
    Unknown,
}

/// Vendor string is returned in EBX, EDX, ECX order.
pub const fn vendor_from_regs(ebx: u32, edx: u32, ecx: u32) -> Vendor {
    if ebx == 0x756e_6547 && edx == 0x4965_6e69 && ecx == 0x6c65_746e {
        Vendor::Intel // "GenuineIntel"
    } else if ebx == 0x6874_7541 && edx == 0x6974_6e65 && ecx == 0x444d_4163 {
        Vendor::Amd // "AuthenticAMD"
    } else {
        Vendor::Unknown
    }
}

/// Decoded family/model/stepping from leaf 1 EAX.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signature {
    pub family: u32,
    pub model: u32,
    pub stepping: u32,
}

impl Signature {
    /// Extended family is added when the base family is 0xF. Extended model is prepended when
    /// the base family is 0xF (all vendors) or 0x6 (Intel only).
    pub const fn decode(eax: u32, vendor: Vendor) -> Self {
        let stepping = eax & 0xf;
        let base_model = (eax >> 4) & 0xf;
        let base_family = (eax >> 8) & 0xf;
        let ext_model = (eax >> 16) & 0xf;
        let ext_family = (eax >> 20) & 0xff;
        let family = if base_family == 0xf {
            base_family + ext_family
        } else {
            base_family
        };
        let use_ext_model =
            base_family == 0xf || (base_family == 6 && matches!(vendor, Vendor::Intel));
        let model = if use_ext_model {
            base_model | (ext_model << 4)
        } else {
            base_model
        };
        Self {
            family,
            model,
            stepping,
        }
    }
}

/// Which CPUID register a feature bit lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reg {
    L1Edx,
    L1Ecx,
    L7Ebx,
    L7Ecx,
    E1Edx,
    E1Ecx,
}

/// CPU features the kernel may query. Bit positions follow Intel SDM Vol. 2 CPUID and AMD APM Vol. 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Feature {
    Fpu,
    Pse,
    Tsc,
    Msr,
    Pae,
    Mce,
    Apic,
    Pge,
    Mca,
    Pat,
    Fxsr,
    Sse,
    Sse2,
    Sse3,
    Ssse3,
    Sse41,
    Sse42,
    Pcid,
    X2apic,
    TscDeadline,
    Xsave,
    /// CPUID reports CR4.OSXSAVE here; it reflects OS configuration, not hardware capability.
    Osxsave,
    Avx,
    Rdrand,
    Hypervisor,
    Vmx,
    Fsgsbase,
    Avx2,
    Smep,
    Invpcid,
    Smap,
    Umip,
    Svm,
    Nx,
    Page1Gib,
    Rdtscp,
    LongMode,
}

const ALL: [Feature; 37] = [
    Feature::Fpu,
    Feature::Pse,
    Feature::Tsc,
    Feature::Msr,
    Feature::Pae,
    Feature::Mce,
    Feature::Apic,
    Feature::Pge,
    Feature::Mca,
    Feature::Pat,
    Feature::Fxsr,
    Feature::Sse,
    Feature::Sse2,
    Feature::Sse3,
    Feature::Ssse3,
    Feature::Sse41,
    Feature::Sse42,
    Feature::Pcid,
    Feature::X2apic,
    Feature::TscDeadline,
    Feature::Xsave,
    Feature::Osxsave,
    Feature::Avx,
    Feature::Rdrand,
    Feature::Hypervisor,
    Feature::Vmx,
    Feature::Fsgsbase,
    Feature::Avx2,
    Feature::Smep,
    Feature::Invpcid,
    Feature::Smap,
    Feature::Umip,
    Feature::Svm,
    Feature::Nx,
    Feature::Page1Gib,
    Feature::Rdtscp,
    Feature::LongMode,
];

impl Feature {
    const fn location(self) -> (Reg, u8) {
        match self {
            Feature::Fpu => (Reg::L1Edx, 0),
            Feature::Pse => (Reg::L1Edx, 3),
            Feature::Tsc => (Reg::L1Edx, 4),
            Feature::Msr => (Reg::L1Edx, 5),
            Feature::Pae => (Reg::L1Edx, 6),
            Feature::Mce => (Reg::L1Edx, 7),
            Feature::Apic => (Reg::L1Edx, 9),
            Feature::Pge => (Reg::L1Edx, 13),
            Feature::Mca => (Reg::L1Edx, 14),
            Feature::Pat => (Reg::L1Edx, 16),
            Feature::Fxsr => (Reg::L1Edx, 24),
            Feature::Sse => (Reg::L1Edx, 25),
            Feature::Sse2 => (Reg::L1Edx, 26),
            Feature::Sse3 => (Reg::L1Ecx, 0),
            Feature::Vmx => (Reg::L1Ecx, 5),
            Feature::Ssse3 => (Reg::L1Ecx, 9),
            Feature::Pcid => (Reg::L1Ecx, 17),
            Feature::Sse41 => (Reg::L1Ecx, 19),
            Feature::Sse42 => (Reg::L1Ecx, 20),
            Feature::X2apic => (Reg::L1Ecx, 21),
            Feature::TscDeadline => (Reg::L1Ecx, 24),
            Feature::Xsave => (Reg::L1Ecx, 26),
            Feature::Osxsave => (Reg::L1Ecx, 27),
            Feature::Avx => (Reg::L1Ecx, 28),
            Feature::Rdrand => (Reg::L1Ecx, 30),
            Feature::Hypervisor => (Reg::L1Ecx, 31),
            Feature::Fsgsbase => (Reg::L7Ebx, 0),
            Feature::Avx2 => (Reg::L7Ebx, 5),
            Feature::Smep => (Reg::L7Ebx, 7),
            Feature::Invpcid => (Reg::L7Ebx, 10),
            Feature::Smap => (Reg::L7Ebx, 20),
            Feature::Umip => (Reg::L7Ecx, 2),
            Feature::Svm => (Reg::E1Ecx, 2),
            Feature::Nx => (Reg::E1Edx, 20),
            Feature::Page1Gib => (Reg::E1Edx, 26),
            Feature::Rdtscp => (Reg::E1Edx, 27),
            Feature::LongMode => (Reg::E1Edx, 29),
        }
    }
}

/// Raw feature registers as reported by the CPU. A register for a leaf the CPU does not
/// implement stays zero, so every feature in it reads as absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Features {
    l1_edx: u32,
    l1_ecx: u32,
    l7_ebx: u32,
    l7_ecx: u32,
    e1_edx: u32,
    e1_ecx: u32,
}

impl Features {
    fn reg(&self, r: Reg) -> u32 {
        match r {
            Reg::L1Edx => self.l1_edx,
            Reg::L1Ecx => self.l1_ecx,
            Reg::L7Ebx => self.l7_ebx,
            Reg::L7Ecx => self.l7_ecx,
            Reg::E1Edx => self.e1_edx,
            Reg::E1Ecx => self.e1_ecx,
        }
    }

    pub fn has(&self, f: Feature) -> bool {
        let (r, bit) = f.location();
        self.reg(r) >> bit & 1 != 0
    }
}

/// Everything CPUID tells the kernel about the boot processor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuInfo {
    pub vendor: Vendor,
    pub signature: Signature,
    pub max_basic_leaf: u32,
    pub max_extended_leaf: u32,
    /// Initial APIC ID (leaf 1 EBX[31:24]). Not a kernel CPU id; translate before use.
    pub initial_apic_id: u8,
    /// Logical processors per package as reported by leaf 1 (valid only if `htt`).
    pub logical_per_package: u8,
    pub htt: bool,
    pub features: Features,
}

impl CpuInfo {
    /// Queries only leaves the CPU reports as present. Returns `None` if leaf 1 is missing,
    /// which no x86-64 CPU should do.
    pub fn detect_with(mut read: impl FnMut(u32, u32) -> CpuidLeaf) -> Option<Self> {
        let l0 = read(0, 0);
        let max_basic_leaf = l0.eax;
        if max_basic_leaf < 1 {
            return None;
        }
        let vendor = vendor_from_regs(l0.ebx, l0.edx, l0.ecx);
        let l1 = read(1, 0);
        let mut features = Features {
            l1_edx: l1.edx,
            l1_ecx: l1.ecx,
            ..Features::default()
        };
        if max_basic_leaf >= 7 {
            let l7 = read(7, 0);
            features.l7_ebx = l7.ebx;
            features.l7_ecx = l7.ecx;
        }
        let max_extended_leaf = read(0x8000_0000, 0).eax;
        // Extended leaves start at 0x8000_0000; a value below it means "none".
        let max_extended_leaf = if max_extended_leaf >= 0x8000_0000 {
            max_extended_leaf
        } else {
            0
        };
        if max_extended_leaf >= 0x8000_0001 {
            let e1 = read(0x8000_0001, 0);
            features.e1_edx = e1.edx;
            features.e1_ecx = e1.ecx;
        }
        Some(Self {
            vendor,
            signature: Signature::decode(l1.eax, vendor),
            max_basic_leaf,
            max_extended_leaf,
            initial_apic_id: (l1.ebx >> 24) as u8,
            logical_per_package: (l1.ebx >> 16) as u8,
            htt: l1.edx >> 28 & 1 != 0,
            features,
        })
    }

    /// Detects on the running CPU.
    ///
    /// # Safety
    /// See [`cpuid`].
    #[cfg(target_arch = "x86_64")]
    pub unsafe fn detect() -> Option<Self> {
        // SAFETY: forwarded contract; detect_with bounds every leaf by the reported maximum.
        Self::detect_with(|leaf, sub| unsafe { cpuid(leaf, sub) })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnableError {
    /// The CPU did not advertise the feature, so the kernel must not turn it on.
    NotDetected,
}

/// Central capability record: detected (hardware) versus enabled (kernel-configured).
#[derive(Debug, Clone, Copy)]
pub struct CpuCapabilities {
    pub info: CpuInfo,
    enabled: u64,
}

impl CpuCapabilities {
    pub const fn new(info: CpuInfo) -> Self {
        Self { info, enabled: 0 }
    }

    pub fn detected(&self, f: Feature) -> bool {
        self.info.features.has(f)
    }

    pub fn enabled(&self, f: Feature) -> bool {
        self.enabled >> (f as u8) & 1 != 0
    }

    /// Records that the kernel has actually configured `f`. Call only after the corresponding
    /// control-register/MSR setup has succeeded. AVX additionally needs XSAVE state management
    /// (XCR0), which is a later batch; this method does not validate that policy.
    pub fn mark_enabled(&mut self, f: Feature) -> Result<(), EnableError> {
        if !self.detected(f) {
            return Err(EnableError::NotDetected);
        }
        self.enabled |= 1u64 << (f as u8);
        Ok(())
    }

    /// Features the kernel's bootstrap contract requires of the boot CPU, as an iterator of the
    /// first missing one (None means all present).
    pub fn first_missing_required(&self) -> Option<Feature> {
        const REQUIRED: [Feature; 9] = [
            Feature::Pse,
            Feature::Msr,
            Feature::Pae,
            Feature::Fxsr,
            Feature::Sse,
            Feature::Sse2,
            Feature::Nx,
            Feature::LongMode,
            Feature::Fpu,
        ];
        REQUIRED.into_iter().find(|f| !self.detected(*f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTEL: (u32, u32, u32) = (0x756e_6547, 0x4965_6e69, 0x6c65_746e);

    fn fake(
        max_basic: u32,
        vendor: (u32, u32, u32),
        l1: CpuidLeaf,
        l7: CpuidLeaf,
        max_ext: u32,
        e1: CpuidLeaf,
        log: &mut Vec<(u32, u32)>,
    ) -> impl FnMut(u32, u32) -> CpuidLeaf + '_ {
        move |leaf, sub| {
            log.push((leaf, sub));
            match leaf {
                0 => CpuidLeaf {
                    eax: max_basic,
                    ebx: vendor.0,
                    edx: vendor.1,
                    ecx: vendor.2,
                },
                1 => l1,
                7 => l7,
                0x8000_0000 => CpuidLeaf {
                    eax: max_ext,
                    ..Default::default()
                },
                0x8000_0001 => e1,
                _ => panic!("unexpected leaf {leaf:#x}"),
            }
        }
    }

    #[test]
    fn vendors() {
        assert_eq!(vendor_from_regs(INTEL.0, INTEL.1, INTEL.2), Vendor::Intel);
        assert_eq!(
            vendor_from_regs(0x6874_7541, 0x6974_6e65, 0x444d_4163),
            Vendor::Amd
        );
        assert_eq!(vendor_from_regs(0, 0, 0), Vendor::Unknown);
        // All three words must match.
        assert_eq!(vendor_from_regs(INTEL.0, INTEL.1, 0), Vendor::Unknown);
    }

    #[test]
    fn signature_intel_family6_extended_model() {
        // Family 6, ext model 9, model 7, stepping 1 => model 0x97.
        let s = Signature::decode(0x0009_0671, Vendor::Intel);
        assert_eq!((s.family, s.model, s.stepping), (6, 0x97, 1));
    }

    #[test]
    fn signature_amd_family6_ignores_extended_model() {
        let s = Signature::decode(0x0009_0671, Vendor::Amd);
        assert_eq!((s.family, s.model, s.stepping), (6, 7, 1));
    }

    #[test]
    fn signature_family_f_adds_extended_family_and_model() {
        // Base family F, ext family 0x8, ext model 1, base model 1, stepping 2 => family 0x17, model 0x11.
        let s = Signature::decode(0x0081_0F12, Vendor::Amd);
        assert_eq!((s.family, s.model, s.stepping), (0x17, 0x11, 2));
    }

    #[test]
    fn signature_plain() {
        let s = Signature::decode(0x0000_0633, Vendor::Unknown);
        assert_eq!((s.family, s.model, s.stepping), (6, 3, 3));
    }

    #[test]
    fn feature_bits_decode() {
        let l1 = CpuidLeaf {
            eax: 0x0000_0f12,
            ebx: 0x0310_0000, // APIC ID 3, 0x10 logical processors
            ecx: (1 << 26) | (1 << 28) | (1 << 21),
            edx: (1 << 0)
                | (1 << 3)
                | (1 << 5)
                | (1 << 6)
                | (1 << 9)
                | (1 << 24)
                | (1 << 25)
                | (1 << 26)
                | (1 << 28),
        };
        let l7 = CpuidLeaf {
            ebx: (1 << 5) | (1 << 7),
            ..Default::default()
        };
        let e1 = CpuidLeaf {
            edx: (1 << 20) | (1 << 29),
            ..Default::default()
        };
        let mut log = Vec::new();
        let info = CpuInfo::detect_with(fake(7, INTEL, l1, l7, 0x8000_0008, e1, &mut log)).unwrap();
        for f in [
            Feature::Fpu,
            Feature::Pse,
            Feature::Msr,
            Feature::Pae,
            Feature::Apic,
            Feature::Fxsr,
            Feature::Sse,
            Feature::Sse2,
            Feature::Xsave,
            Feature::Avx,
            Feature::X2apic,
            Feature::Avx2,
            Feature::Smep,
            Feature::Nx,
            Feature::LongMode,
        ] {
            assert!(info.features.has(f), "{f:?}");
        }
        for f in [
            Feature::Osxsave,
            Feature::Smap,
            Feature::Svm,
            Feature::Page1Gib,
            Feature::Rdtscp,
            Feature::Hypervisor,
        ] {
            assert!(!info.features.has(f), "{f:?}");
        }
        assert_eq!(info.initial_apic_id, 3);
        assert_eq!(info.logical_per_package, 0x10);
        assert!(info.htt);
        assert_eq!(info.vendor, Vendor::Intel);
    }

    #[test]
    fn no_leaf_above_reported_maximum() {
        // max basic = 1 and no extended leaves: leaf 7 and 0x8000_0001 must never be queried.
        let mut log = Vec::new();
        let info = CpuInfo::detect_with(fake(
            1,
            INTEL,
            CpuidLeaf::default(),
            CpuidLeaf {
                ebx: !0,
                ecx: !0,
                edx: !0,
                eax: 0,
            },
            0, // below 0x8000_0000 means no extended range
            CpuidLeaf {
                edx: !0,
                ecx: !0,
                ..Default::default()
            },
            &mut log,
        ))
        .unwrap();
        assert!(
            log.iter().all(|(l, _)| matches!(*l, 0 | 1 | 0x8000_0000)),
            "{log:?}"
        );
        assert_eq!(info.max_extended_leaf, 0);
        assert!(!info.features.has(Feature::Avx2));
        assert!(!info.features.has(Feature::Nx));
    }

    #[test]
    fn missing_leaf_one_is_rejected() {
        let mut log = Vec::new();
        assert!(
            CpuInfo::detect_with(fake(
                0,
                INTEL,
                CpuidLeaf::default(),
                CpuidLeaf::default(),
                0,
                CpuidLeaf::default(),
                &mut log
            ))
            .is_none()
        );
    }

    fn caps(l1_ecx: u32, l1_edx: u32) -> CpuCapabilities {
        let mut log = Vec::new();
        let l1 = CpuidLeaf {
            ecx: l1_ecx,
            edx: l1_edx,
            ..Default::default()
        };
        let e1 = CpuidLeaf {
            edx: (1 << 20) | (1 << 29),
            ..Default::default()
        };
        CpuCapabilities::new(
            CpuInfo::detect_with(fake(
                1,
                INTEL,
                l1,
                CpuidLeaf::default(),
                0x8000_0001,
                e1,
                &mut log,
            ))
            .unwrap(),
        )
    }

    #[test]
    fn detected_is_separate_from_enabled() {
        let mut c = caps(1 << 28, 1 << 25);
        assert!(c.detected(Feature::Avx));
        assert!(!c.enabled(Feature::Avx));
        c.mark_enabled(Feature::Avx).unwrap();
        assert!(c.enabled(Feature::Avx));
        assert!(
            !c.enabled(Feature::Sse),
            "enabling one feature must not enable another"
        );
    }

    #[test]
    fn cannot_enable_undetected_feature() {
        let mut c = caps(0, 0);
        assert_eq!(c.mark_enabled(Feature::Avx), Err(EnableError::NotDetected));
        assert!(!c.enabled(Feature::Avx));
    }

    #[test]
    fn every_feature_has_unique_slot_and_fits_enabled_mask() {
        for (i, a) in ALL.iter().enumerate() {
            assert!((*a as u8) < 64);
            for b in &ALL[i + 1..] {
                assert_ne!(*a as u8, *b as u8, "{a:?} {b:?}");
                assert_ne!(a.location(), b.location(), "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn required_feature_policy() {
        let all = (1 << 0) | (1 << 3) | (1 << 5) | (1 << 6) | (1 << 24) | (1 << 25) | (1 << 26);
        assert_eq!(caps(0, all).first_missing_required(), None);
        assert_eq!(
            caps(0, all & !(1 << 6)).first_missing_required(),
            Some(Feature::Pae)
        );
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn live_cpuid_is_self_consistent() {
        // Runs the real instruction on the host. Proves the wrapper and decode path work, not
        // that any particular feature exists.
        let info = unsafe { CpuInfo::detect() }.expect("leaf 1");
        assert!(info.max_basic_leaf >= 1);
        assert!(
            info.features.has(Feature::LongMode),
            "a 64-bit host must report long mode"
        );
    }
}
