//! Hardware IRQ dispatch, ownership and IOAPIC routing (Phase 9 stages 6-8).
//!
//! The timer (IRQ0) and COM1 (IRQ4) keep dedicated IDT paths in `idt.rs`.
//! Their IOAPIC GSIs, trigger mode and polarity come from ACPI MADT discovery;
//! the fixed stage-8 destination is GSI2/GSI4. `fallback_to_pic` returns both
//! lines to the 8259 if timer delivery does not start. Every other legacy line
//! still uses the 8259 until PCI INTx discovery exists. This module also
//! dispatches those lines to registered handlers, masks/unmasks them, and keeps
//! registration exclusive. See `docs/DRIVER_MODEL.md`, ADR-0034 and R-027.

use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use os_core::ioapic::{Polarity, RedirectionEntry, TriggerMode};

use crate::arch::x86_64::apic;
use crate::arch::x86_64::io::inb;
use crate::arch::x86_64::io::outb;
use crate::arch::x86_64::ioapic::IoApic;

pub const MAX_IRQ: u8 = 16;
const TIMER_LINE: u8 = 0;
const CONSOLE_LINE: u8 = 4;
const TIMER_VECTOR: u8 = 32;
const CONSOLE_VECTOR: u8 = 36;
/// Route marker meaning "handled by the 8259", not an IOAPIC GSI.
const PIC_ROUTE: u8 = 0xff;

type Handler = fn();

static HANDLERS: [AtomicUsize; 16] = [const { AtomicUsize::new(0) }; 16];
static ROUTES: [AtomicU8; 16] = [const { AtomicU8::new(PIC_ROUTE) }; 16];
static IOAPIC_BASE: AtomicUsize = AtomicUsize::new(0);
static LAPIC_BASE: AtomicUsize = AtomicUsize::new(0);

/// Probed IOAPIC identity and the routes programmed into it.
pub struct IoApicInfo {
    pub version: u8,
    pub pins: u8,
    pub id: u8,
    pub timer_gsi: u8,
    pub console_gsi: u8,
}

/// One ISA route supplied by ACPI discovery or the fixed fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegacyRoute {
    pub gsi: u8,
    pub trigger: TriggerMode,
    pub polarity: Polarity,
}

/// True when `line` is a legacy interrupt line this module can assign. Timer
/// and COM1 are reserved and never handed to a device driver.
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

/// Dispatch one hardware IRQ: run the handler, then acknowledge it through the
/// route that delivered it.
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

/// End-of-interrupt for `line`. An IOAPIC route acknowledges the local APIC;
/// a PIC route acknowledges the 8259 (slave first for lines 8..15).
pub fn eoi(line: u8) {
    if line >= MAX_IRQ {
        return;
    }
    if route(line) != PIC_ROUTE {
        let lapic = LAPIC_BASE.load(Ordering::Acquire);
        if lapic != 0 {
            apic::eoi(lapic as *mut u32);
            return;
        }
    }
    if line >= 8 {
        outb(0xA0, 0x20);
    }
    outb(0x20, 0x20);
}

