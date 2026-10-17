//! Framebuffer boot dashboard.
//!
//! This is deliberately *not* a graphics driver: it draws through the
//! bootloader-provided linear framebuffer, validating every geometry value
//! through `os_core::fb`. The dashboard is secondary to serial output; if no
//! framebuffer exists the kernel still boots and reports over serial.
//!
//! Layout: at 800x560 and larger the dashboard is a two-column instrument
//! panel — boot stages on the left; memory map, physical-frame allocator gauge
//! and timer status stacked on the right — with a full-width boot banner at the
//! bottom. Panels are sized to their content so a taller display enlarges the
//! timer panel instead of leaving blank panel space. Smaller framebuffers fall
//! back to a compact status list so no information is lost.

use core::fmt::{self, Write};

use os_core::fb::FramebufferGeometry;
use os_core::memmap::MemorySummary;

use crate::font;

const ROW_COUNT: usize = 8;

const BG: u32 = 0x0b0f14;
const PANEL: u32 = 0x11161d;
const PANEL_EDGE: u32 = 0x21262d;
const TITLE: u32 = 0xe6edf3;
const MUTED: u32 = 0x7d8590;
const ACCENT: u32 = 0x58c4dc;
const OK: u32 = 0x56d364;
const WARN: u32 = 0xd29922;
const FAIL: u32 = 0xf85149;
const PENDING: u32 = 0x6e7681;
const KERNEL: u32 = 0x58c4dc;
const RECLAIM: u32 = 0x6e7681;
const ACPI: u32 = 0xd29922;
const FRAMEBUFFER: u32 = 0x9d7cff;
const RESERVED: u32 = 0x30363d;
const OK_BG: u32 = 0x12261a;
const FAIL_BG: u32 = 0x2a1315;

/// Fixed-capacity line buffer for boot messages. Truncation is visible (`~`)
/// rather than silently dropping content.
pub struct FixedLine<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> FixedLine<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    pub fn set(&mut self, args: fmt::Arguments<'_>) {
        self.len = 0;
        let _ = self.write_fmt(args);
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("<invalid>")
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<const N: usize> Write for FixedLine<N> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            if self.len >= N {
                if N > 0 {
                    self.bytes[N - 1] = b'~';
                }
                return Ok(());
            }
            self.bytes[self.len] = byte;
            self.len += 1;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pending,
    Ok,
    Warn,
}

struct Row {
    label: &'static str,
    value: FixedLine<64>,
    status: Status,
}

/// Pixel metrics shared by every panel of one layout pass.
#[derive(Clone, Copy)]
struct Metrics {
    scale: u64,
    cell_h: u64,
}

/// One row of a swatch/key list.
struct LegendLine<'a> {
    color: u32,
    label: &'a str,
    value: &'a str,
}

pub struct BootDisplay {
    device: Option<Device>,
    rows: [Row; ROW_COUNT],
    build_id: &'static str,
    marker: FixedLine<72>,
    failure: FixedLine<80>,
    success: bool,
    memory: Option<MemorySummary>,
    frames: Option<FramesInfo>,
    timer_hz: u32,
    timer_ticks: u64,
}

/// Allocator state rendered by the PHYSICAL FRAMES panel.
#[derive(Clone, Copy)]
pub struct FramesInfo {
    pub free_frames: u64,
    pub total_frames: u64,
    pub largest_free_run: u64,
}

impl BootDisplay {
    pub fn new(
        framebuffer: Option<(FramebufferGeometry, *mut u8)>,
        build_id: &'static str,
    ) -> Self {
        let device = framebuffer.map(|(geometry, address)| Device { geometry, address });
        let rows = [
            Row::new("boot protocol"),
            Row::new("firmware"),
            Row::new("paging"),
            Row::new("framebuffer"),
            Row::new("memory map"),
            Row::new("frame allocator"),
            Row::new("interrupts"),
            Row::new("timer"),
        ];
        let mut display = Self {
            device,
            rows,
            build_id,
            marker: FixedLine::new(),
            failure: FixedLine::new(),
            success: false,
            memory: None,
            frames: None,
            timer_hz: 0,
            timer_ticks: 0,
        };
        display.render();
        display
    }

