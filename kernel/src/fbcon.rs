//! Framebuffer log console: mirrors the kernel log to the boot framebuffer.
//!
//! Contract: `docs/FRAMEBUFFER_CONSOLE.md`. The scrolling grid model lives in
//! `os_core::textcon` and the pixel compositor in `os_core::textrender`; this
//! module owns sizing, the framebuffer binding, the cursor overlay, the
//! dirty-row redraw policy and the post-boot readback self-test. Serial output
//! remains the primary channel; the screen is a mirror.

use alloc::vec;
use core::slice;

use os_core::fb::FramebufferGeometry;
use os_core::textcon::{Dirty, TextConsole};
use os_core::textrender;

use crate::sync::IrqSpinLock;

pub const MAX_COLS: u16 = 160;
pub const MAX_ROWS: u16 = 64;
const MAX_CELLS: usize = MAX_COLS as usize * MAX_ROWS as usize;

/// Pinned console colours (docs/FRAMEBUFFER_CONSOLE.md §5).
const BG: (u8, u8, u8) = (0x0b, 0x0f, 0x14);
const FG: (u8, u8, u8) = (0xd0, 0xd7, 0xde);

/// Outcome of the framebuffer readback self-test; the caller maps it to the
/// pinned serial lines (docs/FRAMEBUFFER_CONSOLE.md §7).
pub enum Selftest {
    Skipped,
    Passed {
        compared: u64,
        redraws: u64,
    },
    Failed {
        compared: u64,
        mismatched: u64,
        first_row: u16,
        first_offset: u64,
        expected: u8,
        actual: u8,
    },
}

struct FrameConsole {
    geometry: FramebufferGeometry,
    address: usize,
    buffer_len: usize,
    scale: u64,
    grid: [u8; MAX_CELLS],
    state: TextConsole,
    cursor: (u16, u16),
    redraws: u64,
}

static CONSOLE: IrqSpinLock<Option<FrameConsole>> = IrqSpinLock::new(None);

fn packed_colors(geometry: &FramebufferGeometry) -> textrender::Colors {
    // The geometry was validated before `init`; the fallbacks keep this total
    // without panicking if a future caller hands over an unrejected format.
    let fg = geometry
        .format
        .pack_rgb(FG.0, FG.1, FG.2)
        .unwrap_or(0x00ff_ffff);
    let bg = geometry.format.pack_rgb(BG.0, BG.1, BG.2).unwrap_or(0);
    textrender::Colors { fg, bg }
}

fn mapping_len(geometry: &FramebufferGeometry) -> usize {
    (geometry.pitch * geometry.height) as usize
}

/// Activate the screen console. Returns `(cols, rows, scale)` when enabled;
/// `None` disables the mirror for the rest of the boot (no usable
/// framebuffer). The caller logs the pinned serial line.
pub fn init(framebuffer: Option<(FramebufferGeometry, *mut u8)>) -> Option<(u16, u16, u64)> {
    let (geometry, address) = framebuffer?;
    if geometry.width == 0 || geometry.height == 0 || address.is_null() {
        return None;
    }
    // Sizing: target 80x25 at the largest integer scale, grow while columns
    // exceed the static grid, then clamp (docs/FRAMEBUFFER_CONSOLE.md §3).
    let mut scale = (geometry.width / (80 * 6)).min(geometry.height / (25 * 8));
    scale = scale.max(1);
    while geometry.width / (6 * scale) > MAX_COLS as u64 {
        scale += 1;
    }
    let cols = (geometry.width / (6 * scale)).min(MAX_COLS as u64) as u16;
    let rows = (geometry.height / (8 * scale)).min(MAX_ROWS as u64) as u16;
    if cols == 0 || rows == 0 {
        return None;
    }

    let mut console = FrameConsole {
        geometry,
        address: address as usize,
        buffer_len: mapping_len(&geometry),
        scale,
        grid: [b' '; MAX_CELLS],
        state: TextConsole::new(cols, rows),
        cursor: (0, 0),
        redraws: 0,
    };
    let colors = packed_colors(&geometry);
    {
        let buffer = console.mapping_mut();
        let _ = textrender::fill_rect(
            &geometry,
            buffer,
            textrender::Rect {
                x: 0,
                y: 0,
                width: geometry.width,
                height: geometry.height,
            },
            colors.bg,
        );
    }
    console.redraw_rows(0, rows - 1);
    console.draw_cursor();
    *CONSOLE.lock() = Some(console);
    Some((cols, rows, scale))
}

