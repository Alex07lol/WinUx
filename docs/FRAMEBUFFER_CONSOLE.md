# Framebuffer log console (Phase 6, stage 4) — design spec

Status: **approved 2026-10-18 (scope chosen by the user); implemented
2026-10-18.** This document is the contract for the stage: code, tests and logs
must match it, and any deviation is recorded in §11.

## 1. Goal and non-goals

Goal: mirror the kernel log stream — boot log lines, shell echo and shell
command output — to the Limine linear framebuffer as a scrolling text console
**in addition to** COM1, so the running system is observable on screen without
a serial connection. The model (grid bookkeeping and pixel composition) lives
in host-tested `os_core` modules; the kernel side is a thin, IRQ-safe binding
to the real framebuffer with a boot-time read-back self-test.

Deliberately **not** in this stage:

- no framebuffer input: no PS/2 keyboard, no mouse — console input remains the
  COM1 receive path;
- no ANSI/escape-sequence interpretation, no per-line colours: one foreground
  and one background colour;
- no proportional fonts, no Unicode: the original 5×7 ASCII font, integer
  scale only;
- no scrollback history: lines scrolled off the top are gone (the serial log
  remains the persistent record);
- no graphics driver claims: this is text rendering over the bootloader
  framebuffer, exactly like the dashboard it replaces;
- the boot dashboard (`kernel/src/display.rs`) is **replaced** by the console:
  the information it showed (boot stages, memory map, frames, timer) already
  appears in the log stream, and maintaining two framebuffer UIs doubles the
  geometry and failure surface. The font moves to `os_core`.

## 2. Research and primary sources (R-011)

Recorded fully in `docs/RESEARCH.md` as R-011. Summary:

- **Reading back the framebuffer is valid.** The Limine framebuffer is
  described by its geometry (`width`, `height`, `pitch`, pixel format); under
  QEMU and VirtualBox it is ordinary system memory mapped by the bootloader,
  and reads observe prior writes. If a future mapping is write-combining (WC),
  Intel's SDM (Vol. 3A, §11.3, WC section, quoted in the sources below) states
  partially filled WC buffers are flushed by a serializing event "such as an
  SFENCE or MFENCE instruction". The self-test therefore executes `sfence`
  before reading back, which is correct for both WB and WC mappings.
- **Pixel writes are plain stores** (volatile to prevent elision); a fence is
  only needed for the read-back self-test, not for normal rendering.
- Sources: Intel SDM Vol. 3A §11.3 (WC memory; via the 2007 "Intel 64
  Architecture Memory Ordering" white paper and Intel community quotes of
  §11.3), write-combining implementation guidelines (Intel app note 244422),
  fetched 2026-10-18. Confidence: high for the fence claim; the self-test is
  additionally validated empirically in QEMU and VirtualBox.

## 3. Changes overview

- New `os_core::font` — the project's original 5×7 font table moves from
  `kernel/src/font.rs` (which is deleted) so the renderer is host-testable.
- New `os_core::textcon` — the grid model: byte-stream semantics, wrap,
  scroll, dirty-row reporting.
- New `os_core::textrender` — glyph/cell composition into a byte buffer
  through `FramebufferGeometry` (pitch-aware, 16/24/32 bpp).
- New `kernel/src/fbcon.rs` — the kernel binding: static grid under an
  `IrqSpinLock`, sizing, row redraws, cursor, read-back self-test.
- `kernel/src/serial.rs` gains the tap: every existing output path feeds the
  console (see §7).
- `kernel/src/sync.rs` gains `IrqSpinLock::try_lock` for the panic path.
- `kernel/src/display.rs` and its `main.rs` plumbing (dashboard, `Status`,
  `FramesInfo`, `BootDisplay`) are removed.
- New `scripts/test_console_qemu.sh` + `scripts/test_console_qemu.py` and
  `scripts/check_screendump.py`; `scripts/qemu_serial.py` gains the optional
  monitor and `screendump()` helper (see §11); new `fbcon` step in
  `test_all.sh`.

## 4. Grid model (`os_core::textcon`)

`TextConsole::new(cols, rows)` with `cols`/`rows` in `1..=u16::MAX`; the caller
owns the cell buffer (`&mut [u8]`, length exactly `cols * rows`, `b' '`
initially meaningful as space).

Pinned byte semantics for `write_byte` / `write_str`:

| Byte | Meaning |
|---|---|
| `\n` (0x0a) | cursor to column 0 of the next row (carriage return included) |
| `\r` (0x0d) | cursor to column 0, same row |
| `\t` (0x09) | advance to the next column that is a multiple of 8, filling spaces; may wrap |
| 0x08 | if column > 0: column −= 1 and that cell becomes a space; at column 0 it is a no-op |
| 0x20..=0x7e | store the byte in the cell, advance; at the last column the cursor wraps to column 0 of the next row |
| other | ignored |