    pub fn set_row(&mut self, index: usize, value: fmt::Arguments<'_>, status: Status) {
        if let Some(row) = self.rows.get_mut(index) {
            row.value.set(value);
            row.status = status;
        }
    }

    /// Record the memory-map summary for the segmented map panel.
    pub fn set_memory(&mut self, summary: &MemorySummary) {
        self.memory = Some(*summary);
        self.render();
    }

    /// Record physical-frame allocator state for the PHYSICAL FRAMES panel.
    pub fn set_frames(&mut self, info: FramesInfo) {
        self.frames = Some(info);
        self.render();
    }

    /// Record the timer frequency and observed tick count.
    pub fn set_timer(&mut self, hz: u32, ticks: u64) {
        self.timer_hz = hz;
        self.timer_ticks = ticks;
        self.render();
    }

    pub fn success(&mut self) {
        self.success = true;
        self.marker
            .set(format_args!("PROJECT_OS_BOOT_SUCCESS:{}", self.build_id));
        self.render();
    }

    pub fn fail(&mut self, args: fmt::Arguments<'_>) {
        self.failure.set(args);
        self.render();
    }

    pub fn render(&mut self) {
        let Some(device) = &self.device else {
            return;
        };
        if device.geometry.validate().is_err() {
            self.device = None;
            return;
        }
        if device.geometry.width >= 800 && device.geometry.height >= 560 {
            self.render_instrument();
        } else {
            self.render_simple();
        }
    }

