//! Framebuffer geometry, colour packing and checked pixel addressing.
//!
//! The kernel must never trust the bootloader-provided framebuffer dimensions
//! blindly. These helpers validate pitch/stride, pixel format and bounds, and
//! return `Result` so the caller can refuse to draw instead of corrupting
//! memory.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelFormat {
    pub bpp: u16,
    pub red_mask_size: u8,
    pub red_mask_shift: u8,
    pub green_mask_size: u8,
    pub green_mask_shift: u8,
    pub blue_mask_size: u8,
    pub blue_mask_shift: u8,
}

impl PixelFormat {
    pub const fn new(
        bpp: u16,
        red_mask_size: u8,
        red_mask_shift: u8,
        green_mask_size: u8,
        green_mask_shift: u8,
        blue_mask_size: u8,
        blue_mask_shift: u8,
    ) -> Self {
        Self {
            bpp,
            red_mask_size,
            red_mask_shift,
            green_mask_size,
            green_mask_shift,
            blue_mask_size,
            blue_mask_shift,
        }
    }

    /// Bytes per pixel for the pixel depths limine and the VM firmware expose
    /// in practice. Sub-byte formats are intentionally rejected.
    pub const fn bytes_per_pixel(&self) -> Option<u64> {
        match self.bpp {
            16 => Some(2),
            24 => Some(3),
            32 => Some(4),
            _ => None,
        }
    }

    fn channel_fits(size: u8, shift: u8, bpp: u16) -> bool {
        size > 0 && size <= 8 && (shift as u16) + (size as u16) <= bpp
    }

    pub fn validate(&self) -> Result<(), FbError> {
        if self.bytes_per_pixel().is_none() {
            return Err(FbError::UnsupportedBpp(self.bpp));
        }
        for (size, shift) in [
            (self.red_mask_size, self.red_mask_shift),
            (self.green_mask_size, self.green_mask_shift),
            (self.blue_mask_size, self.blue_mask_shift),
        ] {
            if !Self::channel_fits(size, shift, self.bpp) {
                return Err(FbError::BadChannelMask);
            }
        }
        Ok(())
    }

    fn scale_channel(value: u8, size: u8) -> u32 {
        if size == 8 {
            return value as u32;
        }
        let max = (1u32 << size) - 1;
        (value as u32 * max + 127) / 255
    }

