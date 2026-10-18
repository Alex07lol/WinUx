//! Ring-3 user-mode demonstration and self-test (Phase 6, stage 3; PE32+
//! phase 7, stage 1).
//!
//! User programs are ELF64 executables built from `userland/` (Rust, no_std)
//! and PE32+ executables built from `userland/pe`, shipped as Limine modules
//! and loaded by `loader.rs` / `pe_loader.rs` into a fresh paged address
//! space:
//!
//! * `hello.elf` runs at CPL3, performs one `int 0x80` console write and exits
//!   through `SYS_EXIT` with code 0.
//! * `fault.elf` runs at CPL3 and writes to an unmapped user address, raising
//!   a page fault from CPL3. The kernel reports it, kills the thread and
//!   continues boot — the fault-isolation property.
//! * `bad.elf` (a corrupted copy of `hello.elf`) must be rejected by the
//!   loader probe before any memory is touched.
//! * `pehello.exe` runs at CPL3 through the two-phase PE32+ loader: mapped
//!   with W^X section permissions, base-relocated (its `global_asm!` slot
//!   checks the DIR64 fixup at runtime), two console writes, exit 0.
//! * `pebad.exe` (signature bytes zeroed) must be rejected with
//!   `bad signature` before any page is allocated.
//!
//! Each program runs one at a time on its own scheduler thread. The thread
//! publishes its address space and load plan into a static slot; after the
//! thread dies the boot context switches CR3 back to the kernel template,
//! unmaps every page of the load plan plus the stack, and destroys the space.

use core::arch::global_asm;
use core::sync::atomic::Ordering;

use os_core::frame::PhysFrame;
use os_core::paging::{PAGE_SIZE, USER_DATA};

use crate::arch::x86_64::gdt::{USER_CODE_SELECTOR, USER_DATA_SELECTOR, set_tss_rsp0};
use crate::kprintln;
use crate::limine;
use crate::loader;
use crate::memory;
use crate::pe_loader;
use crate::sched;
use crate::serial;
use crate::sync::IrqSpinLock;
use crate::syscall;
use crate::vm::{self, AddressSpace};

const HELLO_PATH: &str = "/boot/hello.elf";
const FAULT_PATH: &str = "/boot/fault.elf";
const BAD_PATH: &str = "/boot/bad.elf";
const PEHELLO_PATH: &str = "/boot/pehello.exe";
const PEBAD_PATH: &str = "/boot/pebad.exe";

/// Single user stack page, at the stage-5b placement just above the code
/// window (`loader::CODE_END` == `STACK_VA`).
const STACK_VA: u64 = 0x50_0000;
const STACK_TOP: u64 = STACK_VA + 0x1000;

global_asm!(
    r#"
.section .text
.p2align 4

// fn(entry: u64, stack_top: u64, code_selector: u64, data_selector: u64) -> !
// (SysV registers rdi, rsi, rdx, rcx). Loads the user data selector, builds
// the iretq frame (ss, rsp, rflags with IF set, cs, rip) and enters ring 3.
.global project_os_enter_user_mode
project_os_enter_user_mode:
    mov ds, cx
    mov es, cx
    push rcx
    push rsi
    push 0x202
    push rdx
    push rdi
    iretq
"#
);

unsafe extern "C" {
    fn project_os_enter_user_mode(
        entry: u64,
        stack_top: u64,
        code_selector: u64,
        data_selector: u64,
    ) -> !;
}

/// The load plan of the program currently occupying [`USER_SLOT`], kept so
/// the boot context can unmap exactly the pages the loader mapped.
// At most one plan exists at a time inside a single static slot; the PE
// variant is larger but there is no collection of these to size against.
#[allow(clippy::large_enum_variant)]
enum LoadedPlan {
    Elf(loader::Inspection),
    Pe(pe_loader::Inspection),
}

/// The running thread publishes its address space and load plan here; the
/// boot context tears both down after the thread exits or is killed. Exactly
/// one user program runs at a time, so one slot suffices.
static USER_SLOT: IrqSpinLock<Option<(AddressSpace, LoadedPlan)>> = IrqSpinLock::new(None);

#[derive(Clone, Copy)]
pub struct DemoReport {
    pub exit_code: u64,
    pub write_calls: u64,
    pub bytes_written: u64,
    pub user_faults: u64,
    pub rejected_syscalls: u64,
    /// False when the ELF self-test was skipped (no modules / missing module).
    pub elf: bool,
    /// False when the PE self-test was skipped (no modules / missing module).
    pub pe: bool,
}