    /// Full instrument-panel layout for normal VM and desktop framebuffers.
    fn render_instrument(&mut self) {
        let Some(device) = &self.device else {
            return;
        };
        let width = device.geometry.width;
        let height = device.geometry.height;
        let margin = (width / 40).clamp(24, 64);
        let scale = if width >= 900 { 2 } else { 1 };
        let cell_h = font::GLYPH_HEIGHT * scale;
        let title_scale = (width / 230).clamp(2, 6);

        device.clear(BG);
        device.fill_rect(0, 0, width, 6, ACCENT);

        // Header: wordmark, build id, and a live uptime derived from the PIT.
        // The handoff facts deliberately live only in the BOOT STAGES rail so
        // the header never repeats the panel below it.
        let mut y = margin;
        device.fill_rect(margin, y + 4, 6, font::GLYPH_HEIGHT * title_scale, ACCENT);
        let title_x = margin + 20;
        let project_width = device.text(title_x, y, title_scale, ACCENT, "PROJECT_");
        device.text(title_x + project_width, y, title_scale, TITLE, "OS");

        let build_width = font::text_width(self.build_id, scale);
        if width > margin * 2 + build_width {
            device.text(
                width - margin - build_width,
                y + (font::GLYPH_HEIGHT * title_scale).saturating_sub(cell_h),
                scale,
                MUTED,
                self.build_id,
            );
        }

        y += font::GLYPH_HEIGHT * title_scale + 10;
        device.text(
            margin,
            y,
            scale,
            MUTED,
            "INDEPENDENT X86-64 KERNEL   BOOT REPORT",
        );
        let mut uptime = FixedLine::<64>::new();
        if self.timer_hz == 0 {
            uptime.set(format_args!("AWAITING FIRST INTERRUPT"));
        } else {
            uptime.set(format_args!(
                "UPTIME {} MS AT {} HZ",
                self.timer_ticks.saturating_mul(1000) / self.timer_hz as u64,
                self.timer_hz
            ));
        }
        let uptime_width = font::text_width(uptime.as_str(), scale);
        if width > margin * 2 + uptime_width {
            let color = if self.timer_ticks > 0 { MUTED } else { PENDING };
            device.text(
                width - margin - uptime_width,
                y,
                scale,
                color,
                uptime.as_str(),
            );
        }

        y += cell_h + 14;
        device.fill_rect(margin, y, width.saturating_sub(margin * 2), 2, PANEL_EDGE);
        let content_top = y + 20;

        // Footer and banner reserve space at the bottom.
        let footer_text_y = height.saturating_sub(margin + cell_h + 6);
        let banner_height = cell_h + 20;
        let banner_y = footer_text_y.saturating_sub(banner_height + 12);
        if banner_y < content_top + 160 {
            self.render_simple();
            return;
        }
        let content_bottom = banner_y.saturating_sub(14);

        let usable_width = width.saturating_sub(margin * 2);
        let gutter = 20u64;
        let left_width = usable_width * 52 / 100;
        let right_width = usable_width.saturating_sub(left_width + gutter);
        let right_x = margin + left_width + gutter;

        // Left panel: the boot-stage rail.
        let left_height = content_bottom - content_top;
        device.fill_rect(margin, content_top, left_width, left_height, PANEL);
        device.text(margin + 16, content_top + 12, scale, MUTED, "BOOT STAGES");
        let mut stage_ok = FixedLine::<32>::new();
        let ok_count = self
            .rows
            .iter()
            .filter(|row| row.status == Status::Ok)
            .count();
        stage_ok.set(format_args!("{}/{} OK", ok_count, ROW_COUNT));
        let stage_ok_width = font::text_width(stage_ok.as_str(), scale);
        device.text(
            margin + left_width - 16 - stage_ok_width,
            content_top + 12,
            scale,
            MUTED,
            stage_ok.as_str(),
        );

        let rail_x = margin + 20;
        let stage_text_x = rail_x + 20;
        let tag_right = margin + left_width - 16;
        let stage_top = content_top + 12 + cell_h + 12;
        let stage_space = content_bottom.saturating_sub(stage_top);
        let stage_height = (stage_space / ROW_COUNT as u64).max(cell_h * 2 + 6);
        for (index, row) in self.rows.iter().enumerate() {
            let stage_y = stage_top + index as u64 * stage_height;
            if stage_y + cell_h * 2 > content_bottom {
                break;
            }
            if index + 1 < ROW_COUNT {
                device.fill_rect(rail_x + 2, stage_y + 10, 2, stage_height - 12, PANEL_EDGE);
            }
            device.fill_rect(rail_x, stage_y + 3, 6, 6, status_color(row.status));
            device.text(stage_text_x, stage_y, scale, MUTED, row.label);
            let (tag, tag_color) = status_tag(row.status);
            let tag_width = font::text_width(tag, scale);
            let value_max = tag_right - tag_width - 12;
            device.text_clipped(
                stage_text_x,
                stage_y + cell_h + 2,
                scale,
                TITLE,
                row.value.as_str(),
                value_max,
            );
            device.text(tag_right - tag_width, stage_y + 2, scale, tag_color, tag);
        }

        // Right column: memory map, physical frames, then timer/IRQ status. The
        // panels are sized to their content and the timer panel absorbs the
        // leftover height, so no panel is padded out with dead space.
        let right_height = content_bottom - content_top;
        let gap = 14u64;
        let memory_height = 24 + cell_h + 26 + 6 * (cell_h + 6) + 14;
        let frames_height = 24 + cell_h + 26 + 3 * (cell_h + 6) + 14;
        let mut timer_height = right_height.saturating_sub(memory_height + frames_height + gap * 2);
        let draw_timer = timer_height >= cell_h * 3;
        let memory_height = if draw_timer {
            memory_height
        } else {
            timer_height = 0;
            right_height.saturating_sub(frames_height + gap)
        };
        let frames_y = content_top + memory_height + gap;
        let timer_y = frames_y + frames_height + gap;
        let metrics = Metrics { scale, cell_h };
        self.draw_memory_panel(
            device,
            right_x,
            content_top,
            right_width,
            memory_height,
            metrics,
        );
        self.draw_frames_panel(
            device,
            right_x,
            frames_y,
            right_width,
            frames_height,
            metrics,
        );
        if draw_timer {
            self.draw_timer_panel(device, right_x, timer_y, right_width, timer_height, metrics);
        }

        self.draw_banner(
            device,
            margin,
            banner_y,
            width.saturating_sub(margin * 2),
            banner_height,
            metrics,
        );

        if footer_text_y + cell_h <= height {
            let mut footer = FixedLine::<96>::new();
            footer.set(format_args!(
                "SERIAL COM1 115200 8N1 | BUILD {}",
                self.build_id
            ));
            device.text_clipped(
                margin,
                footer_text_y,
                scale,
                MUTED,
                footer.as_str(),
                width.saturating_sub(margin),
            );
        }
    }

