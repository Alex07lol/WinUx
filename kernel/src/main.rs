#![no_std]
#![no_main]

//! PROJECT_OS kernel — first bootable milestone.
//!
//! Boot chain: Limine (bootloader) -> `_start` in `entry.S` -> `kmain`.
//! The kernel is a modular monolithic x86-64 kernel written in freestanding
//! Rust. This milestone validates the handoff, records the firmware memory map,
//! initialises descriptor tables, a physical frame allocator and a legacy
//! timer, then reports a unique success marker on serial before entering a
//! controlled idle loop.

mod arch;
mod display;
mod font;
mod limine;
mod memory;
mod panic;
mod serial;
mod sync;

use core::arch::{asm, global_asm};
use core::sync::atomic::Ordering;

use display::{BootDisplay, FramesInfo, Status};
use os_core::fb::{FramebufferGeometry, PixelFormat};
use os_core::memmap::{self, MemoryRegion, MemoryType};

use arch::x86_64::{idt, pic, pit};

global_asm!(include_str!("entry.S"));

/// Supplied by the build scripts for reproducible builds; `dev` otherwise.
const BUILD_ID: &str = match option_env!("PROJECT_OS_BUILD_ID") {
    Some(value) => value,
    None => "dev",
};
const MAX_REGIONS: usize = 256;

fn fatal(display: Option<&mut BootDisplay>, args: core::fmt::Arguments<'_>) -> ! {
    serial::force_write_str("\r\nPROJECT_OS_FATAL: ");
    serial::force_print(args);
    serial::force_write_str("\r\n");
    if let Some(display) = display {
        display.fail(args);
    }
    arch::halt_forever()
}

fn firmware_name(kind: u64) -> &'static str {
    match kind {
        limine::FIRMWARE_X86_BIOS => "BIOS (x86)",
        limine::FIRMWARE_EFI32 => "UEFI (32-bit)",
        limine::FIRMWARE_EFI64 => "UEFI (64-bit)",
        _ => "unknown",
    }
}

fn paging_name(mode: u64) -> &'static str {
    match mode {
        limine::PAGING_MODE_X86_64_4LVL => "4-level",
        limine::PAGING_MODE_X86_64_5LVL => "5-level",
        _ => "unknown",
    }
}

/// Copy the bootloader memory map into kernel-owned storage. Bootloader
/// responses live in reclaimable memory, so the kernel keeps its own copy.
fn load_memory_map(display: &mut BootDisplay) -> ([MemoryRegion; MAX_REGIONS], usize) {
    let mut regions = [MemoryRegion::new(0, 0, MemoryType::Reserved); MAX_REGIONS];
    let request = &limine::get().memmap;
    let Some(response) = request.get() else {
        fatal(
            Some(display),
            format_args!("bootloader did not provide a memory map"),
        );
    };
    if response.entries.is_null() {
        fatal(
            Some(display),
            format_args!("memory map entry array is null"),
        );
    }
    if response.entry_count > MAX_REGIONS as u64 {
        fatal(
            Some(display),
            format_args!(
                "memory map has {} entries, kernel supports at most {}",
                response.entry_count, MAX_REGIONS
            ),
        );
    }

    let mut count = 0usize;
    for index in 0..response.entry_count as usize {
        // SAFETY: the protocol guarantees `entries` points to `entry_count`
        // valid pointers; each entry points to a valid `limine_memmap_entry`.
        let entry = unsafe { &**response.entries.add(index) };
        regions[count] =
            MemoryRegion::new(entry.base, entry.length, MemoryType::from_raw(entry.kind));
        count += 1;
    }
    (regions, count)
}

