//! 16550-compatible COM1 serial output.
//!
//! Serial is the mandatory debug channel: every boot result is written here so
//! automated QEMU/VirtualBox tests can assert on it without a display. The
//! normal path uses a spin lock; panic/exception paths bypass the lock so a
//! fault while holding it cannot deadlock the kernel before it reports.
//!
//! Every line is also mirrored to the framebuffer log console when one is
//! active (docs/FRAMEBUFFER_CONSOLE.md): the normal path mirrors blocking, the
//! panic/exception (`force_*`) paths mirror best-effort so they can never
//! deadlock on the console lock.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::x86_64::io::{inb, outb};

const COM1: u16 = 0x3f8;
const INTERRUPT_ENABLE: u16 = COM1 + 1;
const LINE_STATUS: u16 = COM1 + 5;
const TRANSMIT_EMPTY: u8 = 0x20;

/// LSR bit 0: received data is ready in the FIFO.
pub const LINE_DATA_READY: u8 = 0x01;
/// LSR bit 1: at least one received byte was dropped (FIFO overrun).
pub const LINE_OVERRUN: u8 = 0x02;

/// Read the line status register. Reading it clears the error bits (overrun,
/// parity, framing, break), so callers that track overruns must inspect the
/// value this call returns.
pub fn line_status() -> u8 {
    inb(LINE_STATUS)
}

static LOCK: AtomicBool = AtomicBool::new(false);

fn wait_for_transmit() {
    // Bounded spin: a missing UART must not hang boot forever.
    for _ in 0..1_000_000 {
        if inb(LINE_STATUS) & TRANSMIT_EMPTY != 0 {
            return;
        }
        core::hint::spin_loop();
    }
}

fn write_byte(byte: u8) {
    wait_for_transmit();
    outb(COM1, byte);
}

pub fn init() {
    // Disable interrupts, set DLAB, 115200 baud (divisor 1), 8N1, enable and
    // clear FIFOs, assert DTR/RTS. These are the standard 16550 initialisation
    // steps from the PC UART programming model.
    outb(COM1 + 1, 0x00);
    outb(COM1 + 3, 0x80);
    outb(COM1, 0x01);
    outb(COM1 + 1, 0x00);
    outb(COM1 + 3, 0x03);
    // FCR: enable FIFOs, clear both, RX trigger level 1 byte. A 14-byte
    // trigger (0xc7) strands short bursts below the threshold: the IRQ only
    // fires once 14 bytes accumulate, and anything past the 16-byte FIFO is
    // dropped. Level 1 raises IRQ4 for every byte so the handler can drain
    // immediately; observed as dropped characters when typing over a pipe.
    outb(COM1 + 2, 0x07);
    outb(COM1 + 4, 0x0b);
}

/// True when a received byte is waiting in the UART.
pub fn rx_ready() -> bool {
    line_status() & LINE_DATA_READY != 0
}

/// Read one received byte (call only when [`rx_ready`] is true).
pub fn read_byte() -> u8 {
    inb(COM1)
}

/// Route "data available" to IRQ4. `init()` disabled all UART interrupts, and
/// the PIC must still unmask IRQ4 before any interrupt is delivered.
pub fn enable_rx_interrupt() {
    outb(INTERRUPT_ENABLE, 0x01);
}

/// Lock-free write used by panic and exception diagnostics.
pub fn force_write_str(text: &str) {
    for byte in text.bytes() {
        if byte == b'\n' {
            write_byte(b'\r');
        }
        write_byte(byte);
    }
    crate::fbcon::try_write_bytes(text.as_bytes());
}

/// Lock-free raw-byte write used by the syscall boundary (the kernel validates
/// the user buffer before calling this).
pub fn force_write_bytes(bytes: &[u8]) {
    for &byte in bytes {
        if byte == b'\n' {
            write_byte(b'\r');
        }
        write_byte(byte);
    }
    crate::fbcon::try_write_bytes(bytes);
}

pub fn force_print(args: fmt::Arguments<'_>) {
    struct ForceWriter;
    impl Write for ForceWriter {
        fn write_str(&mut self, text: &str) -> fmt::Result {
            force_write_str(text);
            Ok(())
        }
    }
    let _ = ForceWriter.write_fmt(args);
}

struct SerialWriter;

impl Write for SerialWriter {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            if byte == b'\n' {
                write_byte(b'\r');
            }
            write_byte(byte);
        }
        crate::fbcon::write_bytes(text.as_bytes());
        Ok(())
    }
}

pub fn _print(args: fmt::Arguments<'_>) {
    // Interrupts are disabled while the lock is held so a thread preempted
    // mid-write cannot leave another thread spinning on a lock that only the
    // preempted thread can release. `force_*` callers stay lock-free.
    let flags = crate::arch::x86_64::interrupts_save_disable();
    while LOCK
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    let _ = SerialWriter.write_fmt(args);
    LOCK.store(false, Ordering::Release);
    crate::arch::x86_64::interrupts_restore(flags);
}

#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => {
        $crate::serial::_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! kprintln {
    () => {
        $crate::serial::_print(format_args!("\n"))
    };
    ($($arg:tt)*) => {
        $crate::serial::_print(format_args!("{}\n", format_args!($($arg)*)))
    };
}
