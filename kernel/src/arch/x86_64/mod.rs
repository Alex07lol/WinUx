pub mod apic;
pub mod context;
pub mod gdt;
pub mod idt;
pub mod io;
pub mod ioapic;
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

/// Disable interrupts and return the previous RFLAGS value for
/// [`interrupts_restore`]. Used by IRQ-safe locks so a lock taken with
/// interrupts enabled cannot be interrupted while held (which could otherwise
/// deadlock on a single CPU, since interrupt handlers never block).
pub fn interrupts_save_disable() -> u64 {
    let flags: u64;
    // SAFETY: `pushfq`/`pop`/`cli` are valid in ring 0. The stack is balanced
    // by the end of the block.
    unsafe { asm!("pushfq", "pop {}", "cli", out(reg) flags) };
    flags
}

/// Restore RFLAGS saved by [`interrupts_save_disable`]. Only the interrupted
/// context's flags should be passed back.
pub fn interrupts_restore(flags: u64) {
    // SAFETY: restoring previously saved RFLAGS in ring 0 is valid; the caller
    // passes flags saved by `interrupts_save_disable` on this CPU.
    unsafe { asm!("push {}", "popfq", in(reg) flags) };
}

pub fn halt() {
    // SAFETY: `hlt` in ring 0 is valid; it resumes on the next interrupt.
    unsafe { asm!("hlt", options(nomem, nostack, preserves_flags)) };
}

/// Enable the x86-64 SSE/SSE2 baseline for ring 3 (Windows x64 requires it).
/// The MSVC-targeted fixtures save `xmm6` in their prologues, which faults
/// with `#UD` while `CR0.EM`/`CR4.OSFXSR` keep SIMD disabled. The kernel
/// itself is built for `x86_64-unknown-none` without SSE, so it does not
/// clobber the registers between user entries.
pub fn enable_sse() {
    // SAFETY: CR0/CR4 are writable in ring 0. Clearing EM/TS and setting
    // OSFXSR/OSXMMEXCPT is the described sequence for enabling SSE.
    unsafe {
        let mut cr0: u64;
        asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
        cr0 &= !((1 << 2) | (1 << 3));
        cr0 |= 1 << 1;
        asm!("mov cr0, {}", in(reg) cr0, options(nomem, nostack, preserves_flags));
        let mut cr4: u64;
        asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
        cr4 |= (1 << 9) | (1 << 10);
        asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack, preserves_flags));
    }
}

/// Serialise prior stores. Used before reading back framebuffer bytes that
/// this CPU wrote through a possibly write-combining mapping: Intel SDM
/// Vol 3A states write-combining buffers are flushed by a serialising
/// instruction such as `sfence` (docs/RESEARCH.md R-011).
pub fn sfence() {
    // SAFETY: `sfence` is valid in ring 0 and has no memory operands.
    unsafe { asm!("sfence", options(nomem, nostack, preserves_flags)) };
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
