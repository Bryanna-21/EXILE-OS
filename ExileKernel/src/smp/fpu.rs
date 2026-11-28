//! FPU/SSE/XSAVE state layout validation for per-thread context management.

pub const LEGACY_FX_STATE_BYTES: usize = 512;
pub const MIN_XSAVE_BYTES: usize = LEGACY_FX_STATE_BYTES + 64;
pub const MAX_XSAVE_BYTES: usize = 16 * 1024;
pub const XCR0_X87: u64 = 1;
pub const XCR0_SSE: u64 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FpuError {
    MissingX87OrSse,
    XsaveUnavailable,
    InvalidStateSize,
    UnsupportedXcr0,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FpuFeatures {
    pub xsave: bool,
    pub osxsave: bool,
    pub xsave_size: usize,
    pub xcr0_supported: u64,
}
impl FpuFeatures {
    pub fn validate(self) -> Result<(), FpuError> {
        if self.xcr0_supported & (XCR0_X87 | XCR0_SSE) != (XCR0_X87 | XCR0_SSE) {
            return Err(FpuError::MissingX87OrSse);
        }
        if self.xsave != self.osxsave {
            return Err(FpuError::XsaveUnavailable);
        }
        if self.xsave
            && (self.xsave_size < MIN_XSAVE_BYTES
                || self.xsave_size > MAX_XSAVE_BYTES
                || self.xsave_size % 64 != 0)
        {
            return Err(FpuError::InvalidStateSize);
        }
        Ok(())
    }
    pub fn validate_xcr0(self, requested: u64) -> Result<(), FpuError> {
        self.validate()?;
        if requested & !self.xcr0_supported != 0
            || requested & (XCR0_X87 | XCR0_SSE) != (XCR0_X87 | XCR0_SSE)
        {
            return Err(FpuError::UnsupportedXcr0);
        }
        Ok(())
    }
}

#[repr(C, align(64))]
pub struct FpuState<const N: usize> {
    bytes: [u8; N],
}
impl<const N: usize> FpuState<N> {
    pub const fn zeroed() -> Result<Self, FpuError> {
        // This container also supports the 512-byte legacy FXSAVE format.
        // XSAVE callers must separately validate the CPU-reported XSAVE size.
        if N < LEGACY_FX_STATE_BYTES || N > MAX_XSAVE_BYTES || N % 64 != 0 {
            return Err(FpuError::InvalidStateSize);
        }
        Ok(Self { bytes: [0; N] })
    }
    pub fn as_bytes(&self) -> &[u8; N] {
        &self.bytes
    }
    pub fn as_bytes_mut(&mut self) -> &mut [u8; N] {
        &mut self.bytes
    }
    pub const fn size(&self) -> usize {
        N
    }
}

pub trait FpuContextOps {
    /// # Safety
    /// Must execute on the owning CPU with the required CR0/CR4/XCR0 state configured.
    unsafe fn save(&mut self, destination: &mut [u8]);
    /// # Safety
    /// `source` must contain a correctly sized, initialized state image for this CPU.
    unsafe fn restore(&mut self, source: &[u8]);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_supported_xsave_configuration() {
        let features = FpuFeatures {
            xsave: true,
            osxsave: true,
            xsave_size: 1024,
            xcr0_supported: 7,
        };
        assert!(features.validate_xcr0(3).is_ok());
    }
    #[test]
    fn rejects_missing_sse_support() {
        let features = FpuFeatures {
            xsave: false,
            osxsave: false,
            xsave_size: 512,
            xcr0_supported: 1,
        };
        assert_eq!(features.validate(), Err(FpuError::MissingX87OrSse));
    }
    #[test]
    fn rejects_xsave_area_smaller_than_legacy_region_and_header() {
        let features = FpuFeatures {
            xsave: true,
            osxsave: true,
            xsave_size: 512,
            xcr0_supported: 3,
        };
        assert_eq!(features.validate(), Err(FpuError::InvalidStateSize));
        assert!(
            FpuFeatures {
                xsave: true,
                osxsave: true,
                xsave_size: 576,
                xcr0_supported: 3
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn validates_aligned_state_buffer_size() {
        assert!(FpuState::<512>::zeroed().is_ok());
        assert!(FpuState::<576>::zeroed().is_ok());
        assert_eq!(
            FpuState::<513>::zeroed().err(),
            Some(FpuError::InvalidStateSize)
        );
    }
}