/// Blocking mirror write, used by the regular log path.
pub fn write_bytes(bytes: &[u8]) {
    let mut guard = CONSOLE.lock();
    if let Some(console) = guard.as_mut() {
        console.ingest(bytes);
    }
}

/// Best-effort mirror write, used by lock-free paths (`force_*`) and the
/// syscall boundary: never spins, so it cannot deadlock a panic or an
/// interrupt handler that already holds the console lock.
pub fn try_write_bytes(bytes: &[u8]) {
    let mut guard = CONSOLE.try_lock();
    let Some(console) = guard.as_mut().and_then(|slot| slot.as_mut()) else {
        return;
    };
    console.ingest(bytes);
}
/// Clear the screen (the shell `clear` builtin mirrors here).
pub fn clear() {
    let mut guard = CONSOLE.lock();
    if let Some(console) = guard.as_mut() {
        console.clear();
    }
}

/// Re-render the grid on the host side and byte-compare it against what the
/// framebuffer actually holds. Runs once, after the heap exists, so skewed
/// mappings or a broken redraw path fail loudly instead of silently.
pub fn selftest() -> Selftest {
    let mut guard = CONSOLE.lock();
    match guard.as_mut() {
        None => Selftest::Skipped,
        Some(console) => console.verify(),
    }
}

impl FrameConsole {
    fn mapping_mut(&mut self) -> &mut [u8] {
        // SAFETY: `address`/`buffer_len` come from the validated Limine
        // framebuffer response and stay valid for the kernel's lifetime; every
        // access happens under the console lock.
        unsafe { slice::from_raw_parts_mut(self.address as *mut u8, self.buffer_len) }
    }

    /// Repaint an inclusive row range and keep the redraw counter honest.
    fn redraw_rows(&mut self, first: u16, last: u16) {
        let (_, cell_h) = textrender::cell_size(self.scale);
        let colors = packed_colors(&self.geometry);
        let geometry = self.geometry;
        let scale = self.scale;
        let Self {
            address,
            buffer_len,
            grid,
            state,
            redraws,
            ..
        } = self;
        let cols = state.cols() as usize;
        // SAFETY: same mapping/bounds contract as `mapping_mut`.
        let buffer = unsafe { slice::from_raw_parts_mut(*address as *mut u8, *buffer_len) };
        for row in first..=last {
            let y = row as u64 * cell_h;
            let cells = &grid[row as usize * cols..(row as usize + 1) * cols];
            let _ = textrender::render_row(&geometry, buffer, 0, y, scale, cells, colors);
        }
        *redraws += 1;
    }

    /// Underline the cursor cell; called after every redraw so it is always
    /// painted over the final cell content.
    fn draw_cursor(&mut self) {
        let (row, col) = self.cursor;
        let scale = self.scale;
        let geometry = self.geometry;
        let colors = packed_colors(&geometry);
        let (cell_w, cell_h) = textrender::cell_size(scale);
        let x = col as u64 * cell_w;
        let y = row as u64 * cell_h + cell_h - scale;
        let buffer = self.mapping_mut();
        let _ = textrender::fill_rect(
            &geometry,
            buffer,
            textrender::Rect {
                x,
                y,
                width: cell_w,
                height: scale,
            },
            colors.fg,
        );
    }

