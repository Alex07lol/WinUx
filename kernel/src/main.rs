#![no_std]
#![no_main]

//! PROJECT_OS kernel — boot bring-up through heaps and memory management.
//!
//! Boot chain: Limine (bootloader) -> `_start` in `entry.S` -> `kmain`.
//! The kernel is a modular monolithic x86-64 kernel written in freestanding
//! Rust. This milestone validates the handoff, records the firmware memory map,
//! initialises descriptor tables, a physical frame allocator, a kernel heap
//! with the global allocator and a legacy timer, then reports a unique success
//! marker on serial before entering a controlled idle loop.

extern crate alloc;

mod arch;
mod blk;
mod config;
mod console;
mod e1000;
mod fbcon;
mod heap;
mod limine;
mod loader;
mod memory;
mod net;
mod panic;
mod pci;
mod pe_loader;
mod persist;
mod registry;
mod sched;
mod serial;
mod shell;
mod sync;
mod syscall;
mod user;
mod vfs;
mod vm;

use core::arch::{asm, global_asm};
use core::sync::atomic::Ordering;

use alloc::string::String;

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

fn fatal(args: core::fmt::Arguments<'_>) -> ! {
    // Render the whole line into a stack buffer and hold interrupts off while
    // it is written. A fatal diagnostic must not interleave with a still
    // running thread's serial output: during stage-3 bring-up a preempted
    // fatal writer produced split, misleading messages that masked the real
    // failure (a wait timeout while a user program was making progress).
    struct BufferWriter {
        bytes: [u8; 512],
        length: usize,
    }
    impl core::fmt::Write for BufferWriter {
        fn write_str(&mut self, text: &str) -> core::fmt::Result {
            // Two bytes are reserved for the trailing CRLF.
            let room = self.bytes.len().saturating_sub(self.length + 2);
            let source = text.as_bytes();
            let take = source.len().min(room);
            self.bytes[self.length..self.length + take].copy_from_slice(&source[..take]);
            self.length += take;
            Ok(())
        }
    }
    const PREFIX: &[u8] = b"\r\nPROJECT_OS_FATAL: ";
    let mut writer = BufferWriter {
        bytes: [0; 512],
        length: 0,
    };
    writer.bytes[..PREFIX.len()].copy_from_slice(PREFIX);
    writer.length = PREFIX.len();
    let _ = core::fmt::Write::write_fmt(&mut writer, args);
    writer.bytes[writer.length] = b'\r';
    writer.bytes[writer.length + 1] = b'\n';
    writer.length += 2;
    arch::x86_64::interrupts_disable();
    serial::force_write_bytes(&writer.bytes[..writer.length]);
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
fn load_memory_map() -> ([MemoryRegion; MAX_REGIONS], usize) {
    let mut regions = [MemoryRegion::new(0, 0, MemoryType::Reserved); MAX_REGIONS];
    let request = &limine::get().memmap;
    let Some(response) = request.get() else {
        fatal(format_args!("bootloader did not provide a memory map"));
    };
    if response.entries.is_null() {
        fatal(format_args!("memory map entry array is null"));
    }
    if response.entry_count > MAX_REGIONS as u64 {
        fatal(format_args!(
            "memory map has {} entries, kernel supports at most {}",
            response.entry_count, MAX_REGIONS
        ));
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
        fatal(format_args!(
            "bootloader does not support Limine base revision {}",
            os_core::LIMINE_BASE_REVISION
        ));
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
        None => fatal(format_args!("bootloader did not provide the HHDM offset")),
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
            Err(error) => fatal(format_args!("framebuffer geometry rejected: {:?}", error)),
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

    match fbcon::init(framebuffer) {
        Some((cols, rows, scale)) => {
            kprintln!("fbcon: enabled {}x{} cells at scale {}", cols, rows, scale)
        }
        None => kprintln!("fbcon: no framebuffer; screen console disabled"),
    }

    let (regions, region_count) = load_memory_map();
    if !memmap::is_sorted_by_base(&regions[..region_count]) {
        fatal(format_args!("memory map is not sorted by base address"));
    }
    if !memmap::usable_regions_do_not_overlap(&regions[..region_count]) {
        fatal(format_args!("usable memory-map regions overlap"));
    }
    let summary = memmap::summarize(regions[..region_count].iter().copied());
    if summary.overflowed {
        fatal(format_args!("memory map address arithmetic overflowed"));
    }
    if summary.usable_bytes == 0 {
        fatal(format_args!("memory map reports no usable RAM"));
    }
    if summary.unaligned_usable_regions != 0 {
        fatal(format_args!("memory map contains unaligned usable regions"));
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
    let allocator_stats = match memory::init(&regions[..region_count]) {
        Ok(stats) => stats,
        Err(error) => fatal(format_args!("{}", error)),
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
        Err(error) => fatal(format_args!("frame allocator selftest failed: {}", error)),
    }
    let heap_stats = match heap::init(hhdm_offset) {
        Ok(stats) => stats,
        Err(error) => fatal(format_args!("{}", error)),
    };
    kprintln!(
        "kernel heap: {} KiB region, {} KiB free, {} blocks, largest {} B",
        heap::HEAP_BYTES / 1024,
        heap_stats.free_bytes / 1024,
        heap_stats.block_count,
        heap_stats.largest_free_block
    );
    match heap::self_test() {
        Ok(_) => kprintln!("kernel heap selftest: passed (box, vec growth, 1 MiB block)"),
        Err(error) => fatal(format_args!("kernel heap selftest failed: {}", error)),
    }

    if let Err(error) = vm::init(hhdm_offset) {
        fatal(format_args!("{}", error));
    }
    kprintln!(
        "vm: kernel-owned 4-level tables active (CR3 {:#x}), W^X enforced",
        vm::kernel_pml4().unwrap_or(0)
    );
    kprintln!("vm selftest: passed (map/unmap, translate, W^X, guard refusals, CR3 swap)");

    if let Err(error) = vfs::init() {
        fatal(format_args!("{}", error));
    }
    match vfs::self_test() {
        Ok((nodes, files)) => kprintln!(
            "vfs selftest: passed ({} nodes, {} files; create/read/append/overwrite/rm/list/error paths)",
            nodes,
            files
        ),
        Err(error) => fatal(format_args!("vfs selftest failed: {}", error)),
    }

    // Windows x64 binaries assume the SSE/SSE2 baseline; rustc-generated PE
    // prologues spill xmm6, which faults with #UD until SIMD is enabled.
    arch::x86_64::enable_sse();
    kprintln!("cpu: SSE/SSE2 enabled for user-mode code");
    arch::x86_64::gdt::init();
    kprintln!("gdt: loaded (kernel code/data, TSS, IST1 double-fault stack)");
    idt::init();
    kprintln!("idt: loaded (32 exception gates + IRQ0 + int 0x80 syscall gate)");

    // Deliberate breakpoint: exercises the gate, stub, Rust dispatcher and
    // iretq before interrupts are enabled.
    // SAFETY: INT3 is handled by the installed vector-3 gate.
    unsafe { asm!("int3", options(nomem, nostack)) };
    if !idt::BREAKPOINT_HANDLED.load(Ordering::SeqCst) {
        fatal(format_args!("int3 self-test did not reach the handler"));
    }
    kprintln!("exception selftest: passed");

    pic::remap_and_mask_all();
    pit::init();
    pic::unmask_irq0();
    pic::unmask_irq(4);
    console::init();
    if let Err(error) = setup_apic_extint(hhdm_offset) {
        kprintln!(
            "apic: ExtINT setup unavailable ({}); using disabled-LAPIC path",
            error
        );
        arch::x86_64::disable_local_apic();
    }
    arch::x86_64::interrupts_enable();
    kprintln!("pit: {} Hz, IRQ0 unmasked", pit::effective_hz());
    kprintln!("console: COM1 receive interrupt enabled (IRQ4)");

    if !wait_for_ticks(5) {
        fatal(format_args!(
            "timer did not deliver 5 interrupts (observed {})",
            pit::ticks()
        ));
    }
    kprintln!("timer: {} ticks observed", pit::ticks());

    // Block-device probe. Runs after the PIT is ticking so poll deadlines are
    // real time; machines without the PIIX legacy IDE ports (e.g. q35) simply
    // have no disk and the probe logs that in one line.
    blk::init();
    // Load the best checkpoint or format slot A; inactive without a disk.
    persist::init();
    // Registry-like configuration store: `/etc/config` when the checkpoint has
    // it, otherwise defaults seeded in memory (the file is written lazily by
    // the first shell mutation, so an idle boot never touches the disk).
    if let Err(error) = config::init() {
        fatal(format_args!("config store failed: {}", error));
    }
    match config::self_test() {
        Ok((entries, bytes)) => kprintln!(
            "config selftest: passed ({} entries, {} bytes; get/set/remove/list/encode/decode/error paths)",
            entries,
            bytes
        ),
        Err(error) => fatal(format_args!("config selftest failed: {}", error)),
    }

    let tsc_frequency = limine::get()
        .tsc_frequency
        .get()
        .map(|response| response.frequency)
        .filter(|frequency| *frequency > 0);

    // PCI + network bring-up (Phase 6, stage 5). Runs with interrupts enabled
    // and the PIT ticking (before the scheduler exists), so the driver's poll
    // deadlines are real time. Non-fatal without a supported NIC: the probe
    // and selftest each log one pinned line and boot continues.
    net::init(hhdm_offset, tsc_frequency);
    net::selftest();

    if let Err(error) = sched::init(hhdm_offset) {
        fatal(format_args!("{}", error));
    }
    match sched::demo() {
        Ok(report) => kprintln!(
            "sched: {} workers finished (a={}, b={}, spinner={}), {} switches, {} preemptions, {} live",
            report.worker_a_steps.min(report.worker_b_steps),
            report.worker_a_steps,
            report.worker_b_steps,
            report.spinner_done,
            report.stats.switches,
            report.stats.preemptions,
            report.stats.live
        ),
        Err(error) => fatal(format_args!("scheduler selftest failed: {}", error)),
    }
    kprintln!("sched selftest: passed (round-robin yield + timer preemption)");

    let user_report = match user::demo() {
        Ok(report) => {
            if report.elf {
                kprintln!(
                    "user: ring-3 program exited (code {}), {} write(s), {} bytes, {} isolated fault(s), {} rejected syscall(s)",
                    report.exit_code,
                    report.write_calls,
                    report.bytes_written,
                    report.user_faults,
                    report.rejected_syscalls
                );
                kprintln!(
                    "user selftest: passed (ELF64 modules loaded, int 0x80 write/exit from CPL3, user fault isolated, corrupt module rejected)"
                );
            } else {
                kprintln!("user selftest: skipped (no ELF modules present)");
            }
            if report.pe {
                kprintln!(
                    "pe selftest: passed (PE32+ module loaded, base relocation applied, int 0x80 write from CPL3, corrupt module rejected)"
                );
            } else {
                kprintln!("pe selftest: skipped (no PE modules present)");
            }
            if report.win32 {
                kprintln!(
                    "win32 selftest: passed (kernel32.dll imports resolved, WriteConsoleA wrote to the console fd, ExitProcess ended the thread)"
                );
            }
            if report.win32_program {
                kprintln!(
                    "win32 program selftest: passed (WriteFile, GetLastError/SetLastError, ExitThread, GetCurrentProcessId/GetCurrentThreadId, GetTickCount, GetCommandLineA verified through kernel32.dll imports)"
                );
            }
            report
        }
        Err(error) => fatal(format_args!("user-mode selftest failed: {}", error)),
    };

    match fbcon::selftest() {
        fbcon::Selftest::Passed { compared, redraws } => kprintln!(
            "fbcon selftest: passed ({} bytes match after {} redraws)",
            compared,
            redraws
        ),
        fbcon::Selftest::Skipped => kprintln!("fbcon selftest: skipped (no framebuffer)"),
        fbcon::Selftest::Failed {
            compared,
            mismatched,
            first_row,
            first_offset,
            expected,
            actual,
        } => fatal(format_args!(
            "fbcon selftest failed: {} of {} bytes mismatched, first at row {} offset {} (expected {:#04x}, actual {:#04x})",
            mismatched, compared, first_row, first_offset, expected, actual
        )),
    }

    let mut checks =
        String::from("handoff,memmap,gdt,idt,exceptions,timer,allocator,heap,vm,sched,user");
    if user_report.elf {
        checks.push_str(",elf");
    }
    if user_report.pe {
        checks.push_str(",pe");
    }
    if user_report.win32 {
        checks.push_str(",win32");
    }
    checks.push_str(",vfs");
    checks.push_str(",config");
    if blk::info().is_some() {
        checks.push_str(",blk");
    }
    if persist::active() {
        checks.push_str(",persist");
    }
    if net::selftest_passed() {
        checks.push_str(",net");
    }
    kprintln!("PROJECT_OS_BOOT_SUCCESS:{} checks={}", BUILD_ID, checks);

    // QEMU's isa-debug-exit device (enabled only by the test runner) terminates
    // the VM with status (0x10 << 1) | 1 = 33. On VirtualBox the port is
    // unassigned, the write is ignored, and the kernel continues to the shell.
    if cmdline.contains("debug_exit") {
        arch::x86_64::io::outl(0xf4, 0x10);
    }

    // Interactive shell on the serial console. Boot success has already been
    // reported above, so a serial test that exits via isa-debug-exit never
    // reaches this code.
    shell::configure(BUILD_ID, tsc_frequency);
    if let Err(error) = sched::spawn("shell", shell_entry) {
        fatal(format_args!("cannot start the shell: {}", error));
    }
    // Hand the CPU to the shell thread. It never finishes, so this boot
    // context only runs again if the shell somehow exits; there is no
    // scheduler idle thread yet.
    sched::yield_now();

    loop {
        arch::x86_64::halt();
    }
}

/// Thread entry point for the shell (the scheduler wants a plain `fn()`).
fn shell_entry() {
    shell::run();
}
