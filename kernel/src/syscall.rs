//! The ring-3 syscall boundary (`int 0x80`) and user-fault handling (Phase 5b;
//! Win32 console services Phase 7, stage 2).
//!
//! ABI (Windows-style argument registers chosen so the later PE compatibility
//! layer can reuse the same gate):
//!
//! * `rax` — service number
//! * `rcx`, `rdx`, `r8`, `r9` — arguments
//! * `rax` — return value / status after the call
//!
//! Services:
//!
//! * `1 write(rcx = fd, rdx = buffer, r8 = length)` → bytes written; only
//!   fd 1 (console) and lengths of at most [`MAX_WRITE`] are accepted.
//! * `2 exit(rcx = code)` — the calling thread ends; the value is recorded.
//! * `3 GetStdHandle(rcx = nStdHandle)` → `1` for `STD_OUTPUT_HANDLE`, else
//!   `INVALID_HANDLE_VALUE`; native `kernel32!GetStdHandle`.
//! * `4 WriteConsoleA(rcx = handle, rdx = buffer, r8 = length,
//!   r9 = written)` → `1`/`0`; native `kernel32!WriteConsoleA` (A-variant
//!   byte write, out-parameter updated).
//! * `5 ExitProcess(rcx = code)` — native `kernel32!ExitProcess`; ends the
//!   thread through the same scheduler-kill path as `2 exit`.
//!
//! Every user pointer is validated against the live page tables before the
//! kernel reads it (mapped, user-accessible, canonical, bounded span). A
//! violation, an unknown service, or a fault taken from CPL3 kills the calling
//! thread (`sched::exit`) and the kernel continues — that is the isolation
//! property this milestone proves.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::idt::InterruptFrame;
use crate::arch::x86_64::read_cr2;
use crate::sched;
use crate::serial;
use crate::vm;

pub const SYS_WRITE: u64 = 1;
pub const SYS_EXIT: u64 = 2;
pub const SYS_GET_STD_HANDLE: u64 = 3;
pub const SYS_WRITE_CONSOLE: u64 = 4;
pub const SYS_EXIT_PROCESS: u64 = 5;

/// Maximum accepted console write, small enough to bound the kernel copy.
pub const MAX_WRITE: u64 = 4096;

/// `STD_OUTPUT_HANDLE` is `((DWORD)-11)`; the one handle this stage serves.
const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5;
/// The fabricated console handle `GetStdHandle(2)` returns.
const CONSOLE_HANDLE: u64 = 1;

/// Highest address the kernel accepts from user space (canonical user half:
/// PML4 entries 0..256).
const USER_LIMIT: u64 = 0x0000_8000_0000_0000;

/// Last exit code passed to `SYS_EXIT` or `SYS_EXIT_PROCESS` (single user
/// thread in this milestone).
pub static LAST_EXIT_CODE: AtomicU64 = AtomicU64::new(u64::MAX);
/// Successful `SYS_WRITE` calls.
pub static WRITE_CALLS: AtomicU64 = AtomicU64::new(0);
/// Bytes accepted from user buffers.
pub static BYTES_WRITTEN: AtomicU64 = AtomicU64::new(0);
/// `SYS_GET_STD_HANDLE` calls (successful and invalid alike).
pub static WIN32_GET_STD_HANDLE_CALLS: AtomicU64 = AtomicU64::new(0);
/// Successful `SYS_WRITE_CONSOLE` calls.
pub static WIN32_WRITE_CONSOLE_CALLS: AtomicU64 = AtomicU64::new(0);
/// `SYS_EXIT_PROCESS` calls (each one ends the thread).
pub static WIN32_EXIT_PROCESS_CALLS: AtomicU64 = AtomicU64::new(0);
/// Faults taken from CPL3 and isolated (thread killed, kernel alive).
pub static USER_FAULTS: AtomicU64 = AtomicU64::new(0);
/// Repeated `int 0x80` calls made with an invalid pointer or bad service.
pub static REJECTED_SYSCALLS: AtomicU64 = AtomicU64::new(0);

/// Called from `exception_dispatch` for vector 0x80.
pub fn dispatch(frame: &mut InterruptFrame) {
    match frame.rax {
        SYS_WRITE => write(frame),
        SYS_EXIT => {
            let code = frame.rcx;
            LAST_EXIT_CODE.store(code, Ordering::SeqCst);
            serial::force_print(format_args!("syscall: thread exits with code {}\r\n", code));
            sched::exit();
        }
        SYS_GET_STD_HANDLE => get_std_handle(frame),
        SYS_WRITE_CONSOLE => write_console(frame),
        SYS_EXIT_PROCESS => exit_process(frame),
        _ => {
            REJECTED_SYSCALLS.fetch_add(1, Ordering::SeqCst);
            serial::force_print(format_args!(
                "PROJECT_OS_FATAL: user thread killed for unknown syscall {}\r\n",
                frame.rax
            ));
            sched::exit();
        }
    }
}

/// `3 GetStdHandle`: no validation of the value happens here (per the API
/// docs), so only the output handle is recognized and anything else returns
/// `INVALID_HANDLE_VALUE` without killing the thread.
fn get_std_handle(frame: &mut InterruptFrame) {
    WIN32_GET_STD_HANDLE_CALLS.fetch_add(1, Ordering::SeqCst);
    if frame.rcx as u32 == STD_OUTPUT_HANDLE {
        serial::force_print(format_args!("win32: GetStdHandle(-11) -> 1\r\n"));
        frame.rax = CONSOLE_HANDLE;
    } else {
        frame.rax = u64::MAX;
    }
}

