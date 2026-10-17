//! Local-APIC bring-up for the legacy 8259 path.
//!
//! A 64-bit VirtualBox guest always has an APIC, and the 8259's INT output is
//! delivered through LAPIC LINT0. Firmware/Limine may leave LINT0 masked, which
//! leaves PIC IRQs pending forever even though IF=1. The kernel therefore:
//!
//! 1. maps the APIC MMIO page (see `paging::map_mmio_2m`), and
//! 2. puts LINT0 into ExtINT delivery and unmasks it.
//!
//! Only LINT0 is unmasked; the APIC timer, IOAPIC and SMP startup belong to the
//! later APIC milestone.

use core::arch::asm;
use core::ptr::{read_volatile, write_volatile};

const IA32_APIC_BASE: u32 = 0x1b;
const APIC_BASE_MASK: u64 = 0x000f_ffff_ffff_f000;
const APIC_ENABLE: u64 = 1 << 11;
const X2APIC_ENABLE: u64 = 1 << 10;

const APIC_BASE_PHYSICAL: u64 = 0xfee0_0000;

const SVR: usize = 0x0f0;
const TPR: usize = 0x080;
const LVT_TIMER: usize = 0x320;
const LVT_THERMAL: usize = 0x330;
const LVT_PERF: usize = 0x340;
const LVT_LINT0: usize = 0x350;
const LVT_LINT1: usize = 0x360;
const LVT_ERROR: usize = 0x370;

const APIC_ENABLE_BIT: u32 = 1 << 8;
const LVT_MASKED: u32 = 1 << 16;
const LVT_EXTINT: u32 = 0b111 << 8;

fn read_msr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    // SAFETY: RDMSR is ring-0 only; the caller is the kernel.
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") low,
            out("edx") high,
            options(nomem, nostack)
        )
    };
    ((high as u64) << 32) | low as u64
}

fn write_msr(msr: u32, value: u64) {
    // SAFETY: WRMSR is ring-0 only; the caller is the kernel.
    unsafe {
        asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") value as u32,
            in("edx") (value >> 32) as u32,
            options(nomem, nostack)
        )
    };
}

/// Switch to xAPIC mode so the APIC page is reachable through MMIO.
///
/// Returns the physical APIC base, which the caller must map.
pub fn enable_xapic() -> Result<u64, &'static str> {
    let mut base = read_msr(IA32_APIC_BASE);
    base &= !X2APIC_ENABLE;
    base |= APIC_ENABLE;
    write_msr(IA32_APIC_BASE, base);

    let physical = base & APIC_BASE_MASK;
    if physical == 0 {
        return Err("APIC base address is zero");
    }
    if physical != APIC_BASE_PHYSICAL {
        return Err("unexpected APIC base address (only 0xfee00000 is mapped)");
    }
    Ok(physical)
}

#[inline]
fn read32(base: *mut u32, offset: usize) -> u32 {
    // SAFETY: `base` points at the mapped APIC page; every offset used here is
    // a 4-byte aligned register inside that page.
    unsafe { read_volatile(base.add(offset / 4)) }
}

#[inline]
fn write32(base: *mut u32, offset: usize, value: u32) {
    // SAFETY: as above.
    unsafe { write_volatile(base.add(offset / 4), value) };
}

/// Unmask LINT0 for ExtINT delivery so the 8259 can reach the CPU.
///
/// `lapic` must be the kernel virtual address of the mapped APIC page.
pub fn configure_extint(lapic: *mut u32) -> Result<(), &'static str> {
    // Enable the APIC with a spurious vector and accept all priority classes.
    let svr = read32(lapic, SVR);
    write32(lapic, SVR, (svr & !0xff) | APIC_ENABLE_BIT | 0xff);
    if read32(lapic, SVR) & APIC_ENABLE_BIT == 0 {
        return Err("xAPIC software-enable bit did not stick");
    }
    write32(lapic, TPR, 0);

    // Keep every other LVT source masked during PIC bring-up.
    for offset in [LVT_TIMER, LVT_THERMAL, LVT_PERF, LVT_ERROR, LVT_LINT1] {
        write32(lapic, offset, read32(lapic, offset) | LVT_MASKED);
    }

    // LINT0: ExtINT delivery, unmasked. The vector field is ignored for
    // ExtINT, but zero is the architectural value.
    write32(lapic, LVT_LINT0, LVT_EXTINT);
    let lint0 = read32(lapic, LVT_LINT0);
    if lint0 & LVT_MASKED != 0 {
        return Err("LINT0 stayed masked");
    }
    if lint0 & (0b111 << 8) != LVT_EXTINT {
        return Err("LINT0 is not in ExtINT delivery mode");
    }
    Ok(())
}