    fn draw_memory_panel(
        &self,
        device: &Device,
        x: u64,
        y: u64,
        width: u64,
        height: u64,
        metrics: Metrics,
    ) {
        let Metrics { scale, cell_h } = metrics;
        device.fill_rect(x, y, width, height, PANEL);
        device.fill_rect(x, y, width, 2, PANEL_EDGE);
        device.text(x + 16, y + 12, scale, MUTED, "MEMORY MAP");
        let Some(summary) = self.memory else {
            device.text_clipped(
                x + 16,
                y + 12 + cell_h + 10,
                scale,
                PENDING,
                "READING BOOT MEMORY MAP",
                x + width - 16,
            );
            return;
        };

        let total = format_mib(summary.total_bytes);
        let total_width = font::text_width(total.as_str(), scale);
        device.text(
            x + width - 16 - total_width,
            y + 12,
            scale,
            MUTED,
            total.as_str(),
        );

        let bar_y = y + 12 + cell_h + 12;
        if bar_y + 16 < y + height {
            draw_memory_bar(device, x + 16, bar_y, width - 32, 16, &summary);
        }

        let known = summary
            .usable_bytes
            .saturating_add(summary.executable_and_modules_bytes)
            .saturating_add(summary.bootloader_reclaimable_bytes)
            .saturating_add(summary.acpi_bytes)
            .saturating_add(summary.framebuffer_bytes);
        let reserved = summary.total_bytes.saturating_sub(known);
        let legend = [
            ("USABLE", summary.usable_bytes, OK),
            (
                "KERNEL + MODULES",
                summary.executable_and_modules_bytes,
                KERNEL,
            ),
            ("BOOTLOADER", summary.bootloader_reclaimable_bytes, RECLAIM),
            ("ACPI", summary.acpi_bytes, ACPI),
            ("FRAMEBUFFER", summary.framebuffer_bytes, FRAMEBUFFER),
            ("RESERVED / OTHER", reserved, RESERVED),
        ];

        let mut legend_y = bar_y + 16 + 10;
        for (label, bytes, color) in legend {
            if legend_y + cell_h + 4 > y + height {
                break;
            }
            let value = format_mib_share(bytes, summary.total_bytes);
            draw_legend_row(
                device,
                x + 16,
                legend_y,
                width - 32,
                metrics,
                LegendLine {
                    color,
                    label,
                    value: value.as_str(),
                },
            );
            legend_y += cell_h + 6;
        }
    }

    /// Physical-frame allocator gauge: one segmented bar plus used/free/largest
    /// figures, so the panel reports what the allocator actually handed out.
    fn draw_frames_panel(
        &self,
        device: &Device,
        x: u64,
        y: u64,
        width: u64,
        height: u64,
        metrics: Metrics,
    ) {
        let Metrics { scale, cell_h } = metrics;
        if height < cell_h * 4 {
            return;
        }
        device.fill_rect(x, y, width, height, PANEL);
        device.fill_rect(x, y, width, 2, PANEL_EDGE);
        device.text(x + 16, y + 12, scale, MUTED, "PHYSICAL FRAMES");
        let Some(info) = self.frames else {
            device.text_clipped(
                x + 16,
                y + 12 + cell_h + 10,
                scale,
                PENDING,
                "ALLOCATOR NOT STARTED",
                x + width - 16,
            );
            return;
        };

        let total = info.total_frames.max(1);
        let used = total.saturating_sub(info.free_frames.min(total));
        let mut total_line = FixedLine::<40>::new();
        total_line.set(format_args!("{} FRAMES", info.total_frames));
        let total_width = font::text_width(total_line.as_str(), scale);
        device.text(
            x + width - 16 - total_width,
            y + 12,
            scale,
            MUTED,
            total_line.as_str(),
        );

        let bar_y = y + 12 + cell_h + 12;
        device.fill_rect(x + 16, bar_y, width - 32, 16, PANEL_EDGE);
        if let Some(parts) =
            os_core::fb::proportional_widths_with_floor(&[used, info.free_frames], width - 32, 4)
        {
            device.fill_rect(x + 16, bar_y, parts[0], 16, WARN);
            device.fill_rect(x + 16 + parts[0], bar_y, parts[1], 16, OK);
        }

        let rows = [
            ("USED", used, WARN),
            ("FREE", info.free_frames, OK),
            ("LARGEST RUN", info.largest_free_run, ACCENT),
        ];
        let mut row_y = bar_y + 16 + 10;
        for (label, frames, color) in rows {
            if row_y + cell_h + 4 > y + height {
                break;
            }
            let value = format_frames_share(frames, info.total_frames);
            draw_legend_row(
                device,
                x + 16,
                row_y,
                width - 32,
                metrics,
                LegendLine {
                    color,
                    label,
                    value: value.as_str(),
                },
            );
            row_y += cell_h + 6;
        }
    }

