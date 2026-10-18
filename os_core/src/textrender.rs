//! Glyph and cell composition into a framebuffer byte buffer.
//!
//! This is the host-testable half of the framebuffer console: it writes
//! colours through [`FramebufferGeometry`] (pitch-aware, 16/24/32 bpp) into a
//! caller-provided slice. The kernel passes a slice over the real framebuffer;
//! host tests pass a plain `Vec<u8>`. Renderer rules are pinned in
//! `docs/FRAMEBUFFER_CONSOLE.md` §5.

use crate::fb::{FbError, FramebufferGeometry};
use crate::font;

/// Pre-packed native pixel words for one cell.
#[derive(Clone, Copy)]
pub struct Colors {
    pub fg: u32,
    pub bg: u32,
}

/// Pixel rectangle, inclusive bounds derived from `(x, y)` and the size.
#[derive(Clone, Copy)]
pub struct Rect {
    pub x: u64,
    pub y: u64,
    pub width: u64,
    pub height: u64,
}

/// Pixel size of one character cell at the given scale. The cell is the 5x7
/// glyph plus one column and one row of spacing.
pub fn cell_size(scale: u64) -> (u64, u64) {
    (font::CELL_WIDTH * scale, (font::GLYPH_HEIGHT + 1) * scale)
}

/// Fill a pixel rectangle with `color` (pre-packed native pixel word).
/// Zero-sized rectangles succeed without touching the buffer. On error, no
/// pixel of the rectangle is written.
pub fn fill_rect(
    geometry: &FramebufferGeometry,
    buffer: &mut [u8],
    rect: Rect,
    color: u32,
) -> Result<(), FbError> {
    if rect.width == 0 || rect.height == 0 {
        return Ok(());
    }
    let last_x = rect
        .x
        .checked_add(rect.width)
        .and_then(|edge| edge.checked_sub(1))
        .ok_or(FbError::SizeOverflow)?;
    let last_y = rect
        .y
        .checked_add(rect.height)
        .and_then(|edge| edge.checked_sub(1))
        .ok_or(FbError::SizeOverflow)?;
    if last_x >= geometry.width || last_y >= geometry.height {
        return Err(FbError::OutOfBounds {
            x: rect.x,
            y: rect.y,
        });
    }
    let pixels_end = geometry
        .offset(last_x, last_y)?
        .checked_add(
            geometry
                .bytes_per_pixel()
                .ok_or(FbError::UnsupportedBpp(geometry.format.bpp))?,
        )
        .ok_or(FbError::SizeOverflow)?;
    if pixels_end > buffer.len() as u64 {
        return Err(FbError::OutOfBounds {
            x: last_x,
            y: last_y,
        });
    }
    for row in 0..rect.height {
        for column in 0..rect.width {
            write_pixel(geometry, buffer, rect.x + column, rect.y + row, color)?;
        }
    }
    Ok(())
}

/// Render one character into the cell whose top-left pixel is `(cell_x,
/// cell_y)`.
///
/// If the cell does not fit the geometry or the buffer, `Err` is returned
/// before any pixel of the cell is written; a `ZeroScale` is rejected the
/// same way.
pub fn render_cell(
    geometry: &FramebufferGeometry,
    buffer: &mut [u8],
    cell_x: u64,
    cell_y: u64,
    scale: u64,
    byte: u8,
    colors: Colors,
) -> Result<(), FbError> {
    if scale == 0 {
        return Err(FbError::ZeroScale);
    }
    let (cell_w, cell_h) = cell_size(scale);
    fill_rect(
        geometry,
        buffer,
        Rect {
            x: cell_x,
            y: cell_y,
            width: cell_w,
            height: cell_h,
        },
        colors.bg,
    )?;
    let glyph = font::glyph(byte);
    for (row_index, bits) in glyph.iter().enumerate() {
        for column in 0..font::GLYPH_WIDTH {
            if bits & (1 << (font::GLYPH_WIDTH - 1 - column)) == 0 {
                continue;
            }
            let px = cell_x + column * scale;
            let py = cell_y + row_index as u64 * scale;
            for dy in 0..scale {
                for dx in 0..scale {
                    write_pixel(geometry, buffer, px + dx, py + dy, colors.fg)?;
                }
            }
        }
    }
    Ok(())
}

/// Render a row of cells starting at pixel `(x, y)`. On error the cells
/// rendered before the failing one remain written; the failing cell and every
/// later cell are untouched.
pub fn render_row(
    geometry: &FramebufferGeometry,
    buffer: &mut [u8],
    x: u64,
    y: u64,
    scale: u64,
    cells: &[u8],
    colors: Colors,
) -> Result<usize, FbError> {
    let (cell_w, _) = cell_size(scale);
    let mut rendered = 0;
    for (index, &byte) in cells.iter().enumerate() {
        let cell_x = x
            .checked_add(index as u64 * cell_w)
            .ok_or(FbError::SizeOverflow)?;
        render_cell(geometry, buffer, cell_x, y, scale, byte, colors)?;
        rendered += 1;
    }
    Ok(rendered)
}