    /// Pack an 8-bit-per-channel colour into the framebuffer's native pixel
    /// word. The result is meaningful only after `validate` has succeeded.
    pub fn pack_rgb(&self, red: u8, green: u8, blue: u8) -> Option<u32> {
        self.validate().ok()?;
        let r = Self::scale_channel(red, self.red_mask_size) << self.red_mask_shift;
        let g = Self::scale_channel(green, self.green_mask_size) << self.green_mask_shift;
        let b = Self::scale_channel(blue, self.blue_mask_size) << self.blue_mask_shift;
        Some(r | g | b)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FramebufferGeometry {
    pub width: u64,
    pub height: u64,
    /// Bytes from the start of one scanline to the start of the next.
    pub pitch: u64,
    pub format: PixelFormat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FbError {
    ZeroWidth,
    ZeroHeight,
    UnsupportedBpp(u16),
    BadChannelMask,
    PitchTooSmall { pitch: u64, required: u64 },
    SizeOverflow,
    OutOfBounds { x: u64, y: u64 },
}

impl FramebufferGeometry {
    pub const fn new(width: u64, height: u64, pitch: u64, format: PixelFormat) -> Self {
        Self {
            width,
            height,
            pitch,
            format,
        }
    }

    pub fn bytes_per_pixel(&self) -> Option<u64> {
        self.format.bytes_per_pixel()
    }

    pub fn line_bytes(&self) -> Result<u64, FbError> {
        let bytes = self
            .bytes_per_pixel()
            .ok_or(FbError::UnsupportedBpp(self.format.bpp))?;
        self.width.checked_mul(bytes).ok_or(FbError::SizeOverflow)
    }

    pub fn validate(&self) -> Result<(), FbError> {
        if self.width == 0 {
            return Err(FbError::ZeroWidth);
        }
        if self.height == 0 {
            return Err(FbError::ZeroHeight);
        }
        self.format.validate()?;
        let required = self.line_bytes()?;
        if self.pitch < required {
            return Err(FbError::PitchTooSmall {
                pitch: self.pitch,
                required,
            });
        }
        self.framebuffer_size().map(|_| ())
    }

    /// Total number of bytes touched by the visible pixels, accounting for a
    /// pitch that may be larger than the visible line.
    pub fn framebuffer_size(&self) -> Result<u64, FbError> {
        self.validate_without_size()?;
        self.pitch
            .checked_mul(self.height)
            .ok_or(FbError::SizeOverflow)
    }

    fn validate_without_size(&self) -> Result<(), FbError> {
        if self.width == 0 {
            return Err(FbError::ZeroWidth);
        }
        if self.height == 0 {
            return Err(FbError::ZeroHeight);
        }
        self.format.validate()?;
        let required = self.line_bytes()?;
        if self.pitch < required {
            return Err(FbError::PitchTooSmall {
                pitch: self.pitch,
                required,
            });
        }
        Ok(())
    }

    /// Byte offset of the top-left corner of pixel `(x, y)`.
    pub fn offset(&self, x: u64, y: u64) -> Result<u64, FbError> {
        self.validate()?;
        if x >= self.width || y >= self.height {
            return Err(FbError::OutOfBounds { x, y });
        }
        let row = y.checked_mul(self.pitch).ok_or(FbError::SizeOverflow)?;
        let column = x
            .checked_mul(
                self.bytes_per_pixel()
                    .ok_or(FbError::UnsupportedBpp(self.format.bpp))?,
            )
            .ok_or(FbError::SizeOverflow)?;
        row.checked_add(column).ok_or(FbError::SizeOverflow)
    }

    /// Number of character cells of `cell_width` x `cell_height` pixels that
    /// fit on screen, used by the boot terminal layout.
    pub fn console_grid(&self, cell_width: u64, cell_height: u64) -> Option<(u64, u64)> {
        if cell_width == 0 || cell_height == 0 {
            return None;
        }
        let cols = self.width / cell_width;
        let rows = self.height / cell_height;
        if cols == 0 || rows == 0 {
            None
        } else {
            Some((cols, rows))
        }
    }
}

/// Split `width` pixels proportionally across `parts`, returning widths that
/// sum to exactly `width` (or `None` when every part is zero or width is zero).
///
/// The computation uses 128-bit intermediates so large byte totals cannot
/// overflow on the way to the bar widths. Any rounding remainder is assigned to
/// the largest part so a segmented bar never leaves a gap.
pub fn proportional_widths<const N: usize>(parts: &[u64; N], width: u64) -> Option<[u64; N]> {
    if width == 0 {
        return None;
    }
    let total: u128 = parts.iter().map(|part| *part as u128).sum();
    if total == 0 {
        return None;
    }
    let mut widths = [0u64; N];
    let mut assigned = 0u64;
    let mut largest = 0usize;
    for (index, part) in parts.iter().enumerate() {
        let scaled = (*part as u128 * width as u128) / total;
        widths[index] = scaled as u64;
        assigned = assigned.saturating_add(widths[index]);
        if *part > parts[largest] {
            largest = index;
        }
    }
    let remainder = width.saturating_sub(assigned);
    widths[largest] = widths[largest].saturating_add(remainder);
    Some(widths)
}

/// Like [`proportional_widths`], but every non-zero part keeps at least
/// `floor` pixels so a sub-pixel category (for example 1 MiB of ACPI data in
/// 512 MiB of RAM) stays visible in a segmented bar.
///
/// The floor is charged to the shared width first and then the remaining pixels
/// are split proportionally, so the result still sums to exactly `width`.
/// `floor` is clamped to `width / nonzero_parts` to keep that guarantee.
pub fn proportional_widths_with_floor<const N: usize>(
    parts: &[u64; N],
    width: u64,
    floor: u64,
) -> Option<[u64; N]> {
    let nonzero = parts.iter().filter(|part| **part > 0).count() as u64;
    if width == 0 || nonzero == 0 {
        return None;
    }
    let floor = floor.min(width / nonzero);
    let reserved = floor * nonzero;
    let mut widths = [0u64; N];
    for (index, part) in parts.iter().enumerate() {
        if *part > 0 {
            widths[index] = floor;
        }
    }
    let remaining = width - reserved;
    if remaining == 0 {
        return Some(widths);
    }
    let proportional = proportional_widths(parts, remaining)?;
    for (index, extra) in proportional.iter().enumerate() {
        widths[index] = widths[index].saturating_add(*extra);
    }
    Some(widths)
}

/// Tenths of a percent that `part` represents of `total`, using 128-bit
/// intermediates so large byte counts cannot overflow.
pub fn percent_tenths(part: u64, total: u64) -> Option<u64> {
    if total == 0 {
        return None;
    }
    Some((part as u128 * 1000 / total as u128) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fb32(width: u64, height: u64, pitch: u64) -> FramebufferGeometry {
        FramebufferGeometry::new(
            width,
            height,
            pitch,
            PixelFormat::new(32, 8, 16, 8, 8, 8, 0),
        )
    }

    #[test]
    fn validates_typical_32bpp_framebuffer() {
        let fb = fb32(1024, 768, 4096);
        assert_eq!(fb.validate(), Ok(()));
        assert_eq!(fb.framebuffer_size(), Ok(4096 * 768));
        assert_eq!(fb.offset(0, 0), Ok(0));
        assert_eq!(fb.offset(1023, 767), Ok(767 * 4096 + 1023 * 4));
    }

    #[test]
    fn rejects_short_pitch_and_zero_dimensions() {
        let fb = fb32(1024, 768, 4095);
        assert_eq!(
            fb.validate(),
            Err(FbError::PitchTooSmall {
                pitch: 4095,
                required: 4096
            })
        );
        assert_eq!(fb32(0, 10, 4096).validate(), Err(FbError::ZeroWidth));
        assert_eq!(fb32(10, 0, 4096).validate(), Err(FbError::ZeroHeight));
    }

    #[test]
    fn rejects_unsupported_depth_and_bad_masks() {
        let mut fmt = PixelFormat::new(8, 3, 0, 3, 3, 2, 6);
        assert_eq!(fmt.validate(), Err(FbError::UnsupportedBpp(8)));
        fmt = PixelFormat::new(32, 9, 0, 8, 8, 8, 16);
        assert_eq!(fmt.validate(), Err(FbError::BadChannelMask));
        fmt = PixelFormat::new(32, 8, 25, 8, 8, 8, 0);
        assert_eq!(fmt.validate(), Err(FbError::BadChannelMask));
    }

    #[test]
    fn detects_size_overflow() {
        let fb = fb32(u64::MAX, 2, u64::MAX);
        assert_eq!(fb.validate(), Err(FbError::SizeOverflow));
    }

    #[test]
    fn bounds_are_checked() {
        let fb = fb32(10, 10, 64);
        assert_eq!(fb.offset(10, 0), Err(FbError::OutOfBounds { x: 10, y: 0 }));
        assert_eq!(fb.offset(0, 10), Err(FbError::OutOfBounds { x: 0, y: 10 }));
    }

    #[test]
    fn packs_32bpp_bgra_and_16bpp_rgb565() {
        let bgra = PixelFormat::new(32, 8, 16, 8, 8, 8, 0);
        assert_eq!(bgra.pack_rgb(0x12, 0x34, 0x56), Some(0x0012_3456));
        let rgb565 = PixelFormat::new(16, 5, 11, 6, 5, 5, 0);
        assert_eq!(rgb565.pack_rgb(255, 255, 255), Some(0xffff));
        assert_eq!(rgb565.pack_rgb(0, 0, 0), Some(0));
        assert_eq!(rgb565.pack_rgb(255, 0, 0), Some(0xf800));
    }

    #[test]
    fn splits_proportional_widths_exactly() {
        assert_eq!(proportional_widths(&[1, 1, 2], 100), Some([25, 25, 50]));
        assert_eq!(proportional_widths(&[0, 0, 0], 100), None);
        assert_eq!(proportional_widths(&[10, 0, 10], 0), None);
        let thirds = proportional_widths(&[1, 1, 1], 10).unwrap();
        assert_eq!(thirds.iter().sum::<u64>(), 10);
        let tiny = proportional_widths(&[1, 1, 1], 2).unwrap();
        assert_eq!(tiny.iter().sum::<u64>(), 2);
    }

    #[test]
    fn floor_keeps_small_segments_visible() {
        let parts = [1_000_000u64, 4];
        let widths = proportional_widths_with_floor(&parts, 400, 3).unwrap();
        assert_eq!(widths.iter().sum::<u64>(), 400);
        assert!(widths[1] >= 3, "tiny slice collapsed: {widths:?}");
        assert!(widths[0] > 380);

        let zeros = proportional_widths_with_floor(&[100u64, 0, 50], 30, 2).unwrap();
        assert_eq!(zeros.iter().sum::<u64>(), 30);
        assert_eq!(zeros[1], 0);
        assert!(zeros[0] >= 2 && zeros[2] >= 2);

        assert_eq!(proportional_widths_with_floor(&[0u64; 3], 30, 2), None);
        assert_eq!(proportional_widths_with_floor(&[1u64], 0, 2), None);
    }

    #[test]
    fn floor_is_clamped_to_the_available_width() {
        let widths = proportional_widths_with_floor(&[1u64, 1], 4, 100).unwrap();
        assert_eq!(widths, [2, 2]);
        let widths = proportional_widths_with_floor(&[1u64, 1, 1], 3, 100).unwrap();
        assert_eq!(widths, [1, 1, 1]);
    }

    #[test]
    fn percent_tenths_reports_share_of_total() {
        assert_eq!(percent_tenths(0, 100), Some(0));
        assert_eq!(percent_tenths(100, 100), Some(1000));
        assert_eq!(percent_tenths(1, 4), Some(250));
        assert_eq!(percent_tenths(0, 0), None);
        let tenths = percent_tenths(u64::MAX, u64::MAX).unwrap();
        assert_eq!(tenths, 1000);
    }

    #[test]
    fn proportional_widths_survive_huge_totals() {
        let widths = proportional_widths(&[u64::MAX, u64::MAX], u64::MAX).unwrap();
        assert_eq!(widths.iter().sum::<u64>(), u64::MAX);
        let widths = proportional_widths(&[u64::MAX, 1], 4096).unwrap();
        assert_eq!(widths.iter().sum::<u64>(), 4096);
    }

    #[test]
    fn computes_console_grid() {
        let fb = fb32(1024, 768, 4096);
        assert_eq!(fb.console_grid(12, 16), Some((85, 48)));
        assert_eq!(fb.console_grid(0, 16), None);
        assert_eq!(fb.console_grid(4096, 4096), None);
    }

    #[test]
    fn pitch_larger_than_visible_line_is_valid() {
        let fb = fb32(100, 10, 512);
        assert_eq!(fb.validate(), Ok(()));
        assert_eq!(fb.offset(99, 9), Ok(9 * 512 + 99 * 4));
    }
}