fn thread_fatal(message: core::fmt::Arguments<'_>) -> ! {
    serial::force_print(format_args!(
        "\r\nPROJECT_OS_FATAL: user setup: {}\r\n",
        message
    ));
    sched::exit()
}

/// Build the address space for one ELF module, map the zeroed stack, switch
/// CR3, program the TSS stack and enter ring 3 at the ELF entry. Returns only
/// through the kernel on failure.
fn run_elf(path: &'static str) -> ! {
    let Some(module) = limine::module_by_path(path) else {
        thread_fatal(format_args!("module {} disappeared before load", path));
    };
    let image = match loader::inspect(module) {
        Ok(image) => image,
        Err(reject) => thread_fatal(format_args!("{} rejected: {}", path, reject.label())),
    };
    let Some(hhdm) = vm::hhdm_offset() else {
        thread_fatal(format_args!("HHDM offset is unknown"));
    };
    let mut space = match AddressSpace::new_like_kernel(hhdm) {
        Ok(space) => space,
        Err(_) => thread_fatal(format_args!("cannot create the user address space")),
    };
    if let Err(reject) = loader::map(&mut space, module, &image) {
        let _ = space.destroy();
        thread_fatal(format_args!("cannot load {}: {}", path, reject.label()));
    }
    let stack_frame = match memory::alloc_frame() {
        Ok(frame) => frame,
        Err(_) => thread_fatal(format_args!("no frame for the user stack")),
    };
    let Some(stack_window) = hhdm.checked_add(stack_frame.address()) else {
        let _ = memory::free_pages(stack_frame, 1);
        let _ = space.destroy();
        thread_fatal(format_args!("user stack HHDM overflow"));
    };
    // SAFETY: the frame was just allocated for this stack and is not mapped
    // elsewhere; HHDM maps all of usable RAM. The stack starts zeroed so the
    // program cannot observe stale kernel data.
    unsafe { core::ptr::write_bytes(stack_window as *mut u8, 0, PAGE_SIZE as usize) };
    if space
        .map_page(STACK_VA, stack_frame.address(), USER_DATA)
        .is_err()
    {
        let _ = memory::free_pages(stack_frame, 1);
        let _ = space.destroy();
        thread_fatal(format_args!("mapping the user stack failed"));
    }
    let kernel_stack_top = match sched::current_kernel_stack_top() {
        Some(top) => top,
        None => thread_fatal(format_args!("thread has no kernel stack")),
    };

    // Enter the new address space, then publish it for teardown. The kernel
    // half is shared with the template, so kernel code (including the timer
    // ISR and any preemption) keeps working with the user CR3 loaded.
    if space.activate().is_err() {
        thread_fatal(format_args!("activating the user address space failed"));
    }
    let entry = image.entry();
    *USER_SLOT.lock() = Some((space, LoadedPlan::Elf(image)));

    set_tss_rsp0(kernel_stack_top);
    // SAFETY: `entry` points into a mapped executable segment of the
    // now-active address space and STACK_VA is its mapped, zeroed user page.
    unsafe {
        project_os_enter_user_mode(
            entry,
            STACK_TOP,
            u64::from(USER_CODE_SELECTOR),
            u64::from(USER_DATA_SELECTOR),
        )
    }
}

fn thread_hello() {
    run_elf(HELLO_PATH);
}

fn thread_fault() {
    run_elf(FAULT_PATH);
}