Scrolling: when the cursor advances past the last row, rows shift up by one
(memmove of `cols`-sized rows), the last row is filled with spaces, and the
scroll counter increments. `clear()` blanks the cells and homes the cursor
(the scroll counter is kept: it counts total scrolls since init).

Dirty reporting: every mutating operation returns the inclusive row range that
changed (`first..=last`); a scroll reports the full grid. `row_slice(row)`
exposes a row for tests and the renderer.

## 5. Renderer (`os_core::textrender`)

- Cell metrics: `cell_w = 6 * scale`, `cell_h = 8 * scale`; the 5×7 glyph is
  drawn at the cell's top-left, scaled by `scale`; the right and bottom
  one-cell-unit margins stay background. `scale >= 1`.
- `render_cell(geometry, buffer, cell_x, cell_y, scale, byte, fg, bg)`:
  fills the cell rectangle with `bg`, then draws the glyph pixels of
  `font::glyph(byte)` in `fg`. Colours are pre-packed native words
  (`PixelFormat::pack_rgb`); writes respect pitch and bpp exactly like the
  removed dashboard (`u16` for 16 bpp, three bytes for 24, `u32` for 32).
- `render_row(...)` renders a row of cells at a pixel position from a cell
  slice.
- Error policy: any cell whose rectangle does not fit the geometry or exceeds
  the provided buffer returns `Err(FbError)` **before writing any pixel of
  that cell**; the caller decides whether to continue with later cells.
- Unknown bytes render as space via `font::glyph` (font maps anything outside
  `0x20..=0x5f` after uppercase folding to the space glyph).

## 6. Kernel console (`kernel/src/fbcon.rs`)

- `static CONSOLE: IrqSpinLock<Option<FrameConsole>>` with a compile-time grid
  of `MAX_COLS = 160`, `MAX_ROWS = 64` cells (10 KiB).
- Sizing from the validated geometry: `scale = max(1, min(w / (80 * 6),
  h / (25 * 8)))`; while `w / (6 * scale) > MAX_COLS` increase `scale` by one;
  then `cols = min(w / (6 * scale), MAX_COLS)`,
  `rows = min(h / (8 * scale), MAX_ROWS)`. At 1280×800 this yields 106×50 at
  scale 2; at 1024×768, 85×48 at scale 2. Only the grid rectangle is painted;
  any leftover pixels at the right/bottom edge keep their previous content
  (they are outside the contract).
- Colours: background `0x0b0f14`, foreground `0xd0d7de` (packed per format).
- Redraw policy: each write feeds the grid and repaints exactly the dirty rows
  reported by the model (whole rows, cells then glyphs), then paints the
  cursor (an underline: the bottom `scale` pixel rows of the cursor cell
  filled with the foreground colour). The previous cursor cell is repainted
  from the grid when it moves. Scrolling repaints every row.
- Self-test (boot): re-render the entire grid plus cursor into a scratch
  `Vec<u8>` through `os_core::textrender`, `sfence`, then compare against the
  framebuffer bytes read back through the HHDM; equal byte-for-byte is the
  pass condition. This catches dirty-row, scroll and stride errors that host
  tests cannot (host tests use a plain buffer). Requires the kernel heap;
  runs after the heap self-test.
- No framebuffer or failed geometry: the console stays disabled and every
  entry point becomes a no-op.

## 7. Log tap (`kernel/src/serial.rs`)

- `SerialWriter::write_str` (the `kprint!`/`kprintln!` path) feeds
  `fbcon::write_bytes(text)` after the UART write; the console lock is
  IRQ-safe and taken with interrupts already disabled by the serial lock.
- `force_write_str` / `force_write_bytes` (panic, exception diagnostics, shell
  banner, shell prompt, syscall user writes, read-line echo) feed
  `fbcon::try_write_bytes(...)`: a best-effort non-blocking mirror.
- Degradation policy: a panic raised while the console lock is held renders to
  COM1 only; this is deliberate (the panic path must never wait on a lock it
  may already own). Preliminary boot lines printed before framebuffer
  validation are serial-only.
- No recursion: `fbcon` never logs; its failures never unwind.
- The shell's `clear` builtin also clears the grid (`fbcon::clear()`), in
  addition to whatever it does on the serial side.

## 8. Serial lines pinned (evidence contract)

Exactly one of:

```text
fbcon: enabled {cols}x{rows} cells at scale {scale}
fbcon: no framebuffer; screen console disabled
```

Self-test, exactly one of:

