//! The ring-3 syscall boundary (`int 0x80`) and user-fault handling (Phase 5b;
//! Win32 console services Phase 7, stages 2–3).
//!
//! ABI (Windows-style argument registers chosen so the later PE compatibility
//! layer can reuse the same gate):
//!
//! * `rax` — service number
//! * `rcx`, `rdx`, `r8`, `r9` — arguments
//! * `r10` — fifth argument (`WriteFile`'s `lpOverlapped`)
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
//! * `6 WriteFile(rcx = handle, rdx = buffer, r8 = length, r9 = written,
//!   r10 = overlapped)` → `1`/`0`; native `kernel32!WriteFile` for the one
//!   console handle. Argument errors return `0` and set the per-thread
//!   last-error value; unmapped spans still kill the thread.
//! * `7 GetLastError()` → the calling thread's last-error value.
//! * `8 SetLastError(rcx = code)` — set the calling thread's last-error
//!   value.
//! * `9 ExitThread(rcx = code)` — native `kernel32!ExitThread`; same kill
//!   path as `5`.
//! * `10 GetCurrentProcessId()` / `11 GetCurrentThreadId()` → synthetic
//!   constant `1` (no NT ID space exists here; documented).
//! * `12 GetTickCount()` → milliseconds since boot (`PIT ticks × 10`, a
//!   wrapping 32-bit value, 10 ms resolution).
//! * `13 GetCommandLineA()` → the loader-owned NUL-terminated module path
//!   in the stub page tail.
//!
//! Every user pointer is validated against the live page tables before the
//! kernel reads it (mapped, user-accessible, canonical, bounded span). A
//! violation, an unknown service, or a fault taken from CPL3 kills the calling
//! thread (`sched::exit`) and the kernel continues — that is the isolation
//! property this milestone proves.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::idt::InterruptFrame;
use crate::arch::x86_64::read_cr2;
use crate::pe_loader::COMMAND_LINE_VA;
use crate::sched;
use crate::serial;
use crate::vm;

pub const SYS_WRITE: u64 = 1;
pub const SYS_EXIT: u64 = 2;
pub const SYS_GET_STD_HANDLE: u64 = 3;
pub const SYS_WRITE_CONSOLE: u64 = 4;
pub const SYS_EXIT_PROCESS: u64 = 5;
pub const SYS_WRITEFILE: u64 = 6;
pub const SYS_GET_LAST_ERROR: u64 = 7;
pub const SYS_SET_LAST_ERROR: u64 = 8;
pub const SYS_EXIT_THREAD: u64 = 9;
pub const SYS_GET_CURRENT_PROCESS_ID: u64 = 10;
pub const SYS_GET_CURRENT_THREAD_ID: u64 = 11;
pub const SYS_GET_TICK_COUNT: u64 = 12;
pub const SYS_GET_COMMAND_LINE: u64 = 13;

/// Maximum accepted console write, small enough to bound the kernel copy.
pub const MAX_WRITE: u64 = 4096;

/// `STD_OUTPUT_HANDLE` is `((DWORD)-11)`; the one handle this stage serves.
const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5;
/// The fabricated console handle `GetStdHandle(2)` returns.
const CONSOLE_HANDLE: u64 = 1;

/// WinError.h `ERROR_INVALID_HANDLE`.
const ERROR_INVALID_HANDLE: u32 = 6;
/// WinError.h `ERROR_INVALID_PARAMETER`.
const ERROR_INVALID_PARAMETER: u32 = 87;

/// Highest address the kernel accepts from user space (canonical user half:
/// PML4 entries 0..256).
const USER_LIMIT: u64 = 0x0000_8000_0000_0000;