/// `4 WriteConsoleA`: the native implementation behind the imported
/// `kernel32.dll` function. Nothing is written and no counter moves until
/// every check has passed.
fn write_console(frame: &mut InterruptFrame) {
    let handle = frame.rcx;
    let buffer = frame.rdx;
    let length = frame.r8;
    let written_pointer = frame.r9;
    if handle != CONSOLE_HANDLE || length == 0 || length > MAX_WRITE {
        kill(frame, "invalid WriteConsoleA arguments");
        return;
    }
    if !validate_user_span(frame, buffer, length) {
        return;
    }
    // The out-parameter must be a mapped user page with 4 bytes available.
    let Some(hhdm) = vm::hhdm_offset() else {
        kill(frame, "WriteConsoleA out-pointer is not mapped");
        return;
    };
    if written_pointer < 0x1000
        || written_pointer
            .checked_add(4)
            .is_none_or(|end| end > USER_LIMIT)
        || written_pointer % 4096 > 4092
    {
        kill(frame, "WriteConsoleA out-pointer is not mapped");
        return;
    }
    let Some((physical, flags)) = vm::translate_live(written_pointer) else {
        kill(frame, "WriteConsoleA out-pointer is not mapped");
        return;
    };
    if flags & os_core::paging::USER == 0 {
        kill(frame, "WriteConsoleA out-pointer is not mapped");
        return;
    }
    let Some(cell) = hhdm.checked_add(physical) else {
        kill(frame, "WriteConsoleA out-pointer is not mapped");
        return;
    };
    let cell = cell as *mut u32;

    // SAFETY: the buffer span was validated as mapped, user-accessible and
    // bounded, exactly like `SYS_WRITE`; the out-pointer was validated as a
    // mapped user page with 4 bytes available, and the kernel stores through
    // the HHDM so a read-only user mapping cannot fault it.
    unsafe {
        let bytes = core::slice::from_raw_parts(buffer as *const u8, length as usize);
        serial::force_write_bytes(bytes);
        core::ptr::write_volatile(cell, length as u32);
    }
    WIN32_WRITE_CONSOLE_CALLS.fetch_add(1, Ordering::SeqCst);
    WRITE_CALLS.fetch_add(1, Ordering::SeqCst);
    BYTES_WRITTEN.fetch_add(length, Ordering::SeqCst);
    serial::force_print(format_args!(
        "win32: WriteConsoleA(1, {} bytes) -> 1\r\n",
        length
    ));
    frame.rax = 1;
}

/// `5 ExitProcess`: record the exit code and end the thread through the same
/// scheduler-kill path as `2 exit`, deliberately not a second mechanism.
fn exit_process(frame: &mut InterruptFrame) -> ! {
    let code = frame.rcx as u32;
    LAST_EXIT_CODE.store(u64::from(code), Ordering::SeqCst);
    WIN32_EXIT_PROCESS_CALLS.fetch_add(1, Ordering::SeqCst);
    serial::force_print(format_args!("win32: ExitProcess({})\r\n", code));
    sched::exit()
}

fn write(frame: &mut InterruptFrame) {
    let fd = frame.rcx;
    let buffer = frame.rdx;
    let length = frame.r8;
    if fd != 1 || length == 0 || length > MAX_WRITE {
        kill(frame, "invalid write arguments");
        return;
    }
    if !validate_user_span(frame, buffer, length) {
        return;
    }

    // SAFETY: the span was validated above: in the user half, mapped, user
    // accessible, and bounded. The kernel reads it through the live CR3.
    let bytes = unsafe { core::slice::from_raw_parts(buffer as *const u8, length as usize) };
    serial::force_write_bytes(bytes);
    WRITE_CALLS.fetch_add(1, Ordering::SeqCst);
    BYTES_WRITTEN.fetch_add(length, Ordering::SeqCst);
    frame.rax = length;
}

/// Validate one user buffer span (canonical user half, at least one page in,
/// every touched page mapped user-accessible) or kill the thread. Shared by
/// `write` and `WriteConsoleA`.
fn validate_user_span(frame: &InterruptFrame, buffer: u64, length: u64) -> bool {
    let end = match buffer.checked_add(length) {
        Some(end) => end,
        None => {
            kill(frame, "write buffer overflows the address space");
            return false;
        }
    };
    if buffer < 0x1000 || end > USER_LIMIT {
        kill(frame, "write buffer outside the user address space");
        return false;
    }
    // Every page the buffer touches must be mapped user-accessible in the
    // live address space.
    let mut page = buffer & !0xfff;
    while page < end {
        match vm::translate_live(page) {
            Some((_, flags)) if flags & os_core::paging::USER != 0 => {}
            _ => {
                kill(frame, "write buffer is not mapped for user access");
                return false;
            }
        }
        page += 4096;
    }
    true
}

fn kill(frame: &InterruptFrame, reason: &'static str) {
    REJECTED_SYSCALLS.fetch_add(1, Ordering::SeqCst);
    serial::force_print(format_args!(
        "PROJECT_OS_FATAL: user thread killed: {} (rip={:#x}, rax={}, rcx={:#x}, rdx={:#x}, r8={})\r\n",
        reason, frame.rip, frame.rax, frame.rcx, frame.rdx, frame.r8
    ));
    sched::exit();
}

/// A fault (page fault, #GP, ...) taken from CPL3. Reports the fault and kills
/// the offending thread; the kernel itself continues.
pub fn user_fault(frame: &mut InterruptFrame) -> ! {
    let cr2 = read_cr2();
    USER_FAULTS.fetch_add(1, Ordering::SeqCst);
    serial::force_print(format_args!(
        "user fault isolated: vector={} error={:#x} rip={:#x} cr2={:#x} (thread killed, kernel continues)\r\n",
        frame.vector, frame.error, frame.rip, cr2
    ));
    sched::exit()
}