    fn draw_timer_panel(
        &self,
        device: &Device,
        x: u64,
        y: u64,
        width: u64,
        height: u64,
        metrics: Metrics,
    ) {
        let Metrics { scale, cell_h } = metrics;
        if height < cell_h * 3 {
            return;
        }
        device.fill_rect(x, y, width, height, PANEL);
        device.fill_rect(x, y, width, 2, PANEL_EDGE);
        device.text(x + 16, y + 12, scale, MUTED, "TIMER | IRQ0");

        let mut line_y = y + 12 + cell_h + 10;
        let mut hz = FixedLine::<32>::new();
        if self.timer_hz == 0 {
            hz.set(format_args!("PENDING"));
        } else {
            hz.set(format_args!("{} Hz", self.timer_hz));
        }
        draw_legend_row(
            device,
            x + 16,
            line_y,
            width - 32,
            metrics,
            LegendLine {
                color: ACCENT,
                label: "PIT",
                value: hz.as_str(),
            },
        );
        line_y += cell_h + 6;
        if line_y + cell_h <= y + height {
            let mut ticks = FixedLine::<32>::new();
            ticks.set(format_args!("{}", self.timer_ticks));
            let tick_color = if self.timer_ticks > 0 { OK } else { PENDING };
            draw_legend_row(
                device,
                x + 16,
                line_y,
                width - 32,
                metrics,
                LegendLine {
                    color: tick_color,
                    label: "TICKS",
                    value: ticks.as_str(),
                },
            );
        }
        line_y += cell_h + 6;
        if line_y + cell_h <= y + height {
            let mut period = FixedLine::<32>::new();
            if self.timer_hz == 0 {
                period.set(format_args!("PENDING"));
            } else {
                period.set(format_args!("{} MS PER TICK", 1000 / self.timer_hz as u64));
            }
            draw_legend_row(
                device,
                x + 16,
                line_y,
                width - 32,
                metrics,
                LegendLine {
                    color: ACCENT,
                    label: "PERIOD",
                    value: period.as_str(),
                },
            );
        }
        line_y += cell_h + 6;
        if line_y + cell_h <= y + height {
            let route = literal("8259 -> LINT0 EXTINT");
            draw_legend_row(
                device,
                x + 16,
                line_y,
                width - 32,
                metrics,
                LegendLine {
                    color: ACCENT,
                    label: "ROUTE",
                    value: route.as_str(),
                },
            );
        }

        // Tick strip: one cell per observed tick, up to 16 cells.
        let strip_y = y + height - 16;
        if strip_y > line_y + cell_h + 8 && strip_y < y + height {
            device.text(
                x + 16,
                strip_y - cell_h - 6,
                scale,
                MUTED,
                "TICK STRIP   1 CELL = 1 TICK",
            );
            let cells = 16u64;
            let strip_width = width - 32;
            let cell_width = (strip_width / cells).max(2);
            let filled = self.timer_ticks.min(cells);
            for index in 0..cells {
                let color = if index < filled { OK } else { PANEL_EDGE };
                device.fill_rect(
                    x + 16 + index * cell_width,
                    strip_y,
                    cell_width.saturating_sub(1),
                    8,
                    color,
                );
            }
        }
    }