fn framebuffer_geometry() -> Option<(FramebufferGeometry, *mut u8)> {
    let request = &limine::get().framebuffer;
    let response = request.get()?;
    if response.framebuffer_count == 0 || response.framebuffers.is_null() {
        return None;
    }
    // SAFETY: the response guarantees an array of at least one pointer.
    let framebuffer = unsafe { &**response.framebuffers };
    if framebuffer.address.is_null() {
        return None;
    }
    let format = PixelFormat::new(
        framebuffer.bpp,
        framebuffer.red_mask_size,
        framebuffer.red_mask_shift,
        framebuffer.green_mask_size,
        framebuffer.green_mask_shift,
        framebuffer.blue_mask_size,
        framebuffer.blue_mask_shift,
    );
    Some((
        FramebufferGeometry::new(
            framebuffer.width,
            framebuffer.height,
            framebuffer.pitch,
            format,
        ),
        framebuffer.address,
    ))
}

/// Map the local APIC page and put LINT0 into ExtINT mode so PIC IRQs are
/// accepted. Falls back to the disabled-LAPIC path if mapping is unavailable.
fn setup_apic_extint(hhdm_offset: u64) -> Result<(), &'static str> {
    let physical = arch::x86_64::apic::enable_xapic()?;
    let aligned = physical & !((2 * 1024 * 1024) - 1);
    let virtual_base = hhdm_offset
        .checked_add(aligned)
        .ok_or("APIC HHDM address overflowed")?;
    arch::x86_64::paging::PageTables::new(hhdm_offset).map_mmio_2m(aligned, virtual_base)?;
    let lapic = hhdm_offset
        .checked_add(physical)
        .ok_or("APIC HHDM address overflowed")? as *mut u32;
    arch::x86_64::apic::configure_extint(lapic)
}