    /// Extend the model's dirty rows with the old and new cursor rows and
    /// repaint (docs/FRAMEBUFFER_CONSOLE.md §5, cursor overlay).
    fn repaint(&mut self, dirty: Option<Dirty>) {
        let new_cursor = self.state.cursor();
        let mut range = dirty.map(|dirty| (dirty.first_row, dirty.last_row));
        for row in [self.cursor.0, new_cursor.0] {
            range = Some(match range {
                Some((first, last)) => (first.min(row), last.max(row)),
                None => (row, row),
            });
        }
        if let Some((first, last)) = range {
            self.redraw_rows(first, last);
        }
        self.cursor = new_cursor;
        self.draw_cursor();
    }

    fn ingest(&mut self, bytes: &[u8]) {
        let state = &mut self.state;
        let dirty = state.write_str(&mut self.grid[..state.cells()], bytes);
        self.repaint(dirty);
    }

    fn clear(&mut self) {
        let rows = self.state.rows();
        let cells = self.state.cells();
        self.state.clear(&mut self.grid[..cells]);
        self.repaint(Some(Dirty {
            first_row: 0,
            last_row: rows - 1,
        }));
    }

    /// Host-side re-render of every grid row into a scratch buffer, compared
    /// byte for byte with the framebuffer. The scratch row uses the same pixel
    /// format and pitch as its framebuffer counterpart, so the comparison is
    /// format-agnostic. `sfence` first: the mapping may be write-combining.
    fn verify(&mut self) -> Selftest {
        let colors = packed_colors(&self.geometry);
        let (cell_w, cell_h) = textrender::cell_size(self.scale);
        let row_bytes =
            self.state.cols() as u64 * cell_w * self.geometry.format.bytes_per_pixel().unwrap_or(4);
        let scratch_len = row_bytes * cell_h;
        let scratch_geometry = FramebufferGeometry::new(
            self.state.cols() as u64 * cell_w,
            cell_h,
            row_bytes,
            self.geometry.format,
        );
        let mut scratch = vec![0u8; scratch_len as usize];
        let (cursor_row, cursor_col) = self.cursor;
        crate::arch::x86_64::sfence();
        // SAFETY: same mapping/bounds contract as `mapping_mut`; this is a
        // read-only view and access stays under the console lock.
        let actual = unsafe { slice::from_raw_parts(self.address as *const u8, self.buffer_len) };
        let mut compared = 0u64;
        let mut mismatched = 0u64;
        let mut first_mismatch: Option<(u16, u64, u8, u8)> = None;
        for row in 0..self.state.rows() {
            scratch.fill(0);
            let cells = self.state.row_slice(&self.grid, row);
            let _ = textrender::render_row(
                &scratch_geometry,
                &mut scratch,
                0,
                0,
                self.scale,
                cells,
                colors,
            );
            if row == cursor_row {
                let x = cursor_col as u64 * cell_w;
                let _ = textrender::fill_rect(
                    &scratch_geometry,
                    &mut scratch,
                    textrender::Rect {
                        x,
                        y: cell_h - self.scale,
                        width: cell_w,
                        height: self.scale,
                    },
                    colors.fg,
                );
            }
            let begin = self.geometry.offset(0, row as u64 * cell_h).unwrap_or(0) as usize;
            for y in 0..cell_h {
                let expected_start = (y * row_bytes) as usize;
                let expected_line = &scratch[expected_start..expected_start + row_bytes as usize];
                let actual_start = begin + (y * self.geometry.pitch) as usize;
                let actual_line = actual
                    .get(actual_start..actual_start + row_bytes as usize)
                    .unwrap_or(&[]);
                for (index, (expected, real)) in expected_line.iter().zip(actual_line).enumerate() {
                    compared += 1;
                    if expected != real {
                        mismatched += 1;
                        if first_mismatch.is_none() {
                            first_mismatch =
                                Some((row, (actual_start + index) as u64, *expected, *real));
                        }
                    }
                }
            }
        }
        if mismatched == 0 {
            Selftest::Passed {
                compared,
                redraws: self.redraws,
            }
        } else {
            let (first_row, first_offset, expected, actual) =
                first_mismatch.unwrap_or((0, 0, 0, 0));
            Selftest::Failed {
                compared,
                mismatched,
                first_row,
                first_offset,
                expected,
                actual,
            }
        }
    }
}