    fn draw_banner(
        &self,
        device: &Device,
        x: u64,
        y: u64,
        width: u64,
        height: u64,
        metrics: Metrics,
    ) {
        let Metrics { scale, cell_h } = metrics;
        let text_y = y + (height.saturating_sub(cell_h)) / 2;
        if !self.failure.is_empty() {
            device.fill_rect(x, y, width, height, FAIL_BG);
            device.fill_rect(x, y, width, 2, FAIL);
            device.text(x + 16, text_y, scale, FAIL, "BOOT FAILED");
            device.text_clipped(
                x + 16 + font::text_width("BOOT FAILED", scale) + 14,
                text_y,
                scale,
                TITLE,
                self.failure.as_str(),
                x + width - 16,
            );
            return;
        }
        if self.success {
            device.fill_rect(x, y, width, height, OK_BG);
            device.fill_rect(x, y, width, 2, OK);
            device.fill_rect(x + 16, text_y + (cell_h.saturating_sub(10)) / 2, 10, 10, OK);
            device.text(x + 36, text_y, scale, TITLE, self.marker.as_str());
            let passed = literal("ALL CHECKS PASSED");
            let passed_width = font::text_width(passed.as_str(), scale);
            if x + 36 + font::text_width(self.marker.as_str(), scale) + 16
                < x + width - passed_width
            {
                device.text(
                    x + width - 16 - passed_width,
                    text_y,
                    scale,
                    OK,
                    passed.as_str(),
                );
            }
            return;
        }
        device.fill_rect(x, y, width, height, PANEL);
        device.fill_rect(x, y, width, 2, PANEL_EDGE);
        device.text(x + 16, text_y, scale, MUTED, "BOOT IN PROGRESS");
        let ok_count = self
            .rows
            .iter()
            .filter(|row| row.status == Status::Ok)
            .count();
        let mut progress = FixedLine::<32>::new();
        progress.set(format_args!("{}/{} CHECKS", ok_count, ROW_COUNT));
        let progress_width = font::text_width(progress.as_str(), scale);
        device.text(
            x + width - 16 - progress_width,
            text_y,
            scale,
            MUTED,
            progress.as_str(),
        );
    }

    /// Compact single-column layout for small framebuffers.
    fn render_simple(&mut self) {
        let Some(device) = &self.device else {
            return;
        };
        if device.geometry.validate().is_err() {
            self.device = None;
            return;
        }
        let width = device.geometry.width;
        let height = device.geometry.height;
        let margin = (width / 40).clamp(16, 48);
        let row_scale = if width >= 640 { 2 } else { 1 };
        let title_scale = (width / 220).clamp(2, 6);
        let cell_h = font::GLYPH_HEIGHT * row_scale;
        let row_height = cell_h + 12;

        device.clear(BG);
        device.fill_rect(0, 0, width, 6, ACCENT);

        let mut y = margin + 8;
        device.fill_rect(margin, y + 4, 6, font::GLYPH_HEIGHT * title_scale, ACCENT);
        let title_x = margin + 20;
        let project_width = device.text(title_x, y, title_scale, ACCENT, "PROJECT_");
        device.text(title_x + project_width, y, title_scale, TITLE, "OS");

        let build_width = font::text_width(self.build_id, row_scale);
        if build_width + margin * 2 < width {
            device.text(
                width - margin - build_width,
                y + (font::GLYPH_HEIGHT * title_scale - cell_h),
                row_scale,
                MUTED,
                self.build_id,
            );
        }

        y += font::GLYPH_HEIGHT * title_scale + 12;
        device.text(
            title_x,
            y,
            row_scale,
            MUTED,
            "INDEPENDENT X86-64 KERNEL   BOOT REPORT",
        );
        y += cell_h + 16;
        device.fill_rect(margin, y, width.saturating_sub(margin * 2), 2, PANEL_EDGE);
        y += 22;

        device.text(margin, y, row_scale, ACCENT, "BOOT STATUS");
        y += cell_h + 12;

        let panel_top = y.saturating_sub(6);
        let panel_height = (ROW_COUNT as u64) * row_height + 16;
        if panel_top + panel_height < height {
            device.fill_rect(
                margin.saturating_sub(14),
                panel_top,
                width.saturating_sub(margin.saturating_sub(14) * 2),
                panel_height,
                PANEL,
            );
        }

        let label_column = margin + 20 * font::CELL_WIDTH * row_scale;
        for row in &self.rows {
            let (tag, tag_color) = status_tag(row.status);
            device.text(margin + 8, y, row_scale, MUTED, row.label);
            device.text(label_column, y, row_scale, TITLE, row.value.as_str());
            let tag_width = font::text_width(tag, row_scale);
            if width > margin * 2 + tag_width {
                device.text(width - margin - tag_width, y, row_scale, tag_color, tag);
            }
            y += row_height;
        }

        y += 18;
        if !self.failure.is_empty() {
            device.fill_rect(
                margin,
                y,
                width.saturating_sub(margin * 2),
                cell_h + 16,
                FAIL_BG,
            );
            device.text(margin + 8, y + 8, row_scale, FAIL, "BOOT FAILED");
            device.text_clipped(
                label_column,
                y + 8,
                row_scale,
                FAIL,
                self.failure.as_str(),
                width.saturating_sub(margin),
            );
            y += cell_h + 28;
        } else if self.success {
            device.fill_rect(margin, y + 2, 10, 10, OK);
            device.text_clipped(
                margin + 22,
                y,
                row_scale,
                OK,
                self.marker.as_str(),
                width.saturating_sub(margin),
            );
            y += cell_h + 14;
        }

        let footer_y = height.saturating_sub(margin);
        if footer_y > y + cell_h && footer_y < height {
            let mut footer = FixedLine::<96>::new();
            footer.set(format_args!(
                "SERIAL COM1 115200 8N1 | BUILD {}",
                self.build_id
            ));
            device.text_clipped(
                margin,
                footer_y - cell_h - 6,
                row_scale,
                MUTED,
                footer.as_str(),
                width.saturating_sub(margin),
            );
        }
    }
}

