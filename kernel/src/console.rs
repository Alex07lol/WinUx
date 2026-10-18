//! Serial console input and line editing (Phase 6).
//!
//! The COM1 receive interrupt (IRQ4) appends bytes to a small ring buffer;
//! echo and line editing happen in the reader's thread context, never in the
//! interrupt handler. The reader blocks with `hlt`, so an idle console uses no
//! CPU and wakes on the next timer tick or arriving byte.

use core::sync::atomic::{AtomicU64, Ordering};

use alloc::string::String;

use crate::arch::x86_64::halt;
use crate::serial;
use crate::sync::IrqSpinLock;

pub const RX_CAPACITY: usize = 256;

struct Ring {
    buffer: [u8; RX_CAPACITY],
    head: usize,
    tail: usize,
}

impl Ring {
    const fn new() -> Self {
        Self {
            buffer: [0; RX_CAPACITY],
            head: 0,
            tail: 0,
        }
    }

    fn len(&self) -> usize {
        self.head.wrapping_sub(self.tail)
    }

    /// Push a byte; returns false when the buffer is full.
    fn push(&mut self, byte: u8) -> bool {
        if self.len() == self.buffer.len() {
            return false;
        }
        self.buffer[self.head % self.buffer.len()] = byte;
        self.head = self.head.wrapping_add(1);
        true
    }

    fn pop(&mut self) -> Option<u8> {
        if self.len() == 0 {
            return None;
        }
        let byte = self.buffer[self.tail % self.buffer.len()];
        self.tail = self.tail.wrapping_add(1);
        Some(byte)
    }
}

static RX: IrqSpinLock<Ring> = IrqSpinLock::new(Ring::new());

/// Bytes received while the ring was full.
pub static RX_OVERRUNS: AtomicU64 = AtomicU64::new(0);

/// Configure the UART for receive interrupts. The IRQ4 line must be unmasked
/// at the PIC separately (`pic::unmask_irq(4)`).
pub fn init() {
    // Drop anything stale in the receive register before enabling the
    // interrupt so a phantom byte cannot fire before the shell is ready.
    let mut drained = 0;
    while serial::rx_ready() && drained < 32 {
        let _ = serial::read_byte();
        drained += 1;
    }
    serial::enable_rx_interrupt();
}

/// Called from the IRQ4 handler. Reads every available byte; bounded so a UART
/// that keeps reporting "ready" without data cannot hang the kernel. Hardware
/// FIFO overruns (bytes QEMU delivered faster than the 16-byte FIFO could
/// hold) and a full software ring both count into [`RX_OVERRUNS`].
pub fn on_rx_interrupt() {
    for _ in 0..RX_CAPACITY {
        let status = serial::line_status();
        if status & serial::LINE_DATA_READY == 0 {
            return;
        }
        if status & serial::LINE_OVERRUN != 0 {
            RX_OVERRUNS.fetch_add(1, Ordering::SeqCst);
        }
        let byte = serial::read_byte();
        if !RX.lock().push(byte) {
            RX_OVERRUNS.fetch_add(1, Ordering::SeqCst);
        }
    }
}

pub fn try_read_byte() -> Option<u8> {
    RX.lock().pop()
}

/// Bytes waiting in the software ring.
pub fn ring_len() -> usize {
    RX.lock().len()
}

/// Bytes lost to a UART FIFO overrun or a full software ring.
pub fn overruns() -> u64 {
    RX_OVERRUNS.load(Ordering::SeqCst)
}

pub const MAX_LINE: usize = 128;

/// Read one line, echoing as it is typed. Handles CR/LF, backspace (0x08 and
/// DEL) and printable ASCII; other control bytes are ignored. Never returns
/// without a line, and blocks with `hlt` while no input is pending.
pub fn read_line(line: &mut String) -> usize {
    line.clear();
    loop {
        let byte = loop {
            if let Some(byte) = try_read_byte() {
                break byte;
            }
            // Interrupts are enabled on the shell thread; hlt sleeps until the
            // next timer tick or serial byte.
            halt();
        };
        match byte {
            b'\r' | b'\n' => {
                crate::kprint!("\n");
                return line.len();
            }
            0x08 | 0x7f => {
                if line.pop().is_some() {
                    crate::kprint!("\x08 \x08");
                }
            }
            0x20..=0x7e if line.len() < MAX_LINE => {
                line.push(byte as char);
                crate::kprint!("{}", byte as char);
            }
            _ => {}
        }
    }
}
