//! Ring-3 user-mode demonstration and self-test (Phase 6, stage 3; PE32+
//! phase 7, stages 1–3; Phase 8, stage 2).
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
//! * `peconsole.exe` (stage 2) runs at CPL3 and calls `GetStdHandle`,
//!   `WriteConsoleA` and `ExitProcess` through a real `rust-lld`-produced
//!   import table, dispatched by the kernel-owned stub page.
//!   `pebadimp.exe` (unknown DLL) and `pebadfn.exe` (unknown function) must
//!   be rejected by the import allowlist before any page is allocated.
//! * `pebasics.exe` (stage 3) calls the eight new `kernel32.dll` functions
//!   plus `GetStdHandle` — including the five-argument `WriteFile` and the
//!   per-thread `GetLastError`/`SetLastError` pair — and ends through
//!   `ExitThread`. `pebadfn2.exe` (unknown function) must be rejected the
//!   same way as `pebadfn.exe`.
//! * `peinput.exe` (Phase 8, stage 2) calls `GetStdHandle(STD_INPUT_HANDLE)`,
//!   blocks in a line-mode `ReadFile` until the harness types a line, checks
//!   the documented failure values (`0` + last-error 6 for a bad handle,
//!   `0` + 87 for an overlapped request), plays the line back with `WriteFile`
//!   and exits through `ExitThread`. It exists only on the dedicated input
//!   ISO (`make_iso.sh --input-fixture`); without the module the phase is
//!   skipped with an explicit line.
//! * `pereg.exe` (Phase 8, stage 3) imports `RegOpenKeyExA`,
//!   `RegQueryValueExA` and `RegCloseKey` from `advapi32.dll` — the first
//!   second-DLL allowlist entries and the first six-argument stub — and
//!   exercises the read-only registry view over the config store: handle
//!   validation, size queries, `ERROR_MORE_DATA`, REG_SZ reads, slot
//!   exhaustion/reuse and double close. It exists only on the dedicated
//!   registry ISO (`make_iso.sh --registry-fixture`); without the module the
//!   phase is skipped with an explicit line.
//! * `peregw.exe` (Phase 8, stage 4) imports `RegCreateKeyExA` (the first
//!   nine-argument stub), `RegSetValueExA` and `RegDeleteValueA` from
//!   `advapi32.dll` and exercises the registry write surface over the config
//!   store: create/open dispositions, REG_SZ sets and updates, deletes,
//!   documented error values and persistence through `/etc/config`. It
//!   exists only on the dedicated registry-write ISO
//!   (`make_iso.sh --registry-write-fixture`); without the module the phase
//!   is skipped with an explicit line.
//!
//! Each program runs one at a time on its own scheduler thread. The thread
//! publishes its address space and load plan into a static slot; after the
//! thread dies the boot context switches CR3 back to the kernel template,
//! unmaps every page of the load plan (including the import stub page) plus
//! the stack, destroys the space and resets the registry handle table.

use core::arch::global_asm;
use core::sync::atomic::Ordering;

use os_core::frame::PhysFrame;
use os_core::paging::{PAGE_SIZE, USER_DATA};

use crate::arch::x86_64::gdt::{USER_CODE_SELECTOR, USER_DATA_SELECTOR, set_tss_rsp0};
use crate::config;
use crate::kprintln;
use crate::limine;
use crate::loader;
use crate::memory;
use crate::pe_loader;
use crate::registry;
use crate::sched;
use crate::serial;
use crate::sync::IrqSpinLock;
use crate::syscall;
use crate::vfs;
use crate::vm::{self, AddressSpace};

