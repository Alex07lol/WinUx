//! Ring-3 user-mode demonstration and self-test (Phase 5b).
//!
//! The test programs are assembled into the kernel image and copied at boot
//! into a fresh 4-level address space whose kernel half is the boot template:
//!
//! * Program A runs at CPL3, performs one `int 0x80` console write and exits
//!   through `SYS_EXIT` with code 0.
//! * Program B runs at CPL3 and writes to an unmapped user address, which
//!   raises a page fault from CPL3. The kernel reports it, kills the thread,
//!   and continues boot — the fault-isolation property.
//!
//! Each program runs on its own scheduler thread; the boot thread joins them,
//! then switches CR3 back to the kernel template and tears the test address
//! spaces down (unmapping and freeing their frames and page tables).

use core::arch::global_asm;
use core::sync::atomic::Ordering;

use os_core::frame::PhysFrame;
use os_core::paging::{USER_CODE, USER_DATA};

use crate::arch::x86_64::gdt::{USER_CODE_SELECTOR, USER_DATA_SELECTOR, set_tss_rsp0};
use crate::memory;
use crate::sched;
use crate::serial;
use crate::sync::IrqSpinLock;
use crate::syscall;
use crate::vm::{self, AddressSpace};

/// User program code window (1 MiB, away from the null guard page).
const CODE_VA: u64 = 0x40_0000;
/// Single user stack page.
const STACK_VA: u64 = 0x50_0000;
const STACK_TOP: u64 = STACK_VA + 0x1000;

global_asm!(
    r#"
.section .text
.p2align 4

.global project_os_user_program_a_start
.global project_os_user_program_a_end
project_os_user_program_a_start:
    lea rdx, [rip + program_a_message]
    lea r8, [rip + program_a_message_end]
    sub r8, rdx
    mov ecx, 1
    mov eax, 1
    int 0x80
    mov ecx, 0
    mov eax, 2
    int 0x80
    ud2
program_a_message:
    .ascii "user: hello from ring 3 via int 0x80\n"
program_a_message_end:
project_os_user_program_a_end:

.global project_os_user_program_b_start
.global project_os_user_program_b_end
project_os_user_program_b_start:
    movabs rax, 0x500000000
    mov qword ptr [rax], 0xdead
    ud2
project_os_user_program_b_end:

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
    static project_os_user_program_a_start: u8;
    static project_os_user_program_a_end: u8;
    static project_os_user_program_b_start: u8;
    static project_os_user_program_b_end: u8;
    fn project_os_enter_user_mode(
        entry: u64,
        stack_top: u64,
        code_selector: u64,
        data_selector: u64,
    ) -> !;
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Program {
    A,
    B,
}

static SPACE_A: IrqSpinLock<Option<AddressSpace>> = IrqSpinLock::new(None);
static SPACE_B: IrqSpinLock<Option<AddressSpace>> = IrqSpinLock::new(None);

#[derive(Clone, Copy)]
pub struct DemoReport {
    pub exit_code: u64,
    pub write_calls: u64,
    pub bytes_written: u64,
    pub user_faults: u64,
    pub rejected_syscalls: u64,
}

fn program_bytes(program: Program) -> &'static [u8] {
    let (start, end) = match program {
        Program::A => (
            core::ptr::addr_of!(project_os_user_program_a_start),
            core::ptr::addr_of!(project_os_user_program_a_end),
        ),
        Program::B => (
            core::ptr::addr_of!(project_os_user_program_b_start),
            core::ptr::addr_of!(project_os_user_program_b_end),
        ),
    };
    let length = end as usize - start as usize;
    // SAFETY: both labels are emitted into the kernel text section; the slice
    // is a subrange of that readonly section.
    unsafe { core::slice::from_raw_parts(start, length) }
}

fn thread_fatal(message: &str) -> ! {
    serial::force_print(format_args!(
        "PROJECT_OS_FATAL: user setup: {}\r\n",
        message
    ));
    sched::exit()
}

