use core::arch::asm;

#[inline]
pub fn outb(port: u16, value: u8) {
    // SAFETY: port I/O in ring 0; callers pass valid device ports.
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}

#[inline]
pub fn inb(port: u16) -> u8 {
    let value: u8;
    // SAFETY: port I/O in ring 0; callers pass valid device ports.
    unsafe {
        asm!("in al, dx", out("al") value, in("dx") port, options(nomem, nostack, preserves_flags));
    }
    value
}

#[inline]
pub fn outl(port: u16, value: u32) {
    // SAFETY: port I/O in ring 0; used only for the QEMU test-exit device.
    unsafe {
        asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags));
    }
}

/// Short I/O delay using the legacy port 0x80 (POST diagnostic port). Common
/// on PC hardware and harmless under QEMU/VirtualBox.
pub fn io_wait() {
    outb(0x80, 0);
}