impl Row {
    fn new(label: &'static str) -> Self {
        let mut value = FixedLine::new();
        value.set(format_args!("..."));
        Self {
            label,
            value,
            status: Status::Pending,
        }
    }
}

fn status_color(status: Status) -> u32 {
    match status {
        Status::Pending => PENDING,
        Status::Ok => OK,
        Status::Warn => WARN,
    }
}

fn status_tag(status: Status) -> (&'static str, u32) {
    match status {
        Status::Pending => ("[..]", PENDING),
        Status::Ok => ("[OK]", OK),
        Status::Warn => ("[!!]", WARN),
    }
}

fn literal(text: &'static str) -> FixedLine<32> {
    let mut line = FixedLine::new();
    line.set(format_args!("{}", text));
    line
}

/// Format a byte count as MiB with one decimal place without floating point.
fn format_mib(bytes: u64) -> FixedLine<24> {
    let mut line = FixedLine::new();
    let tenths = (bytes as u128 * 10 / (1024 * 1024)) as u64;
    line.set(format_args!("{}.{} MiB", tenths / 10, tenths % 10));
    line
}

/// Byte count as MiB plus its share of `total` in tenths of a percent, so a
/// 1 MiB category in 512 MiB of RAM reads as `1.0 MiB` and `0.2%` instead of
/// looking like a rounding error.
fn format_mib_share(bytes: u64, total: u64) -> FixedLine<32> {
    let mut line = FixedLine::new();
    let mib = format_mib(bytes);
    match os_core::fb::percent_tenths(bytes, total) {
        Some(tenths) => line.set(format_args!(
            "{} {}.{}%",
            mib.as_str(),
            tenths / 10,
            tenths % 10
        )),
        None => line.set(format_args!("{}", mib.as_str())),
    }
    line
}

/// Frame count as MiB plus its share of the managed frame total.
fn format_frames_share(frames: u64, total_frames: u64) -> FixedLine<32> {
    format_mib_share(
        frames.saturating_mul(4096),
        total_frames.saturating_mul(4096),
    )
}

fn draw_legend_row(
    device: &Device,
    x: u64,
    y: u64,
    width: u64,
    metrics: Metrics,
    line: LegendLine<'_>,
) {
    let scale = metrics.scale;
    device.fill_rect(x, y + 2, 8, 8, line.color);
    let value_width = font::text_width(line.value, scale);
    device.text_clipped(
        x + 14,
        y,
        scale,
        MUTED,
        line.label,
        x + width - value_width - 10,
    );
    device.text(x + width - value_width, y, scale, TITLE, line.value);
}