/// Last exit code passed to `SYS_EXIT`, `SYS_EXIT_PROCESS` or
/// `SYS_EXIT_THREAD` (single user thread in this milestone).
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
/// `SYS_WRITEFILE` calls (successful and failing alike).
pub static WIN32_WRITEFILE_CALLS: AtomicU64 = AtomicU64::new(0);
/// `SYS_GET_LAST_ERROR` calls.
pub static WIN32_GET_LAST_ERROR_CALLS: AtomicU64 = AtomicU64::new(0);
/// `SYS_SET_LAST_ERROR` calls.
pub static WIN32_SET_LAST_ERROR_CALLS: AtomicU64 = AtomicU64::new(0);
/// `SYS_EXIT_THREAD` calls (each one ends the thread).
pub static WIN32_EXIT_THREAD_CALLS: AtomicU64 = AtomicU64::new(0);
/// `SYS_GET_CURRENT_PROCESS_ID` calls.
pub static WIN32_GET_CURRENT_PROCESS_ID_CALLS: AtomicU64 = AtomicU64::new(0);
/// `SYS_GET_CURRENT_THREAD_ID` calls.
pub static WIN32_GET_CURRENT_THREAD_ID_CALLS: AtomicU64 = AtomicU64::new(0);
/// `SYS_GET_TICK_COUNT` calls.
pub static WIN32_GET_TICK_COUNT_CALLS: AtomicU64 = AtomicU64::new(0);
/// `SYS_GET_COMMAND_LINE` calls.
pub static WIN32_GET_COMMAND_LINE_CALLS: AtomicU64 = AtomicU64::new(0);
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
        SYS_WRITEFILE => write_file(frame),
        SYS_GET_LAST_ERROR => get_last_error(frame),
        SYS_SET_LAST_ERROR => set_last_error(frame),
        SYS_EXIT_THREAD => exit_thread(frame),
        SYS_GET_CURRENT_PROCESS_ID => get_current_process_id(frame),
        SYS_GET_CURRENT_THREAD_ID => get_current_thread_id(frame),
        SYS_GET_TICK_COUNT => get_tick_count(frame),
        SYS_GET_COMMAND_LINE => get_command_line(frame),
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
    let Some(cell) = validated_out_pointer(
        frame,
        written_pointer,
        "WriteConsoleA out-pointer is not mapped",
    ) else {
        return;
    };

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

/// `6 WriteFile`: native `kernel32!WriteFile` for the one console handle.
/// Documented argument errors return `0` and set the thread's last-error
/// value (`ERROR_INVALID_HANDLE` / `ERROR_INVALID_PARAMETER`) without killing
/// the thread; unmapped spans are memory-safety violations and still kill it.
/// Success writes the bytes, stores the count and leaves the last-error value
/// untouched.
fn write_file(frame: &mut InterruptFrame) {
    let handle = frame.rcx;
    let buffer = frame.rdx;
    let length = frame.r8;
    let written_pointer = frame.r9;
    let overlapped = frame.r10;
    WIN32_WRITEFILE_CALLS.fetch_add(1, Ordering::SeqCst);
    if handle != CONSOLE_HANDLE {
        sched::set_current_last_error(ERROR_INVALID_HANDLE);
        serial::force_print(format_args!(
            "win32: WriteFile({}) -> 0 (invalid handle)\r\n",
            handle
        ));
        frame.rax = 0;
        return;
    }
    if overlapped != 0 {
        sched::set_current_last_error(ERROR_INVALID_PARAMETER);
        serial::force_print(format_args!(
            "win32: WriteFile(1, {} bytes, overlapped) -> 0 (invalid parameter)\r\n",
            length
        ));
        frame.rax = 0;
        return;
    }
    if length == 0 || length > MAX_WRITE {
        sched::set_current_last_error(ERROR_INVALID_PARAMETER);
        serial::force_print(format_args!(
            "win32: WriteFile(1, {} bytes) -> 0 (invalid parameter)\r\n",
            length
        ));
        frame.rax = 0;
        return;
    }
    if !validate_user_span(frame, buffer, length) {
        return;
    }
    // The out-parameter must be a mapped user page with 4 bytes available.
    let Some(cell) = validated_out_pointer(
        frame,
        written_pointer,
        "WriteFile out-pointer is not mapped",
    ) else {
        return;
    };

    // SAFETY: the span was validated as mapped, user-accessible and bounded;
    // the out-pointer was validated as a mapped user page with 4 bytes
    // available, and the kernel stores through the HHDM so a read-only user
    // mapping cannot fault it. The last-error value is deliberately untouched.
    unsafe {
        let bytes = core::slice::from_raw_parts(buffer as *const u8, length as usize);
        serial::force_write_bytes(bytes);
        core::ptr::write_volatile(cell, length as u32);
    }
    WRITE_CALLS.fetch_add(1, Ordering::SeqCst);
    BYTES_WRITTEN.fetch_add(length, Ordering::SeqCst);
    serial::force_print(format_args!(
        "win32: WriteFile(1, {} bytes) -> 1\r\n",
        length
    ));
    frame.rax = 1;
}

/// `7 GetLastError`: the calling thread's last-error value.
fn get_last_error(frame: &mut InterruptFrame) {
    let value = sched::current_last_error();
    WIN32_GET_LAST_ERROR_CALLS.fetch_add(1, Ordering::SeqCst);
    serial::force_print(format_args!("win32: GetLastError() -> {}\r\n", value));
    frame.rax = u64::from(value);
}

/// `8 SetLastError`: set the calling thread's last-error value.
fn set_last_error(frame: &mut InterruptFrame) {
    let code = frame.rcx as u32;
    sched::set_current_last_error(code);
    WIN32_SET_LAST_ERROR_CALLS.fetch_add(1, Ordering::SeqCst);
    serial::force_print(format_args!("win32: SetLastError({})\r\n", code));
}