const HELLO_PATH: &str = "/boot/hello.elf";
const FAULT_PATH: &str = "/boot/fault.elf";
const BAD_PATH: &str = "/boot/bad.elf";
const PEHELLO_PATH: &str = "/boot/pehello.exe";
const PEBAD_PATH: &str = "/boot/pebad.exe";
const PECONSOLE_PATH: &str = "/boot/peconsole.exe";
const PEBADIMP_PATH: &str = "/boot/pebadimp.exe";
const PEBADFN_PATH: &str = "/boot/pebadfn.exe";
const PEBASICS_PATH: &str = "/boot/pebasics.exe";
const PEBADFN2_PATH: &str = "/boot/pebadfn2.exe";
const PEINPUT_PATH: &str = "/boot/peinput.exe";
const PEREG_PATH: &str = "/boot/pereg.exe";
const PEREGW_PATH: &str = "/boot/peregw.exe";

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
    Pe(pe_loader::Loaded),
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
    /// False when the Win32 console self-test was skipped.
    pub win32: bool,
    /// False when the Win32 program-basics self-test was skipped.
    pub win32_program: bool,
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
/// relocations, resolve imports), map the zeroed stack, switch CR3, program
/// the TSS stack and enter ring 3 at the PE entry. Returns only through the
/// kernel on failure.
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
    let loaded = match pe_loader::map(&mut space, module, &image) {
        Ok(loaded) => loaded,
        Err(reject) => {
            let _ = space.destroy();
            thread_fatal(format_args!("cannot load {}: {}", path, reject.label()));
        }
    };
    let entry = loader::CODE_BASE + u64::from(loaded.image.entry_rva());
    kprintln!(
        "pe: {} loaded at {:#x} (entry {:#x}, {} sections, {} relocation{} applied)",
        path.rsplit('/').next().unwrap_or(path),
        loader::CODE_BASE,
        entry,
        loaded.image.sections().len(),
        loaded.applied_relocations,
        if loaded.applied_relocations == 1 {
            ""
        } else {
            "s"
        }
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
    *USER_SLOT.lock() = Some((space, LoadedPlan::Pe(loaded)));

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

fn thread_peconsole() {
    run_pe(PECONSOLE_PATH);
}

fn thread_pebasics() {
    run_pe(PEBASICS_PATH);
}

fn thread_peinput() {
    run_pe(PEINPUT_PATH);
}

fn thread_pereg() {
    run_pe(PEREG_PATH);
}

fn thread_peregw() {
    run_pe(PEREGW_PATH);
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
    // Key handles die with the program that opened them.
    registry::reset();
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
        LoadedPlan::Pe(loaded) => {
            for section in loaded.image.sections() {
                let range = pe_loader::section_page_range(section);
                let mut page = range.start;
                while page < range.end {
                    unmap_and_free(&mut space, page)?;
                    page += PAGE_SIZE;
                }
            }
            if loaded.stub_page != 0 {
                unmap_and_free(&mut space, loaded.stub_page)?;
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
        win32: false,
        win32_program: false,
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
pub fn demo() -> Result<DemoReport, &'static str> {
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
    sched::wait_all_finished(300)?;
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
    sched::wait_all_finished(300)?;
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
    sched::wait_all_finished(300)?;
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

    // Phase 7, stage 2: the native Win32 console surface. The negative
    // fixtures are probed first — a rejection here proves the import
    // allowlist before any page is allocated — then `peconsole.exe` runs at
    // CPL3 and reaches our implementations through resolved `kernel32.dll`
    // imports.
    for path in [PECONSOLE_PATH, PEBADIMP_PATH, PEBADFN_PATH] {
        if limine::module_by_path(path).is_none() {
            kprintln!("user: Win32 self-test skipped (`{}` module missing)", path);
            kprintln!("win32 selftest: skipped (no Win32 fixtures present)");
            sched::set_preemption(false);
            return Ok(DemoReport {
                elf: true,
                pe: true,
                ..idle_report()
            });
        }
    }

    let Some(bad_import) = limine::module_by_path(PEBADIMP_PATH) else {
        return Err("pebadimp.exe module vanished after the presence check");
    };
    match pe_loader::inspect(bad_import) {
        Ok(_) => return Err("module pebadimp.exe was accepted despite an unknown import dll"),
        Err(pe_loader::Reject::UnknownImportDll) => {
            kprintln!("user: /boot/pebadimp.exe rejected (unknown import dll)");
        }
        Err(_) => return Err("module pebadimp.exe was rejected for an unexpected reason"),
    }

    let Some(bad_function) = limine::module_by_path(PEBADFN_PATH) else {
        return Err("pebadfn.exe module vanished after the presence check");
    };
    match pe_loader::inspect(bad_function) {
        Ok(_) => return Err("module pebadfn.exe was accepted despite an unknown import function"),
        Err(pe_loader::Reject::UnknownImportFunction) => {
            kprintln!("user: /boot/pebadfn.exe rejected (unknown import function)");
        }
        Err(_) => return Err("module pebadfn.exe was rejected for an unexpected reason"),
    }

    let writes_before_win32 = syscall::WRITE_CALLS.load(Ordering::SeqCst);
    let bytes_before_win32 = syscall::BYTES_WRITTEN.load(Ordering::SeqCst);
    let faults_before_win32 = syscall::USER_FAULTS.load(Ordering::SeqCst);
    let get_before_win32 = syscall::WIN32_GET_STD_HANDLE_CALLS.load(Ordering::SeqCst);
    let console_before_win32 = syscall::WIN32_WRITE_CONSOLE_CALLS.load(Ordering::SeqCst);
    let exit_before_win32 = syscall::WIN32_EXIT_PROCESS_CALLS.load(Ordering::SeqCst);

    sched::spawn("user-win32", thread_peconsole)?;
    sched::wait_all_finished(300)?;
    vm::activate_kernel().map_err(|_| "returning to the kernel address space failed")?;
    cleanup_slot()?;
    if syscall::LAST_EXIT_CODE.load(Ordering::SeqCst) != 0 {
        return Err("peconsole.exe did not exit with code 0");
    }
    if syscall::WIN32_GET_STD_HANDLE_CALLS.load(Ordering::SeqCst) != get_before_win32 + 1 {
        return Err("peconsole.exe did not call GetStdHandle exactly once");
    }
    if syscall::WIN32_WRITE_CONSOLE_CALLS.load(Ordering::SeqCst) != console_before_win32 + 2 {
        return Err("peconsole.exe did not call WriteConsoleA exactly twice");
    }
    if syscall::WIN32_EXIT_PROCESS_CALLS.load(Ordering::SeqCst) != exit_before_win32 + 1 {
        return Err("peconsole.exe did not call ExitProcess exactly once");
    }
    if syscall::WRITE_CALLS.load(Ordering::SeqCst) != writes_before_win32 + 2 {
        return Err("peconsole.exe did not perform exactly two console writes");
    }
    if syscall::BYTES_WRITTEN.load(Ordering::SeqCst) != bytes_before_win32 + 76 {
        return Err("peconsole.exe did not write exactly 76 bytes");
    }
    if syscall::USER_FAULTS.load(Ordering::SeqCst) != faults_before_win32 {
        return Err("peconsole.exe produced a user fault");
    }

    // Phase 7, stage 3: the Win32 program basics. `pebadfn2.exe` is probed
    // first — a rejection proves the allowlist before any allocation — then
    // `pebasics.exe` runs at CPL3 and exercises the five-argument WriteFile,
    // the per-thread last-error pair, identity/tick queries, the command
    // line and ExitThread.
    for path in [PEBASICS_PATH, PEBADFN2_PATH] {
        if limine::module_by_path(path).is_none() {
            kprintln!(
                "user: Win32 program self-test skipped (`{}` module missing)",
                path
            );
            kprintln!("win32 program selftest: skipped (no program fixtures present)");
            sched::set_preemption(false);
            return Ok(DemoReport {
                elf: true,
                pe: true,
                win32: true,
                ..idle_report()
            });
        }
    }

    let Some(bad_function2) = limine::module_by_path(PEBADFN2_PATH) else {
        return Err("pebadfn2.exe module vanished after the presence check");
    };
    match pe_loader::inspect(bad_function2) {
        Ok(_) => return Err("module pebadfn2.exe was accepted despite an unknown import function"),
        Err(pe_loader::Reject::UnknownImportFunction) => {
            kprintln!("user: /boot/pebadfn2.exe rejected (unknown import function)");
        }
        Err(_) => return Err("module pebadfn2.exe was rejected for an unexpected reason"),
    }

    let writes_before_program = syscall::WRITE_CALLS.load(Ordering::SeqCst);
    let bytes_before_program = syscall::BYTES_WRITTEN.load(Ordering::SeqCst);
    let faults_before_program = syscall::USER_FAULTS.load(Ordering::SeqCst);
    let get_before_program = syscall::WIN32_GET_STD_HANDLE_CALLS.load(Ordering::SeqCst);
    let console_before_program = syscall::WIN32_WRITE_CONSOLE_CALLS.load(Ordering::SeqCst);
    let exit_before_program = syscall::WIN32_EXIT_PROCESS_CALLS.load(Ordering::SeqCst);
    let writefile_before_program = syscall::WIN32_WRITEFILE_CALLS.load(Ordering::SeqCst);
    let last_error_before_program = syscall::WIN32_GET_LAST_ERROR_CALLS.load(Ordering::SeqCst);
    let set_last_error_before_program = syscall::WIN32_SET_LAST_ERROR_CALLS.load(Ordering::SeqCst);
    let exit_thread_before_program = syscall::WIN32_EXIT_THREAD_CALLS.load(Ordering::SeqCst);
    let process_id_before_program =
        syscall::WIN32_GET_CURRENT_PROCESS_ID_CALLS.load(Ordering::SeqCst);
    let thread_id_before_program =
        syscall::WIN32_GET_CURRENT_THREAD_ID_CALLS.load(Ordering::SeqCst);
    let tick_before_program = syscall::WIN32_GET_TICK_COUNT_CALLS.load(Ordering::SeqCst);
    let command_line_before_program = syscall::WIN32_GET_COMMAND_LINE_CALLS.load(Ordering::SeqCst);

    sched::spawn("user-win32-prog", thread_pebasics)?;
    sched::wait_all_finished(300)?;
    vm::activate_kernel().map_err(|_| "returning to the kernel address space failed")?;
    cleanup_slot()?;
    if syscall::LAST_EXIT_CODE.load(Ordering::SeqCst) != 0 {
        return Err("pebasics.exe did not exit with code 0");
    }
    if syscall::WIN32_GET_STD_HANDLE_CALLS.load(Ordering::SeqCst) != get_before_program + 1 {
        return Err("pebasics.exe did not call GetStdHandle exactly once");
    }
    if syscall::WIN32_WRITE_CONSOLE_CALLS.load(Ordering::SeqCst) != console_before_program {
        return Err("pebasics.exe called WriteConsoleA");
    }
    if syscall::WIN32_EXIT_PROCESS_CALLS.load(Ordering::SeqCst) != exit_before_program {
        return Err("pebasics.exe called ExitProcess");
    }
    if syscall::WIN32_WRITEFILE_CALLS.load(Ordering::SeqCst) != writefile_before_program + 4 {
        return Err("pebasics.exe did not call WriteFile exactly four times");
    }
    if syscall::WIN32_GET_LAST_ERROR_CALLS.load(Ordering::SeqCst) != last_error_before_program + 5 {
        return Err("pebasics.exe did not call GetLastError exactly five times");
    }
    if syscall::WIN32_SET_LAST_ERROR_CALLS.load(Ordering::SeqCst)
        != set_last_error_before_program + 2
    {
        return Err("pebasics.exe did not call SetLastError exactly twice");
    }
    if syscall::WIN32_EXIT_THREAD_CALLS.load(Ordering::SeqCst) != exit_thread_before_program + 1 {
        return Err("pebasics.exe did not call ExitThread exactly once");
    }
    if syscall::WIN32_GET_CURRENT_PROCESS_ID_CALLS.load(Ordering::SeqCst)
        != process_id_before_program + 1
    {
        return Err("pebasics.exe did not call GetCurrentProcessId exactly once");
    }
    if syscall::WIN32_GET_CURRENT_THREAD_ID_CALLS.load(Ordering::SeqCst)
        != thread_id_before_program + 1
    {
        return Err("pebasics.exe did not call GetCurrentThreadId exactly once");
    }
    if syscall::WIN32_GET_TICK_COUNT_CALLS.load(Ordering::SeqCst) != tick_before_program + 2 {
        return Err("pebasics.exe did not call GetTickCount exactly twice");
    }
    if syscall::WIN32_GET_COMMAND_LINE_CALLS.load(Ordering::SeqCst)
        != command_line_before_program + 1
    {
        return Err("pebasics.exe did not call GetCommandLineA exactly once");
    }
    if syscall::WRITE_CALLS.load(Ordering::SeqCst) != writes_before_program + 2 {
        return Err("pebasics.exe did not perform exactly two console writes");
    }
    if syscall::BYTES_WRITTEN.load(Ordering::SeqCst) != bytes_before_program + 59 {
        return Err("pebasics.exe did not write exactly 59 bytes");
    }
    if syscall::USER_FAULTS.load(Ordering::SeqCst) != faults_before_program {
        return Err("pebasics.exe produced a user fault");
    }

    // Phase 8, stage 2: the Win32 console-input surface. `peinput.exe` is
    // only present on the dedicated input ISO, so a missing module skips the
    // phase like the stages above and every default-boot contract stays
    // unchanged.
    if limine::module_by_path(PEINPUT_PATH).is_some() {
        let get_before_input = syscall::WIN32_GET_STD_HANDLE_CALLS.load(Ordering::SeqCst);
        let readfile_before_input = syscall::WIN32_READFILE_CALLS.load(Ordering::SeqCst);
        let last_error_before_input = syscall::WIN32_GET_LAST_ERROR_CALLS.load(Ordering::SeqCst);
        let writefile_before_input = syscall::WIN32_WRITEFILE_CALLS.load(Ordering::SeqCst);
        let faults_before_input = syscall::USER_FAULTS.load(Ordering::SeqCst);

        // The fixture blocks in ReadFile until the harness types a line, so
        // this phase gets a longer window than the compute-only phases.
        sched::spawn("user-win32-input", thread_peinput)?;
        sched::wait_all_finished(3000)?;
        vm::activate_kernel().map_err(|_| "returning to the kernel address space failed")?;
        cleanup_slot()?;
        if syscall::LAST_EXIT_CODE.load(Ordering::SeqCst) != 0 {
            return Err("peinput.exe did not exit with code 0");
        }
        if syscall::WIN32_GET_STD_HANDLE_CALLS.load(Ordering::SeqCst) != get_before_input + 1 {
            return Err("peinput.exe did not call GetStdHandle exactly once");
        }
        if syscall::WIN32_READFILE_CALLS.load(Ordering::SeqCst) != readfile_before_input + 3 {
            return Err("peinput.exe did not call ReadFile exactly three times");
        }
        if syscall::WIN32_GET_LAST_ERROR_CALLS.load(Ordering::SeqCst) != last_error_before_input + 2
        {
            return Err("peinput.exe did not call GetLastError exactly twice");
        }
        if syscall::WIN32_WRITEFILE_CALLS.load(Ordering::SeqCst) != writefile_before_input + 2 {
            return Err("peinput.exe did not call WriteFile exactly twice");
        }
        if syscall::USER_FAULTS.load(Ordering::SeqCst) != faults_before_input {
            return Err("peinput.exe produced a user fault");
        }
        kprintln!(
            "win32 input selftest: passed (ReadFile line input, echo, byte count, invalid handle/parameter errors verified through kernel32.dll imports)"
        );
    } else {
        kprintln!(
            "user: Win32 input self-test skipped (`{}` module missing)",
            PEINPUT_PATH
        );
        kprintln!("win32 input selftest: skipped (no input fixture present)");
    }

    // Phase 8, stage 3: the read-only Win32 registry surface. `pereg.exe` is
    // only present on the dedicated registry ISO, so a missing module skips
    // the phase like the stages above and every default-boot contract stays
    // unchanged.
    if limine::module_by_path(PEREG_PATH).is_some() {
        let open_before_registry = syscall::WIN32_REG_OPEN_KEY_CALLS.load(Ordering::SeqCst);
        let query_before_registry = syscall::WIN32_REG_QUERY_VALUE_CALLS.load(Ordering::SeqCst);
        let close_before_registry = syscall::WIN32_REG_CLOSE_KEY_CALLS.load(Ordering::SeqCst);
        let writefile_before_registry = syscall::WIN32_WRITEFILE_CALLS.load(Ordering::SeqCst);
        let writes_before_registry = syscall::WRITE_CALLS.load(Ordering::SeqCst);
        let bytes_before_registry = syscall::BYTES_WRITTEN.load(Ordering::SeqCst);
        let faults_before_registry = syscall::USER_FAULTS.load(Ordering::SeqCst);

        sched::spawn("user-win32-registry", thread_pereg)?;
        sched::wait_all_finished(300)?;
        vm::activate_kernel().map_err(|_| "returning to the kernel address space failed")?;
        cleanup_slot()?;
        if syscall::LAST_EXIT_CODE.load(Ordering::SeqCst) != 0 {
            return Err("pereg.exe did not exit with code 0");
        }
        if syscall::WIN32_REG_OPEN_KEY_CALLS.load(Ordering::SeqCst) != open_before_registry + 10 {
            return Err("pereg.exe did not call RegOpenKeyExA exactly ten times");
        }
        if syscall::WIN32_REG_QUERY_VALUE_CALLS.load(Ordering::SeqCst) != query_before_registry + 7
        {
            return Err("pereg.exe did not call RegQueryValueExA exactly seven times");
        }
        if syscall::WIN32_REG_CLOSE_KEY_CALLS.load(Ordering::SeqCst) != close_before_registry + 7 {
            return Err("pereg.exe did not call RegCloseKey exactly seven times");
        }
        if syscall::WIN32_WRITEFILE_CALLS.load(Ordering::SeqCst) != writefile_before_registry + 1 {
            return Err("pereg.exe did not call WriteFile exactly once");
        }
        if syscall::WRITE_CALLS.load(Ordering::SeqCst) != writes_before_registry + 1 {
            return Err("pereg.exe did not perform exactly one console write");
        }
        if syscall::BYTES_WRITTEN.load(Ordering::SeqCst) != bytes_before_registry + 33 {
            return Err("pereg.exe did not write exactly 33 bytes");
        }
        if syscall::USER_FAULTS.load(Ordering::SeqCst) != faults_before_registry {
            return Err("pereg.exe produced a user fault");
        }
        kprintln!(
            "win32 registry selftest: passed (advapi32.dll RegOpenKeyExA/RegQueryValueExA/RegCloseKey over the config store; handle validation, size queries, ERROR_MORE_DATA, REG_SZ reads, slot exhaustion/reuse and double close verified)"
        );
    } else {
        kprintln!(
            "user: Win32 registry self-test skipped (`{}` module missing)",
            PEREG_PATH
        );
        kprintln!("win32 registry selftest: skipped (no registry fixture present)");
    }

    // Phase 8, stage 4: the Win32 registry write surface. `peregw.exe` is
    // only present on the dedicated registry-write ISO, so a missing module
    // skips the phase like the stages above and every earlier contract stays
    // unchanged.
    if limine::module_by_path(PEREGW_PATH).is_some() {
        let create_before_rw = syscall::WIN32_REG_CREATE_KEY_CALLS.load(Ordering::SeqCst);
        let set_before_rw = syscall::WIN32_REG_SET_VALUE_CALLS.load(Ordering::SeqCst);
        let delete_before_rw = syscall::WIN32_REG_DELETE_VALUE_CALLS.load(Ordering::SeqCst);
        let query_before_rw = syscall::WIN32_REG_QUERY_VALUE_CALLS.load(Ordering::SeqCst);
        let close_before_rw = syscall::WIN32_REG_CLOSE_KEY_CALLS.load(Ordering::SeqCst);
        let writefile_before_rw = syscall::WIN32_WRITEFILE_CALLS.load(Ordering::SeqCst);
        let writes_before_rw = syscall::WRITE_CALLS.load(Ordering::SeqCst);
        let bytes_before_rw = syscall::BYTES_WRITTEN.load(Ordering::SeqCst);
        let faults_before_rw = syscall::USER_FAULTS.load(Ordering::SeqCst);

        sched::spawn("user-win32-registry-write", thread_peregw)?;
        sched::wait_all_finished(300)?;
        vm::activate_kernel().map_err(|_| "returning to the kernel address space failed")?;
        cleanup_slot()?;
        if syscall::LAST_EXIT_CODE.load(Ordering::SeqCst) != 0 {
            return Err("peregw.exe did not exit with code 0");
        }
        if syscall::WIN32_REG_CREATE_KEY_CALLS.load(Ordering::SeqCst) != create_before_rw + 3 {
            return Err("peregw.exe did not call RegCreateKeyExA exactly three times");
        }
        if syscall::WIN32_REG_SET_VALUE_CALLS.load(Ordering::SeqCst) != set_before_rw + 8 {
            return Err("peregw.exe did not call RegSetValueExA exactly eight times");
        }
        if syscall::WIN32_REG_DELETE_VALUE_CALLS.load(Ordering::SeqCst) != delete_before_rw + 3 {
            return Err("peregw.exe did not call RegDeleteValueA exactly three times");
        }
        if syscall::WIN32_REG_QUERY_VALUE_CALLS.load(Ordering::SeqCst) != query_before_rw + 3 {
            return Err("peregw.exe did not call RegQueryValueExA exactly three times");
        }
        if syscall::WIN32_REG_CLOSE_KEY_CALLS.load(Ordering::SeqCst) != close_before_rw + 3 {
            return Err("peregw.exe did not call RegCloseKey exactly three times");
        }
        if syscall::WIN32_WRITEFILE_CALLS.load(Ordering::SeqCst) != writefile_before_rw + 1 {
            return Err("peregw.exe did not call WriteFile exactly once");
        }
        if syscall::WRITE_CALLS.load(Ordering::SeqCst) != writes_before_rw + 1 {
            return Err("peregw.exe did not perform exactly one console write");
        }
        if syscall::BYTES_WRITTEN.load(Ordering::SeqCst) != bytes_before_rw + 39 {
            return Err("peregw.exe did not write exactly 39 bytes");
        }
        if syscall::USER_FAULTS.load(Ordering::SeqCst) != faults_before_rw {
            return Err("peregw.exe produced a user fault");
        }

        // The fixture's mutation must still be in the store, must survive a
        // save/read-back/decode round trip through `/etc/config`, and is then
        // removed so the store returns to the three seeded defaults. The
        // file itself stays written with the defaults (no disk flush happens
        // without shell activity; documented stage-4 behavior).
        const PERSISTED: &str = "Session/Current/Persisted";
        match config::get(PERSISTED) {
            Ok(Some(value)) if value == "yes" => {}
            _ => return Err("peregw.exe did not persist the registry value"),
        }
        config::save()?;
        let read = vfs::with(|vfs| vfs.read_file(config::CONFIG_PATH).map(|data| data.to_vec()))
            .map_err(|_| "reading /etc/config failed")?;
        let blob = read.map_err(|_| "reading /etc/config failed")?;
        let decoded = os_core::config::Config::decode(&blob)
            .map_err(|_| "the persisted /etc/config did not decode")?;
        if decoded.get(PERSISTED) != Some("yes") {
            return Err("the persisted /etc/config lost the registry value");
        }
        config::remove(PERSISTED)?;
        config::save()?;
        if !matches!(config::get(PERSISTED), Ok(None)) {
            return Err("restoring the store after the registry write failed");
        }
        kprintln!(
            "win32 registry write selftest: passed (advapi32.dll RegCreateKeyExA/RegSetValueExA/RegDeleteValueA over the config store; create/open dispositions, REG_SZ sets and updates, deletes, error values 2/6/87, persistence through /etc/config verified)"
        );
    } else {
        kprintln!(
            "user: Win32 registry write self-test skipped (`{}` module missing)",
            PEREGW_PATH
        );
        kprintln!("win32 registry write selftest: skipped (no registry write fixture present)");
    }

    sched::set_preemption(false);

    let report = DemoReport {
        elf: true,
        pe: true,
        win32: true,
        win32_program: true,
        ..idle_report()
    };
    if report.rejected_syscalls != 0 {
        return Err("a syscall was rejected during the demo");
    }
    Ok(report)
}