/// Mask one line through its current route.
pub fn mask(line: u8) {
    if line >= MAX_IRQ {
        return;
    }
    let gsi = route(line);
    if gsi != PIC_ROUTE {
        if let Some(ioapic) = ioapic() {
            let _ = ioapic.mask(gsi);
        }
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

/// Unmask one line through its current route, including the PIC cascade for
/// slave lines.
pub fn unmask(line: u8) {
    if line >= MAX_IRQ {
        return;
    }
    let gsi = route(line);
    if gsi != PIC_ROUTE {
        if let Some(ioapic) = ioapic() {
            let _ = ioapic.unmask(gsi);
        }
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

/// Program the discovered PIT and COM1 routes into the selected IOAPIC.
///
/// Every entry is masked first, then the two routes are installed. On failure
/// the table is left fully masked and the caller keeps the 8259 path. The PIT
/// output is normally edge-triggered; the UART's interrupt line is normally
/// level asserted, so COM1 uses level-high to avoid losing a byte that arrives
/// while the handler is draining the FIFO. MADT overrides can change both.
pub fn init_ioapic(
    base: *mut u32,
    lapic: *mut u32,
    timer_route: LegacyRoute,
    console_route: LegacyRoute,
) -> Result<IoApicInfo, &'static str> {
    if timer_route.gsi == PIC_ROUTE || console_route.gsi == PIC_ROUTE {
        return Err("ISA route uses the reserved PIC marker");
    }
    if timer_route.gsi == console_route.gsi {
        return Err("timer and console share one GSI");
    }
    let ioapic = IoApic::new(base)?;
    ioapic.mask_all()?;
    let destination = apic::local_id(lapic);
    let timer = RedirectionEntry::fixed(
        TIMER_VECTOR,
        destination,
        timer_route.trigger,
        timer_route.polarity,
        false,
    )
    .ok_or("timer vector is reserved")?;
    let console = RedirectionEntry::fixed(
        CONSOLE_VECTOR,
        destination,
        console_route.trigger,
        console_route.polarity,
        false,
    )
    .ok_or("console vector is reserved")?;
    ioapic.write_entry(timer_route.gsi, timer)?;
    if let Err(error) = ioapic.write_entry(console_route.gsi, console) {
        let _ = ioapic.mask_all();
        return Err(error);
    }

    let info = IoApicInfo {
        version: ioapic.version(),
        pins: ioapic.pins(),
        id: ioapic.id(),
        timer_gsi: timer_route.gsi,
        console_gsi: console_route.gsi,
    };
    IOAPIC_BASE.store(base as usize, Ordering::Release);
    LAPIC_BASE.store(lapic as usize, Ordering::Release);
    ROUTES[TIMER_LINE as usize].store(timer_route.gsi, Ordering::Release);
    ROUTES[CONSOLE_LINE as usize].store(console_route.gsi, Ordering::Release);
    Ok(info)
}

/// Return the PIT and COM1 lines to the 8259 after an IOAPIC timer-delivery
/// timeout. The LAPIC stays enabled for LINT0 ExtINT.
pub fn fallback_to_pic() {
    if let Some(ioapic) = ioapic() {
        let timer_gsi = route(TIMER_LINE);
        if timer_gsi != PIC_ROUTE {
            let _ = ioapic.mask(timer_gsi);
        }
        let console_gsi = route(CONSOLE_LINE);
        if console_gsi != PIC_ROUTE {
            let _ = ioapic.mask(console_gsi);
        }
    }
    ROUTES[TIMER_LINE as usize].store(PIC_ROUTE, Ordering::Release);
    ROUTES[CONSOLE_LINE as usize].store(PIC_ROUTE, Ordering::Release);
    IOAPIC_BASE.store(0, Ordering::Release);
    LAPIC_BASE.store(0, Ordering::Release);
    crate::arch::x86_64::pic::unmask_irq0();
    crate::arch::x86_64::pic::unmask_irq(CONSOLE_LINE);
}

/// Whether the PIT is currently delivered through the IOAPIC.
pub fn timer_uses_ioapic() -> bool {
    route(TIMER_LINE) != PIC_ROUTE
}

pub fn timer_gsi() -> u8 {
    route(TIMER_LINE)
}

pub fn console_gsi() -> u8 {
    route(CONSOLE_LINE)
}

fn route(line: u8) -> u8 {
    if line >= MAX_IRQ {
        return PIC_ROUTE;
    }
    ROUTES[line as usize].load(Ordering::Acquire)
}

fn ioapic() -> Option<IoApic> {
    let base = IOAPIC_BASE.load(Ordering::Acquire);
    if base == 0 {
        return None;
    }
    IoApic::new(base as *mut u32).ok()
}
