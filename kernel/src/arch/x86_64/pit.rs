//! 8254 programmable interval timer, channel 0, periodic mode.
//!
//! This is a bring-up timer for the first interrupt-driven milestone. Stage 8
//! delivers its output through IOAPIC GSI2 (with an 8259 fallback). The APIC
//! timer with TSC calibration is the planned replacement once SMP exists.

use core::sync::atomic::{AtomicU64, Ordering};

use super::io::{io_wait, outb};

const PIT_INPUT_HZ: u32 = 1_193_182;
pub const TARGET_HZ: u32 = 100;

static TICKS: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    let divisor = (PIT_INPUT_HZ / TARGET_HZ).clamp(1, 0xffff) as u16;
    // Channel 0, lobyte/hibyte, mode 2 (rate generator). Mode 2 is the
    // conventional periodic-interrupt mode and is what VirtualBox's 8254
    // emulation delivers to IRQ0 reliably.
    outb(0x43, 0x34);
    io_wait();
    outb(0x40, divisor as u8);
    io_wait();
    outb(0x40, (divisor >> 8) as u8);
}

pub fn handle_interrupt() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn effective_hz() -> u32 {
    PIT_INPUT_HZ / (PIT_INPUT_HZ / TARGET_HZ)
}