/// Build the user address space, copy the program, switch CR3, program the
/// TSS stack and enter ring 3. Returns only through the kernel on failure.
fn run(program: Program) -> ! {
    let Some(hhdm) = vm::hhdm_offset() else {
        thread_fatal("HHDM offset is unknown");
    };
    let code = program_bytes(program);
    let mut space = match AddressSpace::new_like_kernel(hhdm) {
        Ok(space) => space,
        Err(_) => thread_fatal("cannot create the user address space"),
    };
    let code_frame = match memory::alloc_frame() {
        Ok(frame) => frame,
        Err(_) => thread_fatal("no frame for the user program"),
    };
    let stack_frame = match memory::alloc_frame() {
        Ok(frame) => frame,
        Err(_) => thread_fatal("no frame for the user stack"),
    };

    // Copy the program into its frame through the HHDM.
    let code_window = match hhdm.checked_add(code_frame.address()) {
        Some(window) => window as *mut u8,
        None => thread_fatal("user code HHDM overflow"),
    };
    // SAFETY: the frame was just allocated for this copy and is not mapped
    // elsewhere; `code` is a kernel-text subrange.
    unsafe { core::ptr::copy_nonoverlapping(code.as_ptr(), code_window, code.len()) };

    if space
        .map_page(CODE_VA, code_frame.address(), USER_CODE)
        .is_err()
        || space
            .map_page(STACK_VA, stack_frame.address(), USER_DATA)
            .is_err()
    {
        thread_fatal("mapping the user program failed");
    }
    let kernel_stack_top = match sched::current_kernel_stack_top() {
        Some(top) => top,
        None => thread_fatal("thread has no kernel stack"),
    };

    // Enter the new address space, then publish it for teardown. The kernel
    // half is shared with the template, so kernel code (including the timer
    // ISR and any preemption) keeps working with the user CR3 loaded.
    if space.activate().is_err() {
        thread_fatal("activating the user address space failed");
    }
    match program {
        Program::A => *SPACE_A.lock() = Some(space),
        Program::B => *SPACE_B.lock() = Some(space),
    }

    set_tss_rsp0(kernel_stack_top);
    // SAFETY: CODE_VA and STACK_VA are mapped user-accessible pages in the
    // now-active address space, with the claimed entry and stack contents.
    unsafe {
        project_os_enter_user_mode(
            CODE_VA,
            STACK_TOP,
            u64::from(USER_CODE_SELECTOR),
            u64::from(USER_DATA_SELECTOR),
        )
    }
}

fn thread_a() {
    run(Program::A);
}

fn thread_b() {
    run(Program::B);
}

fn cleanup_slot(slot: &IrqSpinLock<Option<AddressSpace>>) -> Result<(), &'static str> {
    let Some(mut space) = slot.lock().take() else {
        return Ok(());
    };
    for virtual_address in [CODE_VA, STACK_VA] {
        if let Ok(physical) = space.unmap_page(virtual_address)
            && let Some(frame) = PhysFrame::from_address(physical)
        {
            memory::free_pages(frame, 1).map_err(|_| "freeing a user frame failed")?;
        }
    }
    space
        .destroy()
        .map_err(|_| "destroying a user space failed")
}

/// Boot-time demonstration. The scheduler must already be initialised.
pub fn demo(tsc_frequency: Option<u64>) -> Result<DemoReport, &'static str> {
    sched::set_preemption(true);

    sched::spawn("user-a", thread_a)?;
    sched::wait_all_finished(300, tsc_frequency)?;
    sched::spawn("user-b", thread_b)?;
    sched::wait_all_finished(300, tsc_frequency)?;

    sched::set_preemption(false);

    // Back to the kernel address space, then tear the user spaces down.
    vm::activate_kernel().map_err(|_| "returning to the kernel address space failed")?;
    cleanup_slot(&SPACE_A)?;
    cleanup_slot(&SPACE_B)?;

    let report = DemoReport {
        exit_code: syscall::LAST_EXIT_CODE.load(Ordering::SeqCst),
        write_calls: syscall::WRITE_CALLS.load(Ordering::SeqCst),
        bytes_written: syscall::BYTES_WRITTEN.load(Ordering::SeqCst),
        user_faults: syscall::USER_FAULTS.load(Ordering::SeqCst),
        rejected_syscalls: syscall::REJECTED_SYSCALLS.load(Ordering::SeqCst),
    };
    if report.exit_code != 0 {
        return Err("program A did not exit with code 0");
    }
    if report.write_calls != 1 || report.bytes_written == 0 {
        return Err("program A did not perform exactly one console write");
    }
    if report.user_faults != 1 {
        return Err("program B did not produce exactly one isolated user fault");
    }
    if report.rejected_syscalls != 0 {
        return Err("a syscall was rejected during the demo");
    }
    Ok(report)
}