/// Build the address space for one PE module (inspect, map, apply DIR64 base
/// relocations), map the zeroed stack, switch CR3, program the TSS stack and
/// enter ring 3 at the PE entry. Returns only through the kernel on failure.
fn run_pe(path: &'static str) -> ! {
    let Some(module) = limine::module_by_path(path) else {
        thread_fatal(format_args!("module {} disappeared before load", path));
    };
    let image = match pe_loader::inspect(module) {
        Ok(image) => image,
        Err(reject) => thread_fatal(format_args!("{} rejected: {}", path, reject.label())),
    };
    let Some(hhdm) = vm::hhdm_offset() else {
        thread_fatal(format_args!("HHDM offset is unknown"));
    };
    let mut space = match AddressSpace::new_like_kernel(hhdm) {
        Ok(space) => space,
        Err(_) => thread_fatal(format_args!("cannot create the user address space")),
    };
    let applied = match pe_loader::map(&mut space, module, &image) {
        Ok(applied) => applied,
        Err(reject) => {
            let _ = space.destroy();
            thread_fatal(format_args!("cannot load {}: {}", path, reject.label()));
        }
    };
    let entry = loader::CODE_BASE + u64::from(image.entry_rva());
    kprintln!(
        "pe: {} loaded at {:#x} (entry {:#x}, {} sections, {} relocation{} applied)",
        path.rsplit('/').next().unwrap_or(path),
        loader::CODE_BASE,
        entry,
        image.sections().len(),
        applied,
        if applied == 1 { "" } else { "s" }
    );
    let stack_frame = match memory::alloc_frame() {
        Ok(frame) => frame,
        Err(_) => thread_fatal(format_args!("no frame for the user stack")),
    };
    let Some(stack_window) = hhdm.checked_add(stack_frame.address()) else {
        let _ = memory::free_pages(stack_frame, 1);
        let _ = space.destroy();
        thread_fatal(format_args!("user stack HHDM overflow"));
    };
    // SAFETY: the frame was just allocated for this stack and is not mapped
    // elsewhere; HHDM maps all of usable RAM. The stack starts zeroed so the
    // program cannot observe stale kernel data.
    unsafe { core::ptr::write_bytes(stack_window as *mut u8, 0, PAGE_SIZE as usize) };
    if space
        .map_page(STACK_VA, stack_frame.address(), USER_DATA)
        .is_err()
    {
        let _ = memory::free_pages(stack_frame, 1);
        let _ = space.destroy();
        thread_fatal(format_args!("mapping the user stack failed"));
    }
    let kernel_stack_top = match sched::current_kernel_stack_top() {
        Some(top) => top,
        None => thread_fatal(format_args!("thread has no kernel stack")),
    };

    if space.activate().is_err() {
        thread_fatal(format_args!("activating the user address space failed"));
    }
    *USER_SLOT.lock() = Some((space, LoadedPlan::Pe(image)));

    set_tss_rsp0(kernel_stack_top);
    // SAFETY: `entry` points into a mapped executable section of the
    // now-active address space and STACK_VA is its mapped, zeroed user page.
    unsafe {
        project_os_enter_user_mode(
            entry,
            STACK_TOP,
            u64::from(USER_CODE_SELECTOR),
            u64::from(USER_DATA_SELECTOR),
        )
    }
}

fn thread_pehello() {
    run_pe(PEHELLO_PATH);
}

fn unmap_and_free(space: &mut AddressSpace, virtual_address: u64) -> Result<(), &'static str> {
    if let Ok(physical) = space.unmap_page(virtual_address)
        && let Some(frame) = PhysFrame::from_address(physical)
    {
        memory::free_pages(frame, 1).map_err(|_| "freeing a user frame failed")?;
    }
    Ok(())
}

fn cleanup_slot() -> Result<(), &'static str> {
    let Some((mut space, plan)) = USER_SLOT.lock().take() else {
        return Ok(());
    };
    match plan {
        LoadedPlan::Elf(image) => {
            for segment in image.segments() {
                let range = segment.page_range();
                let mut page = range.start;
                while page < range.end {
                    unmap_and_free(&mut space, page)?;
                    page += PAGE_SIZE;
                }
            }
        }
        LoadedPlan::Pe(image) => {
            for section in image.sections() {
                let range = pe_loader::section_page_range(section);
                let mut page = range.start;
                while page < range.end {
                    unmap_and_free(&mut space, page)?;
                    page += PAGE_SIZE;
                }
            }
        }
    }
    unmap_and_free(&mut space, STACK_VA)?;
    space
        .destroy()
        .map_err(|_| "destroying a user space failed")
}

fn idle_report() -> DemoReport {
    DemoReport {
        exit_code: syscall::LAST_EXIT_CODE.load(Ordering::SeqCst),
        write_calls: syscall::WRITE_CALLS.load(Ordering::SeqCst),
        bytes_written: syscall::BYTES_WRITTEN.load(Ordering::SeqCst),
        user_faults: syscall::USER_FAULTS.load(Ordering::SeqCst),
        rejected_syscalls: syscall::REJECTED_SYSCALLS.load(Ordering::SeqCst),
        elf: false,
        pe: false,
    }
}

/// The path of a module entry, or an empty string for a null pointer.
fn module_name(file: &*const limine::File) -> &'static str {
    if file.is_null() {
        return "";
    }
    // SAFETY: non-null Limine module entries point to valid `limine_file`
    // structs for the whole boot.
    limine::cstr(unsafe { (**file).path })
}