/// `9 ExitThread`: record the exit code and end the thread through the same
/// scheduler-kill path as `5 ExitProcess`.
fn exit_thread(frame: &mut InterruptFrame) -> ! {
    let code = frame.rcx as u32;
    LAST_EXIT_CODE.store(u64::from(code), Ordering::SeqCst);
    WIN32_EXIT_THREAD_CALLS.fetch_add(1, Ordering::SeqCst);
    serial::force_print(format_args!("win32: ExitThread({})\r\n", code));
    sched::exit()
}

/// `10 GetCurrentProcessId`: synthetic constant `1` (no NT ID space exists).
fn get_current_process_id(frame: &mut InterruptFrame) {
    WIN32_GET_CURRENT_PROCESS_ID_CALLS.fetch_add(1, Ordering::SeqCst);
    serial::force_print(format_args!("win32: GetCurrentProcessId() -> 1\r\n"));
    frame.rax = 1;
}

/// `11 GetCurrentThreadId`: synthetic constant `1` (no NT ID space exists).
fn get_current_thread_id(frame: &mut InterruptFrame) {
    WIN32_GET_CURRENT_THREAD_ID_CALLS.fetch_add(1, Ordering::SeqCst);
    serial::force_print(format_args!("win32: GetCurrentThreadId() -> 1\r\n"));
    frame.rax = 1;
}

/// `12 GetTickCount`: milliseconds since boot, derived from the 100 Hz PIT
/// (10 ms resolution) as a wrapping 32-bit value, like the documented DWORD.
fn get_tick_count(frame: &mut InterruptFrame) {
    let milliseconds = crate::arch::x86_64::pit::ticks().wrapping_mul(10) as u32;
    WIN32_GET_TICK_COUNT_CALLS.fetch_add(1, Ordering::SeqCst);
    serial::force_print(format_args!(
        "win32: GetTickCount() -> {}\r\n",
        milliseconds
    ));
    frame.rax = u64::from(milliseconds);
}

/// `13 GetCommandLineA`: the loader-owned NUL-terminated module path in the
/// stub page tail. The string is read back through the live user mapping;
/// the loader guarantees the terminator inside the fixed span.
fn get_command_line(frame: &mut InterruptFrame) {
    let pointer = COMMAND_LINE_VA;
    match vm::translate_live(pointer) {
        Some((_, flags)) if flags & os_core::paging::USER != 0 => {}
        _ => {
            kill(frame, "GetCommandLineA buffer is not mapped");
            return;
        }
    }
    let mut text = [0u8; 255];
    let mut length = 0usize;
    // SAFETY: the stub page is mapped user-readable for the whole program
    // lifetime and the loader wrote the path (at most 255 bytes) followed by
    // a zeroed terminator into it; the read stays inside the fixed span.
    unsafe {
        while length < text.len() {
            let byte = core::ptr::read_volatile((pointer + length as u64) as *const u8);
            if byte == 0 {
                break;
            }
            text[length] = byte;
            length += 1;
        }
    }
    if length == text.len() {
        kill(frame, "GetCommandLineA string is not terminated");
        return;
    }
    let path = core::str::from_utf8(&text[..length]).unwrap_or("<non-utf8 path>");
    WIN32_GET_COMMAND_LINE_CALLS.fetch_add(1, Ordering::SeqCst);
    serial::force_print(format_args!("win32: GetCommandLineA() -> \"{}\"\r\n", path));
    frame.rax = pointer;
}

/// Validate a 4-byte user out-pointer (mapped, user-accessible, canonical,
/// in-page) and return its HHDM cell, or kill the thread with `reason`.
/// Shared by `WriteConsoleA` and `WriteFile`.
fn validated_out_pointer(
    frame: &InterruptFrame,
    pointer: u64,
    reason: &'static str,
) -> Option<*mut u32> {
    let Some(hhdm) = vm::hhdm_offset() else {
        kill(frame, reason);
        return None;
    };
    if pointer < 0x1000
        || pointer.checked_add(4).is_none_or(|end| end > USER_LIMIT)
        || pointer % 4096 > 4092
    {
        kill(frame, reason);
        return None;
    }
    let Some((physical, flags)) = vm::translate_live(pointer) else {
        kill(frame, reason);
        return None;
    };
    if flags & os_core::paging::USER == 0 {
        kill(frame, reason);
        return None;
    }
    let Some(cell) = hhdm.checked_add(physical) else {
        kill(frame, reason);
        return None;
    };
    Some(cell as *mut u32)
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
