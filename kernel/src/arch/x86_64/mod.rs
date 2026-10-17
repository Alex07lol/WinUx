pub mod apic;
pub mod gdt;
pub mod idt;
pub mod io;
pub mod paging;
pub mod pic;
pub mod pit;

use core::arch::asm;

pub fn interrupts_disable() {
    // SAFETY: clearing IF is always valid in ring 0.
    unsafe { asm!("cli", options(nomem, nostack, preserves_flags)) };
}

pub fn interrupts_enable() {
    // SAFETY: the IDT is installed before this is called.
    unsafe { asm!("sti", options(nomem, nostack, preserves_flags)) };
}

pub fn halt() {
    // SAFETY: `hlt` in ring 0 is valid; it resumes on the next interrupt.
    unsafe { asm!("hlt", options(nomem, nostack, preserves_flags)) };
}

pub fn read_cr2() -> u64 {
    let value: u64;
    // SAFETY: CR2 is readable in ring 0.
    unsafe { asm!("mov {}, cr2", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

pub fn read_tsc() -> u64 {
    let low: u32;
    let high: u32;
    // SAFETY: RDTSC is available on all x86-64 CPUs.
    unsafe {
        asm!("rdtsc", out("eax") low, out("edx") high, options(nomem, nostack, preserves_flags))
    };
    ((high as u64) << 32) | low as u64
}

pub fn disable_local_apic() {
    const IA32_APIC_BASE: u32 = 0x1b;
    // SAFETY: writing the APIC base MSR is legal in ring 0. Limine enables the
    // local APIC (base revision 5); this milestone uses the legacy 8259+PIT
    // path, so the LAPIC is explicitly disabled until the APIC timer and IOAPIC
    // are implemented.
    unsafe {
        let low: u32;
        let high: u32;
        asm!("rdmsr", in("ecx") IA32_APIC_BASE, out("eax") low, out("edx") high, options(nomem, nostack));
        let mut value = ((high as u64) << 32) | low as u64;
        // Leave x2APIC mode first, then clear the global APIC enable bit.
        // Leaving x2APIC set can make the enable-bit write ineffective.
        value &= !(1 << 10);
        value &= !(1 << 11);
        asm!(
            "wrmsr",
            in("ecx") IA32_APIC_BASE,
            in("eax") value as u32,
            in("edx") (value >> 32) as u32,
            options(nomem, nostack)
        );
    }
}