```text
fbcon selftest: passed ({bytes} bytes match after {redraws} redraws)
fbcon selftest: skipped (no framebuffer)
```

A self-test mismatch is a `PROJECT_OS_FATAL` (boot failure), like the other
self-tests. The boot-success marker and its `checks=` list are unchanged; the
console is a UI mirror, not a new gate. `redraws` counts row-range repaints
including full repaints.

## 9. Testing plan

Host tests (all in `os_core`, run by `scripts/test_unit.sh`):

- `textcon`: printable writes, wrap at the last column, `\n`/`\r`/`\t`
  semantics, backspace at column 0 and mid-line, scroll contents (top line
  leaves, bottom is blank, scroll counter), dirty ranges (single row, multiple
  rows, full on scroll), `clear`, long-line double wrap.
- `textrender`: exact pixel pattern of a known glyph at scale 1 and 2 for
  16/24/32 bpp geometries, background fill, spacing margins, pitch padding
  untouched, out-of-bounds cell returns `Err` with the buffer unchanged,
  row rendering matches cell-by-cell rendering.
- `font`: table size, bit order (`0b10000` = leftmost pixel), uppercase
  folding, unknown byte → space.

Kernel self-test: as §6; asserted by the QEMU script below and by the
VirtualBox run (which logs the same lines).

`scripts/test_console_qemu.sh` (wired into `test_all.sh` as `fbcon`):

1. Builds `artifacts/project-os-interactive.iso` (the debug ISO exits before
   the shell, and a live screen is needed for the screendump).
2. Boots it with a TCP serial chardev (existing `qemu_serial.py` harness) and
   a TCP QEMU monitor.
3. Asserts on serial: the `fbcon: enabled ...` line, the
   `fbcon selftest: passed ...` line, the boot marker, then types `version`
   and asserts the shell response (proving shell output is mirrored while the
   system runs).
4. Takes `screendump artifacts/console-screendump.ppm` through the monitor,
   then quits QEMU.
5. Runs `scripts/check_screendump.py`: the PPM parses, its dimensions match
   the `framebuffer: WxH ...` serial line, and the grid rectangle derived from
   the `fbcon: enabled CxR cells at scale S` line contains at least 500
   non-background pixels (text is actually visible in the scanout). The
   byte-exact assertion is the kernel self-test; the screendump check proves
   the scanout shows the rendered text and is kept as an artifact.

## 10. Limits (restated honestly)

- Input is serial-only; the on-screen console cannot be typed into.
- No ANSI, no colours per severity, no Unicode, uppercase-folded ASCII only.
- Lines before framebuffer validation and lines while the console lock is held
  by a panicking path are serial-only.
- Read-back requires the framebuffer to be readable memory (true for QEMU and
  VirtualBox linear framebuffers; a WC mapping is handled by the `sfence`).
- One framebuffer consumer (the console); no compositor, no scrolling history.

## 11. Deviations and implementation notes

- **Screendump helper location.** The spec named a separate
  `scripts/qemu_screendump.py`; the implementation instead extends
  `scripts/qemu_serial.py` with an optional QEMU monitor port and a
  `screendump()` method reused by `scripts/test_console_qemu.py`. The session
  setup (TCP chardevs, prompt handling, cleanup) is shared, so a second driver
  would duplicate it.
- **`serial_wait` opt-in.** The console harness starts QEMU with
  `-serial ...,server=on,wait=on` (new `serial_wait=True` parameter) so the
  transcript contains the early `framebuffer:` and `fbcon:` lines; with
  `wait=off` QEMU writes boot bytes before the client connects and loses them.
  Existing shell/persistence sessions keep `wait=off` and their transcripts are
  unchanged (they wait for the later shell banner).
- **Renderer API shape.** Clippy's `too_many_arguments` forced
  `os_core::textrender` to take `Colors { fg, bg }` and `Rect { x, y, width,
  height }` instead of separate colour/geometry arguments; `fill_rect()` is
  public and is also used for the cursor underline and the initial clear.
- **Background fill.** Init fills the whole visible framebuffer with the
  background colour, not only the grid rectangle (§6 said leftover pixels keep
  their previous content). This makes the screendump checker's "outside the
  grid is pure background" assertion meaningful; it does not change the grid
  contract.
- **Self-test byte count.** `{bytes}` is the grid-pixel byte count
  (`cols*cell_w * rows*cell_h * bpp`), e.g. 4,070,400 at 1280×800/scale 2; the
  comparison is per scanline because the framebuffer pitch (5120) exceeds the
  grid row width.
- **Clear semantics.** `fbcon::clear()` repaints every cell in the background
  colour and homes the cursor; the model's scroll counter (kept by
  `textcon::clear`) is not printed, so serial output is unchanged beyond the
  cursor overlay.

