//! Legacy 8259A IRQ dispatch and ownership (Phase 9 stage 6).
//!
//! The timer (IRQ0) and COM1 (IRQ4) keep their dedicated paths in `idt.rs`.
//! This module dispatches the remaining legacy lines to registered handlers,
//! sends the 8259 end-of-interrupt sequence (slave first for lines 8..15),
//! and masks/unmasks individual lines. Registration is exclusive and
//! allocation-free; the driver resource ledger claims the same line so a
//! second device cannot silently share it. See `docs/DRIVER_MODEL.md` and
//! ADR-0032/R-025.

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::arch::x86_64::io::{inb, outb};

pub const MAX_IRQ: u8 = 16;
const TIMER_LINE: u8 = 0;
const CONSOLE_LINE: u8 = 4;

type Handler = fn();

static HANDLERS: [AtomicUsize; 16] = [const { AtomicUsize::new(0) }; 16];

/// True when `line` is a legacy PIC line this module can route. Timer and
/// COM1 are reserved and never handed to a device driver.
pub fn usable(line: u8) -> bool {
    line < MAX_IRQ && line != TIMER_LINE && line != CONSOLE_LINE
}

/// Register the single handler for `line`. Fails for a reserved line or when
/// another handler is already installed.
pub fn register(line: u8, handler: Handler) -> Result<(), &'static str> {
    if !usable(line) {
        return Err("IRQ line is not assignable");
    }
    HANDLERS[line as usize]
        .compare_exchange(0, handler as usize, Ordering::SeqCst, Ordering::SeqCst)
        .map(|_| ())
        .map_err(|_| "IRQ line already has a handler")
}

/// Remove `line`'s handler if it is still installed.
pub fn unregister(line: u8) -> bool {
    if line >= MAX_IRQ {
        return false;
    }
    HANDLERS[line as usize].swap(0, Ordering::SeqCst) != 0
}

/// Dispatch one hardware IRQ: run the handler, then acknowledge the 8259.
pub fn dispatch(line: u8) {
    if line >= MAX_IRQ {
        return;
    }
    let handler = HANDLERS[line as usize].load(Ordering::Relaxed);
    if handler != 0 {
        // SAFETY: `register` stores only the address of a `fn()`; a handler
        // cannot be unregistered while this interrupt frame is running on the
        // single CPU (unregister is called with the line masked).
        let handler: Handler = unsafe { core::mem::transmute(handler) };
        handler();
    }
    eoi(line);
}

/// Non-specific end-of-interrupt for `line`; a slave line acknowledges the
/// slave first, then the master cascade.
pub fn eoi(line: u8) {
    if line >= 8 {
        outb(0xA0, 0x20);
    }
    outb(0x20, 0x20);
}

/// Mask one line at the 8259.
pub fn mask(line: u8) {
    if line >= MAX_IRQ {
        return;
    }
    if line < 8 {
        let mask = inb(0x21) | (1 << line);
        outb(0x21, mask);
    } else {
        let mask = inb(0xA1) | (1 << (line - 8));
        outb(0xA1, mask);
    }
}

/// Unmask one line, including the master cascade for slave lines.
pub fn unmask(line: u8) {
    if line >= MAX_IRQ {
        return;
    }
    if line >= 8 {
        let master = inb(0x21) & !(1 << 2);
        outb(0x21, master);
        let slave = inb(0xA1) & !(1 << (line - 8));
        outb(0xA1, slave);
    } else {
        let mask = inb(0x21) & !(1 << line);
        outb(0x21, mask);
    }
}