/// Boot-time demonstration. The scheduler must already be initialised.
pub fn demo(tsc_frequency: Option<u64>) -> Result<DemoReport, &'static str> {
    sched::set_preemption(true);

    let Some(modules) = limine::modules().filter(|modules| !modules.is_empty()) else {
        kprintln!("user: no Limine modules present; skipping the ELF self-test");
        sched::set_preemption(false);
        return Ok(idle_report());
    };
    let elf_modules = modules
        .iter()
        .filter(|file| module_name(file).ends_with(".elf"))
        .count();
    let pe_modules = modules
        .iter()
        .filter(|file| module_name(file).ends_with(".exe"))
        .count();
    kprintln!("elf: {} modules present", elf_modules);
    kprintln!("pe: {} modules present", pe_modules);

    for path in [HELLO_PATH, FAULT_PATH, BAD_PATH] {
        if limine::module_by_path(path).is_none() {
            kprintln!("user: ELF self-test skipped (`{}` module missing)", path);
            sched::set_preemption(false);
            return Ok(idle_report());
        }
    }

    // The corrupt-module probe runs before anything is allocated: a rejection
    // here proves the parser gate, an acceptance is a boot-fatal bug.
    let bad = match limine::module_by_path(BAD_PATH) {
        Some(module) => module,
        None => return Err("bad.elf module vanished after the presence check"),
    };
    match loader::inspect(bad) {
        Ok(_) => return Err("corrupt module bad.elf was accepted"),
        Err(reject) => kprintln!("user: /boot/bad.elf rejected ({})", reject.label()),
    }

    sched::spawn("user-hello", thread_hello)?;
    sched::wait_all_finished(300, tsc_frequency)?;
    vm::activate_kernel().map_err(|_| "returning to the kernel address space failed")?;
    cleanup_slot()?;
    if syscall::LAST_EXIT_CODE.load(Ordering::SeqCst) != 0 {
        return Err("hello.elf did not exit with code 0");
    }
    if syscall::WRITE_CALLS.load(Ordering::SeqCst) != 1
        || syscall::BYTES_WRITTEN.load(Ordering::SeqCst) == 0
    {
        return Err("hello.elf did not perform exactly one console write");
    }
    let faults_after_hello = syscall::USER_FAULTS.load(Ordering::SeqCst);

    sched::spawn("user-fault", thread_fault)?;
    sched::wait_all_finished(300, tsc_frequency)?;
    vm::activate_kernel().map_err(|_| "returning to the kernel address space failed")?;
    cleanup_slot()?;
    if syscall::USER_FAULTS.load(Ordering::SeqCst) != faults_after_hello + 1 {
        return Err("fault.elf did not produce exactly one additional isolated user fault");
    }

    // Phase 7, stage 1: the PE32+ fixture phase. Missing PE modules skip the
    // phase (report.pe = false) without failing boot; a corrupt module that
    // is accepted is a boot-fatal bug.
    for path in [PEHELLO_PATH, PEBAD_PATH] {
        if limine::module_by_path(path).is_none() {
            kprintln!("user: PE self-test skipped (`{}` module missing)", path);
            sched::set_preemption(false);
            return Ok(DemoReport {
                elf: true,
                ..idle_report()
            });
        }
    }

    let Some(bad_pe) = limine::module_by_path(PEBAD_PATH) else {
        return Err("pebad.exe module vanished after the presence check");
    };
    match pe_loader::inspect(bad_pe) {
        Ok(_) => return Err("corrupt module pebad.exe was accepted"),
        Err(reject) => kprintln!("user: /boot/pebad.exe rejected ({})", reject.label()),
    }

    let writes_before = syscall::WRITE_CALLS.load(Ordering::SeqCst);
    let bytes_before = syscall::BYTES_WRITTEN.load(Ordering::SeqCst);
    let faults_before_pe = syscall::USER_FAULTS.load(Ordering::SeqCst);

    sched::spawn("user-pe", thread_pehello)?;
    sched::wait_all_finished(300, tsc_frequency)?;
    vm::activate_kernel().map_err(|_| "returning to the kernel address space failed")?;
    cleanup_slot()?;
    if syscall::LAST_EXIT_CODE.load(Ordering::SeqCst) != 0 {
        return Err("pehello.exe did not exit with code 0");
    }
    if syscall::WRITE_CALLS.load(Ordering::SeqCst) != writes_before + 2 {
        return Err("pehello.exe did not perform exactly two console writes");
    }
    if syscall::BYTES_WRITTEN.load(Ordering::SeqCst) <= bytes_before {
        return Err("pehello.exe wrote no bytes");
    }
    if syscall::USER_FAULTS.load(Ordering::SeqCst) != faults_before_pe {
        return Err("pehello.exe produced a user fault");
    }

    sched::set_preemption(false);

    let report = DemoReport {
        elf: true,
        pe: true,
        ..idle_report()
    };
    if report.rejected_syscalls != 0 {
        return Err("a syscall was rejected during the demo");
    }
    Ok(report)
}
