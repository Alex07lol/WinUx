//! Legacy 8259A PIC initialisation and line masking. IRQ0 (PIT) and IRQ4
//! (COM1) are normally routed through the IOAPIC in stage 8; the PIC remains
//! the fallback and still carries PCI INTx lines such as the e1000. Device
//! lines are managed through `crate::irq`.

use super::io::{inb, io_wait, outb};

pub fn remap_and_mask_all() {
    // ICW1: begin initialisation, expect ICW4.
    outb(0x20, 0x11);
    io_wait();
    outb(0xa0, 0x11);
    io_wait();
    // ICW2: vector offsets (master 0x20, slave 0x28) — never collide with the
    // CPU exception vectors 0..31.
    outb(0x21, 0x20);
    io_wait();
    outb(0xa1, 0x28);
    io_wait();
    // ICW3: slave on master IRQ2.
    outb(0x21, 0x04);
    io_wait();
    outb(0xa1, 0x02);
    io_wait();
    // ICW4: 8086 mode.
    outb(0x21, 0x01);
    io_wait();
    outb(0xa1, 0x01);
    io_wait();
    // Mask everything; callers unmask what they have handlers for.
    outb(0x21, 0xff);
    outb(0xa1, 0xff);
}

/// Unmask one PIC line (0..=15); callers must have a handler installed for
/// the corresponding vector (32 + line). Slave lines also unmask the master
/// cascade (IRQ2).
pub fn unmask_irq(line: u8) {
    debug_assert!(line < 16);
    if line < 8 {
        let bit = 1u8 << line;
        let mask = inb(0x21);
        outb(0x21, mask & !bit);
    } else {
        let cascade = inb(0x21) & !(1 << 2);
        outb(0x21, cascade);
        let bit = 1u8 << (line - 8);
        let mask = inb(0xA1);
        outb(0xA1, mask & !bit);
    }
}

pub fn unmask_irq0() {
    unmask_irq(0);
}