fn draw_memory_bar(
    device: &Device,
    x: u64,
    y: u64,
    width: u64,
    height: u64,
    summary: &MemorySummary,
) {
    let known = summary
        .usable_bytes
        .saturating_add(summary.executable_and_modules_bytes)
        .saturating_add(summary.bootloader_reclaimable_bytes)
        .saturating_add(summary.acpi_bytes)
        .saturating_add(summary.framebuffer_bytes);
    let reserved = summary.total_bytes.saturating_sub(known);
    let parts = [
        summary.usable_bytes,
        summary.executable_and_modules_bytes,
        summary.bootloader_reclaimable_bytes,
        summary.acpi_bytes,
        summary.framebuffer_bytes,
        reserved,
    ];
    device.fill_rect(x, y, width, height, PANEL_EDGE);
    let Some(widths) = os_core::fb::proportional_widths_with_floor(&parts, width, 3) else {
        return;
    };
    let colors = [OK, KERNEL, RECLAIM, ACPI, FRAMEBUFFER, RESERVED];
    let mut cursor = x;
    for (index, part_width) in widths.iter().enumerate() {
        device.fill_rect(cursor, y, *part_width, height, colors[index]);
        cursor += *part_width;
    }
}

struct Device {
    geometry: FramebufferGeometry,
    address: *mut u8,
}

impl Device {
    fn color(&self, rgb: u32) -> u32 {
        self.geometry
            .format
            .pack_rgb(
                ((rgb >> 16) & 0xff) as u8,
                ((rgb >> 8) & 0xff) as u8,
                (rgb & 0xff) as u8,
            )
            .unwrap_or(0)
    }

    fn pixel(&self, x: u64, y: u64, color: u32) {
        let Ok(offset) = self.geometry.offset(x, y) else {
            return;
        };
        let offset = offset as usize;
        // SAFETY: `offset` was bounds-checked against the validated geometry.
        // The framebuffer is mapped by the bootloader for the kernel lifetime
        // and is only written by the boot CPU in this milestone.
        unsafe {
            match self.geometry.format.bpp {
                32 => (self.address.add(offset) as *mut u32).write_volatile(color),
                24 => {
                    for index in 0..3 {
                        *self.address.add(offset + index) = (color >> (index * 8)) as u8;
                    }
                }
                16 => (self.address.add(offset) as *mut u16).write_volatile(color as u16),
                _ => {}
            }
        }
    }

    fn fill_rect(&self, x: u64, y: u64, width: u64, height: u64, color: u32) {
        let color = self.color(color);
        let end_x = x.saturating_add(width).min(self.geometry.width);
        let end_y = y.saturating_add(height).min(self.geometry.height);
        let mut row = y;
        while row < end_y {
            let mut column = x;
            while column < end_x {
                self.pixel(column, row, color);
                column += 1;
            }
            row += 1;
        }
    }

    fn clear(&self, color: u32) {
        self.fill_rect(0, 0, self.geometry.width, self.geometry.height, color);
    }

    /// Draw text and return the advance in pixels (including the trailing gap).
    fn text(&self, x: u64, y: u64, scale: u64, color: u32, text: &str) -> u64 {
        self.text_clipped(x, y, scale, color, text, u64::MAX)
    }

    /// Draw text but never start a glyph past `max_x`. Returns the advance.
    fn text_clipped(&self, x: u64, y: u64, scale: u64, color: u32, text: &str, max_x: u64) -> u64 {
        let color = self.color(color);
        let mut cursor = x;
        for byte in text.bytes() {
            if cursor.saturating_add(font::GLYPH_WIDTH * scale) > max_x {
                break;
            }
            let glyph = font::glyph(byte);
            for (row_index, row) in glyph.iter().enumerate() {
                for column in 0..font::GLYPH_WIDTH {
                    if row & (1 << (font::GLYPH_WIDTH - 1 - column)) == 0 {
                        continue;
                    }
                    let px = cursor + column * scale;
                    let py = y + row_index as u64 * scale;
                    for dy in 0..scale {
                        for dx in 0..scale {
                            self.pixel(px + dx, py + dy, color);
                        }
                    }
                }
            }
            cursor += font::CELL_WIDTH * scale;
        }
        cursor.saturating_sub(x)
    }
}
