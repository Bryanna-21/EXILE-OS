//! Linear framebuffer descriptor validation and pixel access over a caller-provided byte slice.
//!
//! Status: pure software. Works on any `&mut [u8]`; does not map or discover hardware. Tested on
//! heap buffers only (no real GOP/VBE framebuffer, no QEMU).

/// Pixel layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Bytes: B, G, R, X.
    Bgrx32,
    /// Bytes: R, G, B, X.
    Rgbx32,
}

/// Framebuffer descriptor from the bootloader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FbInfo {
    /// Width px.
    pub width: u32,
    /// Height px.
    pub height: u32,
    /// Bytes per scanline (may exceed width*4).
    pub pitch: u32,
    /// Format.
    pub format: Format,
}

/// Errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FbError {
    /// Zero or absurd dimensions.
    BadGeometry,
    /// Pitch smaller than a row.
    PitchTooSmall,
    /// Backing memory smaller than pitch*height.
    BufferTooSmall,
}

/// Validated framebuffer.
pub struct Framebuffer<'a> {
    info: FbInfo,
    mem: &'a mut [u8],
}

impl<'a> Framebuffer<'a> {
    /// Validate geometry against the backing slice (overflow-safe).
    pub fn new(info: FbInfo, mem: &'a mut [u8]) -> Result<Self, FbError> {
        if info.width == 0 || info.height == 0 || info.width > 16384 || info.height > 16384 {
            return Err(FbError::BadGeometry);
        }
        let row = u64::from(info.width) * 4;
        if u64::from(info.pitch) < row {
            return Err(FbError::PitchTooSmall);
        }
        let need = u64::from(info.pitch) * u64::from(info.height);
        if need > mem.len() as u64 {
            return Err(FbError::BufferTooSmall);
        }
        Ok(Framebuffer { info, mem })
    }
    fn enc(&self, r: u8, g: u8, b: u8) -> [u8; 4] {
        match self.info.format {
            Format::Bgrx32 => [b, g, r, 0],
            Format::Rgbx32 => [r, g, b, 0],
        }
    }
    /// Set a pixel; returns false (and writes nothing) if out of bounds.
    pub fn put(&mut self, x: u32, y: u32, r: u8, g: u8, b: u8) -> bool {
        if x >= self.info.width || y >= self.info.height {
            return false;
        }
        let o = y as usize * self.info.pitch as usize + x as usize * 4;
        let p = self.enc(r, g, b);
        self.mem[o..o + 4].copy_from_slice(&p);
        true
    }
    /// Fill a rectangle, clipped to the screen.
    pub fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, rgb: [u8; 3]) {
        let [r, g, b] = rgb;
        let x1 = x.saturating_add(w).min(self.info.width);
        let y1 = y.saturating_add(h).min(self.info.height);
        for yy in y..y1 {
            for xx in x..x1 {
                self.put(xx, yy, r, g, b);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn info(pitch: u32) -> FbInfo {
        FbInfo {
            width: 4,
            height: 3,
            pitch,
            format: Format::Bgrx32,
        }
    }
    #[test]
    fn validation_rules() {
        let mut m = std::vec![0u8; 64];
        assert!(Framebuffer::new(info(16), &mut m).is_ok());
        assert_eq!(
            Framebuffer::new(info(12), &mut m).err(),
            Some(FbError::PitchTooSmall)
        );
        assert_eq!(
            Framebuffer::new(info(32), &mut m).err(),
            Some(FbError::BufferTooSmall)
        );
        assert_eq!(
            Framebuffer::new(
                FbInfo {
                    width: 0,
                    ..info(16)
                },
                &mut m
            )
            .err(),
            Some(FbError::BadGeometry)
        );
        let huge = FbInfo {
            width: 16384,
            height: 16384,
            pitch: u32::MAX,
            format: Format::Bgrx32,
        };
        assert_eq!(
            Framebuffer::new(huge, &mut m).err(),
            Some(FbError::BufferTooSmall),
            "no overflow wrap"
        );
    }
    #[test]
    fn pixel_layout_and_pitch_padding() {
        let mut m = std::vec![0xEEu8; 24 * 3];
        let mut f = Framebuffer::new(info(24), &mut m).unwrap();
        assert!(f.put(1, 2, 1, 2, 3));
        assert!(!f.put(4, 0, 9, 9, 9) && !f.put(0, 3, 9, 9, 9));
        assert_eq!(&m[2 * 24 + 4..2 * 24 + 8], &[3, 2, 1, 0]);
        assert_eq!(m[2 * 24 + 16], 0xEE, "padding bytes must be untouched");
    }
    #[test]
    fn rgb_format_and_clipped_fill() {
        let mut m = std::vec![0u8; 16 * 3];
        let mut f = Framebuffer::new(
            FbInfo {
                format: Format::Rgbx32,
                ..info(16)
            },
            &mut m,
        )
        .unwrap();
        f.fill(2, 1, 100, 100, [7, 8, 9]);
        assert_eq!(&m[16 + 8..16 + 12], &[7, 8, 9, 0]);
        assert_eq!(&m[16..16 + 4], &[0, 0, 0, 0], "outside rect untouched");
        let mut m2 = std::vec![0u8; 48];
        Framebuffer::new(info(16), &mut m2)
            .unwrap()
            .fill(u32::MAX, 0, 5, 5, [1, 1, 1]);
        assert!(m2.iter().all(|b| *b == 0));
    }
}