fn write_pixel(
    geometry: &FramebufferGeometry,
    buffer: &mut [u8],
    x: u64,
    y: u64,
    color: u32,
) -> Result<(), FbError> {
    let offset = geometry.offset(x, y)? as usize;
    match geometry.format.bpp {
        16 => buffer[offset..offset + 2].copy_from_slice(&(color as u16).to_le_bytes()),
        24 => {
            buffer[offset] = color as u8;
            buffer[offset + 1] = (color >> 8) as u8;
            buffer[offset + 2] = (color >> 16) as u8;
        }
        32 => buffer[offset..offset + 4].copy_from_slice(&color.to_le_bytes()),
        bpp => return Err(FbError::UnsupportedBpp(bpp)),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fb::PixelFormat;

    const FG_RGB: (u8, u8, u8) = (0xd0, 0xd7, 0xde);
    const BG_RGB: (u8, u8, u8) = (0x0b, 0x0f, 0x14);

    fn fmt32() -> PixelFormat {
        PixelFormat::new(32, 8, 16, 8, 8, 8, 0)
    }

    fn colors(geometry: &FramebufferGeometry) -> Colors {
        Colors {
            fg: geometry
                .format
                .pack_rgb(FG_RGB.0, FG_RGB.1, FG_RGB.2)
                .unwrap(),
            bg: geometry
                .format
                .pack_rgb(BG_RGB.0, BG_RGB.1, BG_RGB.2)
                .unwrap(),
        }
    }

    fn pixel32(buffer: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(buffer[offset..offset + 4].try_into().unwrap())
    }

    fn rect(x: u64, y: u64, width: u64, height: u64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn cell_size_is_glyph_plus_spacing() {
        assert_eq!(cell_size(1), (6, 8));
        assert_eq!(cell_size(2), (12, 16));
        assert_eq!(cell_size(3), (18, 24));
    }

    #[test]
    fn renders_exact_glyph_pixels_at_scale_one() {
        let geometry = FramebufferGeometry::new(8, 12, 8 * 4, fmt32());
        let mut buffer = vec![0u8; (8 * 4 * 12) as usize];
        let palette = colors(&geometry);
        render_cell(&geometry, &mut buffer, 0, 0, 1, b'A', palette).unwrap();
        let glyph = crate::font::glyph(b'A');
        for y in 0..8u64 {
            for x in 0..6u64 {
                let expected = if x < 5 && y < 7 && glyph[y as usize] & (1 << (4 - x)) != 0 {
                    palette.fg
                } else {
                    palette.bg
                };
                let offset = geometry.offset(x, y).unwrap() as usize;
                assert_eq!(
                    pixel32(&buffer, offset),
                    expected,
                    "pixel ({x},{y}) of the cell"
                );
            }
        }
        for y in 0..12u64 {
            for x in 6..8u64 {
                let offset = geometry.offset(x, y).unwrap() as usize;
                assert_eq!(
                    pixel32(&buffer, offset),
                    0,
                    "pixel ({x},{y}) outside the cell is untouched"
                );
            }
        }
    }

    #[test]
    fn scale_two_doubles_every_pixel() {
        let geometry = FramebufferGeometry::new(16, 20, 16 * 4, fmt32());
        let mut buffer = vec![0u8; (16 * 4 * 20) as usize];
        let palette = colors(&geometry);
        render_cell(&geometry, &mut buffer, 0, 0, 2, b'A', palette).unwrap();
        let glyph = crate::font::glyph(b'A');
        for y in 0..16u64 {
            for x in 0..12u64 {
                let source_x = x / 2;
                let source_y = y / 2;
                let expected = if source_x < 5
                    && source_y < 7
                    && glyph[source_y as usize] & (1 << (4 - source_x)) != 0
                {
                    palette.fg
                } else {
                    palette.bg
                };
                let offset = geometry.offset(x, y).unwrap() as usize;
                assert_eq!(pixel32(&buffer, offset), expected, "pixel ({x},{y})");
            }
        }
    }

    #[test]
    fn renders_into_24_bpp_with_three_byte_pixels() {
        let format = PixelFormat::new(24, 8, 16, 8, 8, 8, 0);
        let geometry = FramebufferGeometry::new(8, 12, 8 * 3, format);
        let mut buffer = vec![0u8; (8 * 3 * 12) as usize];
        let palette = colors(&geometry);
        render_cell(&geometry, &mut buffer, 0, 0, 1, b'A', palette).unwrap();
        let offset = geometry.offset(0, 0).unwrap() as usize;
        assert_eq!(&buffer[offset..offset + 3], &palette.bg.to_le_bytes()[..3]);
        let offset = geometry.offset(1, 0).unwrap() as usize;
        assert_eq!(&buffer[offset..offset + 3], &palette.fg.to_le_bytes()[..3]);
    }

    #[test]
    fn renders_into_16_bpp_with_the_565_format() {
        let format = PixelFormat::new(16, 5, 11, 6, 5, 5, 0);
        let geometry = FramebufferGeometry::new(8, 12, 8 * 2, format);
        let mut buffer = vec![0u8; (8 * 2 * 12) as usize];
        let palette = colors(&geometry);
        render_cell(&geometry, &mut buffer, 0, 0, 1, b'A', palette).unwrap();
        let offset = geometry.offset(0, 0).unwrap() as usize;
        assert_eq!(buffer[offset], (palette.bg as u16).to_le_bytes()[0]);
        let offset = geometry.offset(1, 0).unwrap() as usize;
        assert_eq!(buffer[offset], (palette.fg as u16).to_le_bytes()[0]);
    }

    #[test]
    fn pitch_padding_bytes_stay_untouched() {
        let geometry = FramebufferGeometry::new(8, 12, 40, fmt32());
        let mut buffer = vec![0xAAu8; (40 * 12) as usize];
        let palette = colors(&geometry);
        render_cell(&geometry, &mut buffer, 0, 0, 1, b'A', palette).unwrap();
        for y in 0..12usize {
            let padding = &buffer[y * 40 + 32..(y + 1) * 40];
            assert!(
                padding.iter().all(|&byte| byte == 0xAA),
                "padding of row {y} is untouched"
            );
        }
    }

    #[test]
    fn a_cell_that_does_not_fit_is_rejected_without_writing() {
        let geometry = FramebufferGeometry::new(8, 8, 32, fmt32());
        let mut buffer = vec![0u8; (32 * 8) as usize];
        let palette = colors(&geometry);
        assert_eq!(
            render_cell(&geometry, &mut buffer, 4, 0, 1, b'A', palette),
            Err(FbError::OutOfBounds { x: 4, y: 0 })
        );
        assert!(buffer.iter().all(|&byte| byte == 0));
    }

    #[test]
    fn a_buffer_that_is_too_small_is_rejected_without_writing() {
        let geometry = FramebufferGeometry::new(8, 8, 32, fmt32());
        let mut buffer = vec![0u8; (32 * 8) as usize - 16];
        let palette = colors(&geometry);
        assert!(render_cell(&geometry, &mut buffer, 0, 0, 1, b'A', palette).is_err());
        assert!(buffer.iter().all(|&byte| byte == 0));
    }

    #[test]
    fn zero_scale_is_rejected() {
        let geometry = FramebufferGeometry::new(8, 8, 32, fmt32());
        let mut buffer = vec![0u8; (32 * 8) as usize];
        let palette = colors(&geometry);
        assert_eq!(
            render_cell(&geometry, &mut buffer, 0, 0, 0, b'A', palette),
            Err(FbError::ZeroScale)
        );
    }

    #[test]
    fn row_rendering_matches_cell_by_cell_rendering() {
        let geometry = FramebufferGeometry::new(16, 12, 16 * 4, fmt32());
        let palette = colors(&geometry);
        let mut row_buffer = vec![0u8; (16 * 4 * 12) as usize];
        let rendered = render_row(&geometry, &mut row_buffer, 0, 0, 1, b"AB", palette).unwrap();
        assert_eq!(rendered, 2);
        let mut cell_buffer = vec![0u8; (16 * 4 * 12) as usize];
        render_cell(&geometry, &mut cell_buffer, 0, 0, 1, b'A', palette).unwrap();
        render_cell(&geometry, &mut cell_buffer, 6, 0, 1, b'B', palette).unwrap();
        assert_eq!(row_buffer, cell_buffer);
    }

    #[test]
    fn empty_rows_render_nothing() {
        let geometry = FramebufferGeometry::new(8, 8, 32, fmt32());
        let mut buffer = vec![0u8; (32 * 8) as usize];
        let palette = colors(&geometry);
        assert_eq!(
            render_row(&geometry, &mut buffer, 0, 0, 1, b"", palette),
            Ok(0)
        );
    }

    #[test]
    fn fill_rect_paints_exactly_the_requested_pixels() {
        let geometry = FramebufferGeometry::new(8, 8, 32, fmt32());
        let mut buffer = vec![0u8; (32 * 8) as usize];
        let palette = colors(&geometry);
        fill_rect(&geometry, &mut buffer, rect(2, 1, 2, 3), palette.fg).unwrap();
        for y in 0..8u64 {
            for x in 0..8u64 {
                let inside = (2..4).contains(&x) && (1..4).contains(&y);
                let offset = geometry.offset(x, y).unwrap() as usize;
                let expected = if inside { palette.fg } else { 0 };
                assert_eq!(pixel32(&buffer, offset), expected, "pixel ({x},{y})");
            }
        }
    }

    #[test]
    fn a_rect_that_does_not_fit_is_rejected_without_writing() {
        let geometry = FramebufferGeometry::new(8, 8, 32, fmt32());
        let mut buffer = vec![0u8; (32 * 8) as usize];
        let palette = colors(&geometry);
        assert_eq!(
            fill_rect(&geometry, &mut buffer, rect(7, 7, 2, 2), palette.fg),
            Err(FbError::OutOfBounds { x: 7, y: 7 })
        );
        assert!(buffer.iter().all(|&byte| byte == 0));
        assert_eq!(
            fill_rect(&geometry, &mut buffer, rect(0, 0, 0, 4), palette.fg),
            Ok(())
        );
    }
}
