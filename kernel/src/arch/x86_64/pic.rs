//! Legacy 8259A PIC initialisation. The first milestone uses IRQ0 only; the
//! APIC/IOAPIC path is a later milestone (documented in docs/ROADMAP.md).

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

/// Unmask one master-PIC line (0..=7); callers must have a handler installed
/// for the corresponding vector (32 + line).
pub fn unmask_irq(line: u8) {
    debug_assert!(line < 8);
    let bit = 1u8 << (line & 7);
    let mask = inb(0x21);
    outb(0x21, mask & !bit);
}

pub fn unmask_irq0() {
    unmask_irq(0);
}

pub fn eoi_master() {
    outb(0x20, 0x20);
}