fn wait_for_ticks(target: u64) -> bool {
    let frequency = limine::get()
        .tsc_frequency
        .get()
        .map(|response| response.frequency)
        .filter(|frequency| *frequency > 0);

    match frequency {
        Some(frequency) => {
            let start = arch::x86_64::read_tsc();
            let limit = frequency.saturating_mul(2);
            while pit::ticks() < target {
                if arch::x86_64::read_tsc().wrapping_sub(start) > limit {
                    return false;
                }
                // Busy-wait rather than `hlt`: a broken interrupt route must
                // hit the bounded timeout and report, not sleep forever.
                core::hint::spin_loop();
            }
            true
        }
        None => {
            // No TSC frequency from the bootloader: spin with a bounded budget
            // rather than risk an unbounded halt if the timer never fires.
            let mut budget = 200_000_000u64;
            while pit::ticks() < target {
                if budget == 0 {
                    return false;
                }
                budget -= 1;
                core::hint::spin_loop();
            }
            true
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kmain() -> ! {
    serial::init();

    kprintln!();
    kprintln!("PROJECT_OS: independent x86-64 kernel");
    kprintln!(
        "build: {} v{} (target x86_64-unknown-none)",
        BUILD_ID,
        os_core::VERSION
    );

    let loaded_revision = limine::loaded_base_revision();
    match loaded_revision {
        Some(loaded) => kprintln!(
            "limine: requested base revision {}, loaded {}",
            os_core::LIMINE_BASE_REVISION,
            loaded
        ),
        None => kprintln!(
            "limine: requested base revision {}, loaded revision not reported",
            os_core::LIMINE_BASE_REVISION
        ),
    }
    if !limine::base_revision_supported() {
        fatal(
            None,
            format_args!(
                "bootloader does not support Limine base revision {}",
                os_core::LIMINE_BASE_REVISION
            ),
        );
    }

    let (bootloader_name, bootloader_version) = limine::get()
        .bootloader_info
        .get()
        .map(|info| (limine::cstr(info.name), limine::cstr(info.version)))
        .unwrap_or(("unknown bootloader", ""));
    kprintln!("bootloader: {} {}", bootloader_name, bootloader_version);

    let firmware = limine::get()
        .firmware_type
        .get()
        .map(|response| firmware_name(response.firmware_type))
        .unwrap_or("unknown");
    kprintln!("firmware: {}", firmware);

    let paging = limine::get()
        .paging_mode
        .get()
        .map(|response| paging_name(response.mode))
        .unwrap_or("unknown");
    kprintln!("paging: {}", paging);

    let hhdm_offset = match limine::get().hhdm.get() {
        Some(response) => response.offset,
        None => fatal(
            None,
            format_args!("bootloader did not provide the HHDM offset"),
        ),
    };
    kprintln!("hhdm: {:#018x}", hhdm_offset);

    match limine::get().executable_address.get() {
        Some(response) => kprintln!(
            "kernel: virtual {:#018x} physical {:#018x}",
            response.virtual_base,
            response.physical_base
        ),
        None => kprintln!("kernel: executable address response missing"),
    }
    let cmdline = limine::get()
        .executable_cmdline
        .get()
        .map(|response| limine::cstr(response.cmdline))
        .unwrap_or("");
    kprintln!("cmdline: {:?}", cmdline);

    let framebuffer = framebuffer_geometry();
    let framebuffer = match framebuffer {
        Some((geometry, address)) => match geometry.validate() {
            Ok(()) => Some((geometry, address)),
            Err(error) => fatal(
                None,
                format_args!("framebuffer geometry rejected: {:?}", error),
            ),
        },
        None => None,
    };
    if let Some((geometry, address)) = framebuffer {
        kprintln!(
            "framebuffer: {}x{} pitch {} bpp {} at {:p}",
            geometry.width,
            geometry.height,
            geometry.pitch,
            geometry.format.bpp,
            address
        );
    } else {
        kprintln!("framebuffer: not available; serial-only boot");
    }

    let mut display = BootDisplay::new(framebuffer, BUILD_ID);

    if let Some((geometry, _)) = framebuffer {
        display.set_row(
            3,
            format_args!(
                "{}x{}x{}",
                geometry.width, geometry.height, geometry.format.bpp
            ),
            Status::Ok,
        );
    } else {
        display.set_row(3, format_args!("not available"), Status::Warn);
    }
    match loaded_revision {
        Some(revision) => display.set_row(
            0,
            format_args!(
                "{} {} | revision {}",
                bootloader_name, bootloader_version, revision
            ),
            Status::Ok,
        ),
        None => display.set_row(
            0,
            format_args!("{} | revision unknown", bootloader_name),
            Status::Ok,
        ),
    }
    display.set_row(1, format_args!("{}", firmware), Status::Ok);
    display.set_row(2, format_args!("{}", paging), Status::Ok);

    let (regions, region_count) = load_memory_map(&mut display);
    if !memmap::is_sorted_by_base(&regions[..region_count]) {
        fatal(
            Some(&mut display),
            format_args!("memory map is not sorted by base address"),
        );
    }
    if !memmap::usable_regions_do_not_overlap(&regions[..region_count]) {
        fatal(
            Some(&mut display),
            format_args!("usable memory-map regions overlap"),
        );
    }
    let summary = memmap::summarize(regions[..region_count].iter().copied());
    if summary.overflowed {
        fatal(
            Some(&mut display),
            format_args!("memory map address arithmetic overflowed"),
        );
    }
    if summary.usable_bytes == 0 {
        fatal(
            Some(&mut display),
            format_args!("memory map reports no usable RAM"),
        );
    }
    if summary.unaligned_usable_regions != 0 {
        fatal(
            Some(&mut display),
            format_args!("memory map contains unaligned usable regions"),
        );
    }
    kprintln!(
        "memory map: {} entries, {} MiB usable, {} MiB framebuffer, {} MiB bootloader-reclaimable, highest end {:#x}",
        summary.entries,
        summary.usable_bytes / (1024 * 1024),
        summary.framebuffer_bytes / (1024 * 1024),
        summary.bootloader_reclaimable_bytes / (1024 * 1024),
        summary.highest_end
    );
    for (index, region) in regions[..region_count].iter().enumerate() {
        kprintln!(
            "  [{:2}] {:#012x}-{:#012x} {:>25} {:>10} KiB",
            index,
            region.base,
            region.checked_end().unwrap_or(region.base),
            region.kind.label(),
            region.length / 1024
        );
    }
    display.set_row(
        4,
        format_args!(
            "{} regions | {} MiB usable",
            summary.entries,
            summary.usable_bytes / (1024 * 1024)
        ),
        Status::Ok,
    );
    display.set_memory(&summary);

    let allocator_stats = match memory::init(&regions[..region_count]) {
        Ok(stats) => stats,
        Err(error) => fatal(Some(&mut display), format_args!("{}", error)),
    };
    kprintln!(
        "frame allocator: {} free / {} frames ({} MiB managed), largest run {} frames",
        allocator_stats.free_frames,
        allocator_stats.total_frames,
        allocator_stats.total_bytes / (1024 * 1024),
        allocator_stats.largest_free_run
    );
    match memory::self_test(hhdm_offset) {
        Ok(_) => kprintln!("frame allocator selftest: passed"),
        Err(error) => fatal(
            Some(&mut display),
            format_args!("frame allocator selftest failed: {}", error),
        ),
    }
    display.set_frames(FramesInfo {
        free_frames: allocator_stats.free_frames as u64,
        total_frames: allocator_stats.total_frames as u64,
        largest_free_run: allocator_stats.largest_free_run as u64,
    });
    display.set_row(
        5,
        format_args!(
            "{} / {} frames free",
            allocator_stats.free_frames, allocator_stats.total_frames
        ),
        Status::Ok,
    );

    arch::x86_64::gdt::init();
    kprintln!("gdt: loaded (kernel code/data, TSS, IST1 double-fault stack)");
    idt::init();
    kprintln!("idt: loaded (32 exception gates + IRQ0)");

    // Deliberate breakpoint: exercises the gate, stub, Rust dispatcher and
    // iretq before interrupts are enabled.
    // SAFETY: INT3 is handled by the installed vector-3 gate.
    unsafe { asm!("int3", options(nomem, nostack)) };
    if !idt::BREAKPOINT_HANDLED.load(Ordering::SeqCst) {
        fatal(
            Some(&mut display),
            format_args!("int3 self-test did not reach the handler"),
        );
    }
    kprintln!("exception selftest: passed");
    display.set_row(6, format_args!("IDT | TSS/IST1 | INT3"), Status::Ok);

    pic::remap_and_mask_all();
    pit::init();
    pic::unmask_irq0();
    if let Err(error) = setup_apic_extint(hhdm_offset) {
        kprintln!(
            "apic: ExtINT setup unavailable ({}); using disabled-LAPIC path",
            error
        );
        arch::x86_64::disable_local_apic();
    }
    arch::x86_64::interrupts_enable();
    kprintln!("pit: {} Hz, IRQ0 unmasked", pit::effective_hz());

    if !wait_for_ticks(5) {
        fatal(
            Some(&mut display),
            format_args!(
                "timer did not deliver 5 interrupts (observed {})",
                pit::ticks()
            ),
        );
    }
    kprintln!("timer: {} ticks observed", pit::ticks());
    display.set_timer(pit::effective_hz(), pit::ticks());
    display.set_row(
        7,
        format_args!("PIT {} Hz | {} ticks", pit::effective_hz(), pit::ticks()),
        Status::Ok,
    );

    display.success();
    kprintln!(
        "PROJECT_OS_BOOT_SUCCESS:{} checks=handoff,memmap,gdt,idt,exceptions,timer,allocator",
        BUILD_ID
    );

    // QEMU's isa-debug-exit device (enabled only by the test runner) terminates
    // the VM with status (0x10 << 1) | 1 = 33. On VirtualBox the port is
    // unassigned, the write is ignored, and the kernel falls through to idle.
    if cmdline.contains("debug_exit") {
        arch::x86_64::io::outl(0xf4, 0x10);
    }

    loop {
        arch::x86_64::halt();
    }
}
