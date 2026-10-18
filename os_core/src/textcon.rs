//! Scrolling text-grid model for the framebuffer console.
//!
//! The model is deliberately free of pointers and hardware: it bookkeeps a
//! caller-provided cell buffer (`cols * rows` bytes, one byte per character)
//! and reports which rows changed, so the kernel can repaint exactly those
//! rows. Byte semantics are pinned in `docs/FRAMEBUFFER_CONSOLE.md` §4.

/// Inclusive range of rows touched by one operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dirty {
    pub first_row: u16,
    pub last_row: u16,
}

impl Dirty {
    fn single(row: u16) -> Self {
        Self {
            first_row: row,
            last_row: row,
        }
    }

    fn extend(&mut self, row: u16) {
        self.first_row = self.first_row.min(row);
        self.last_row = self.last_row.max(row);
    }
}

pub struct TextConsole {
    cols: u16,
    rows: u16,
    row: u16,
    col: u16,
    scrolls: u32,
}

impl TextConsole {
    /// `cols` and `rows` must be non-zero; the caller clamps them to whatever
    /// the framebuffer can hold.
    pub fn new(cols: u16, rows: u16) -> Self {
        assert!(cols > 0 && rows > 0, "console grid must be non-empty");
        Self {
            cols,
            rows,
            row: 0,
            col: 0,
            scrolls: 0,
        }
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    /// Cursor position as `(row, col)`; the cursor cell is the one the next
    /// character will occupy.
    pub fn cursor(&self) -> (u16, u16) {
        (self.row, self.col)
    }

    /// Number of times content scrolled off the top since construction.
    pub fn scrolls(&self) -> u32 {
        self.scrolls
    }

    pub fn cells(&self) -> usize {
        self.cols as usize * self.rows as usize
    }

    pub fn row_slice<'a>(&self, cells: &'a [u8], row: u16) -> &'a [u8] {
        let start = row as usize * self.cols as usize;
        &cells[start..start + self.cols as usize]
    }

    /// Blank the grid and home the cursor. The scroll counter is kept.
    pub fn clear(&mut self, cells: &mut [u8]) {
        let count = self.cells().min(cells.len());
        for cell in &mut cells[..count] {
            *cell = b' ';
        }
        self.row = 0;
        self.col = 0;
    }

    pub fn write_str(&mut self, cells: &mut [u8], text: &[u8]) -> Option<Dirty> {
        debug_assert!(cells.len() >= self.cells());
        let mut dirty: Option<Dirty> = None;
        for &byte in text {
            self.write_byte(cells, byte, &mut dirty);
        }
        dirty
    }

    /// Feed one byte; `dirty` accumulates the inclusive rows whose cells
    /// changed (`None` means nothing was written).
    pub fn write_byte(&mut self, cells: &mut [u8], byte: u8, dirty: &mut Option<Dirty>) {
        match byte {
            b'\n' => self.newline(cells, dirty),
            b'\r' => self.col = 0,
            b'\t' => {
                let mut wrote = false;
                loop {
                    if wrote && self.col.is_multiple_of(8) {
                        break;
                    }
                    self.put(cells, b' ', dirty);
                    wrote = true;
                }
            }
            0x08 => {
                if self.col > 0 {
                    self.col -= 1;
                    let (row, col) = (self.row, self.col);
                    self.store(cells, row, col, b' ', dirty);
                }
            }
            0x20..=0x7e => self.put(cells, byte, dirty),
            _ => {}
        }
    }

    fn put(&mut self, cells: &mut [u8], byte: u8, dirty: &mut Option<Dirty>) {
        let (row, col) = (self.row, self.col);
        self.store(cells, row, col, byte, dirty);
        self.col += 1;
        if self.col == self.cols {
            self.col = 0;
            self.newline(cells, dirty);
        }
    }

    fn newline(&mut self, cells: &mut [u8], dirty: &mut Option<Dirty>) {
        self.col = 0;
        if self.row + 1 < self.rows {
            self.row += 1;
        } else {
            self.scroll(cells, dirty);
        }
    }

    fn scroll(&mut self, cells: &mut [u8], dirty: &mut Option<Dirty>) {
        let cols = self.cols as usize;
        let rows = self.rows as usize;
        cells.copy_within(cols..cols * rows, 0);
        for cell in &mut cells[cols * (rows - 1)..cols * rows] {
            *cell = b' ';
        }
        self.row = self.rows - 1;
        self.scrolls = self.scrolls.wrapping_add(1);
        *dirty = Some(Dirty {
            first_row: 0,
            last_row: self.rows - 1,
        });
    }

    fn store(&mut self, cells: &mut [u8], row: u16, col: u16, byte: u8, dirty: &mut Option<Dirty>) {
        let index = row as usize * self.cols as usize + col as usize;
        cells[index] = byte;
        match dirty {
            Some(range) => range.extend(row),
            None => *dirty = Some(Dirty::single(row)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn console(cols: u16, rows: u16) -> (TextConsole, Vec<u8>) {
        (
            TextConsole::new(cols, rows),
            vec![b' '; cols as usize * rows as usize],
        )
    }

    fn row<'a>(console: &TextConsole, cells: &'a [u8], index: u16) -> &'a [u8] {
        console.row_slice(cells, index)
    }

    #[test]
    fn printable_bytes_wrap_at_the_last_column() {
        let (mut con, mut cells) = console(4, 2);
        let dirty = con.write_str(&mut cells, b"abcdef");
        assert_eq!(
            dirty,
            Some(Dirty {
                first_row: 0,
                last_row: 1
            })
        );
        assert_eq!(row(&con, &cells, 0), b"abcd");
        assert_eq!(row(&con, &cells, 1), b"ef  ");
        assert_eq!(con.cursor(), (1, 2));
        assert_eq!(con.scrolls(), 0);
    }

    #[test]
    fn carriage_return_homes_the_column() {
        let (mut con, mut cells) = console(4, 2);
        con.write_str(&mut cells, b"ab\ryz");
        assert_eq!(row(&con, &cells, 0), b"yz  ");
        assert_eq!(con.cursor(), (0, 2));
    }

    #[test]
    fn newline_is_carriage_return_plus_advance() {
        let (mut con, mut cells) = console(4, 2);
        let dirty = con.write_str(&mut cells, b"ab\ncd");
        assert_eq!(
            dirty,
            Some(Dirty {
                first_row: 0,
                last_row: 1
            })
        );
        assert_eq!(row(&con, &cells, 0), b"ab  ");
        assert_eq!(row(&con, &cells, 1), b"cd  ");
        assert_eq!(con.cursor(), (1, 2));
    }

    #[test]
    fn bare_newline_alone_reports_no_cell_change() {
        let (mut con, mut cells) = console(4, 2);
        let dirty = con.write_str(&mut cells, b"\n\r");
        assert_eq!(dirty, None);
        assert_eq!(con.cursor(), (1, 0));
    }

    #[test]
    fn backspace_clears_the_previous_cell() {
        let (mut con, mut cells) = console(4, 2);
        con.write_str(&mut cells, b"ab\x08");
        assert_eq!(row(&con, &cells, 0), b"a   ");
        assert_eq!(con.cursor(), (0, 1));
        con.write_str(&mut cells, b"\x08");
        assert_eq!(row(&con, &cells, 0), b"    ");
        assert_eq!(con.cursor(), (0, 0));
        let dirty = con.write_str(&mut cells, b"\x08");
        assert_eq!(dirty, None, "backspace at column 0 is a no-op");
        assert_eq!(con.cursor(), (0, 0));
    }

    #[test]
    fn tab_advances_to_the_next_stop_filling_spaces() {
        let (mut con, mut cells) = console(16, 2);
        con.write_str(&mut cells, b"a\tb");
        assert_eq!(con.cursor(), (0, 9));
        assert_eq!(&row(&con, &cells, 0)[..9], b"a       b");
    }

    #[test]
    fn tab_crosses_a_row_when_the_stop_is_past_the_edge() {
        let (mut con, mut cells) = console(6, 2);
        con.write_str(&mut cells, b"ab\tc");
        assert_eq!(row(&con, &cells, 0), b"ab    ");
        assert_eq!(row(&con, &cells, 1), b"c     ");
        assert_eq!(con.cursor(), (1, 1));
    }

    #[test]
    fn scrolling_shifts_rows_up_and_clears_the_last() {
        let (mut con, mut cells) = console(4, 2);
        con.write_str(&mut cells, b"abcd");
        let dirty = con.write_str(&mut cells, b"efgh");
        assert_eq!(
            dirty,
            Some(Dirty {
                first_row: 0,
                last_row: 1
            }),
            "the wrap into a new row and the scroll touch both rows"
        );
        assert_eq!(row(&con, &cells, 0), b"efgh");
        assert_eq!(row(&con, &cells, 1), b"    ");
        assert_eq!(con.cursor(), (1, 0));
        assert_eq!(con.scrolls(), 1);
    }

    #[test]
    fn scrolling_via_newline_clears_the_last_row() {
        let (mut con, mut cells) = console(4, 2);
        con.write_str(&mut cells, b"one\n");
        con.write_str(&mut cells, b"\ntail");
        assert_eq!(
            con.scrolls(),
            2,
            "the full row 'tail' wraps and scrolls once more"
        );
        assert_eq!(row(&con, &cells, 0), b"tail");
        assert_eq!(row(&con, &cells, 1), b"    ");
        assert_eq!(con.cursor(), (1, 0));
    }

    #[test]
    fn long_lines_wrap_repeatedly_and_scroll_once_per_overflow() {
        let (mut con, mut cells) = console(4, 2);
        con.write_str(&mut cells, b"0123456789");
        assert_eq!(con.scrolls(), 1);
        assert_eq!(row(&con, &cells, 0), b"4567");
        assert_eq!(row(&con, &cells, 1), b"89  ");
    }

    #[test]
    fn ignored_bytes_change_nothing() {
        let (mut con, mut cells) = console(4, 2);
        let dirty = con.write_str(&mut cells, &[0x00, 0x07, 0x7f, 0xff, b'\x1b']);
        assert_eq!(dirty, None);
        assert_eq!(con.cursor(), (0, 0));
    }

    #[test]
    fn clear_blanks_the_grid_and_homes_the_cursor() {
        let (mut con, mut cells) = console(4, 2);
        con.write_str(&mut cells, b"abcd");
        assert_eq!(con.cursor(), (1, 0));
        con.clear(&mut cells);
        assert_eq!(con.cursor(), (0, 0));
        assert!(cells.iter().all(|&cell| cell == b' '));
        assert_eq!(con.scrolls(), 0);
    }

    #[test]
    fn dirty_range_spans_all_touched_rows() {
        let (mut con, mut cells) = console(4, 2);
        let dirty = con.write_str(&mut cells, b"a\nb\nc");
        assert_eq!(
            dirty,
            Some(Dirty {
                first_row: 0,
                last_row: 1
            })
        );
    }

    #[test]
    fn dirty_range_is_a_single_row_for_one_line() {
        let (mut con, mut cells) = console(8, 3);
        let dirty = con.write_str(&mut cells, b"hello");
        assert_eq!(
            dirty,
            Some(Dirty {
                first_row: 0,
                last_row: 0
            })
        );
    }
}
