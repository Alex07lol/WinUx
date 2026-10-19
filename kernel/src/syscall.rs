//! The ring-3 syscall boundary (`int 0x80`) and user-fault handling (Phase 5b;
//! Win32 console services Phase 7, stages 2–3; Phase 8, stages 2–3).
//!
//! ABI (Windows-style argument registers chosen so the later PE compatibility
//! layer can reuse the same gate):
//!
//! * `rax` — service number
//! * `rcx`, `rdx`, `r8`, `r9` — arguments
//! * `r10` — fifth argument (`WriteFile`'s `lpOverlapped`,
//!   `RegQueryValueExA`'s `lpData`)
//! * `r11` — sixth argument (`RegQueryValueExA`'s `lpcbData`)
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
//! * `14 ReadFile(rcx = handle, rdx = buffer, r8 = length, r9 = read,
//!   r10 = overlapped)` → `1`/`0`; native `kernel32!ReadFile` for the console
//!   input handle (blocking line mode, `docs/WIN32_INPUT.md`).
//! * `15 RegOpenKeyExA(rcx = hKey, rdx = subkey, r8 = options,
//!   r9 = samDesired, r10 = result)` → `0`/WinError; native
//!   `advapi32!RegOpenKeyExA` over the config store (`docs/WIN32_REGISTRY.md`).
//! * `16 RegQueryValueExA(rcx = hKey, rdx = value name, r8 = reserved,
//!   r9 = type, r10 = data, r11 = size)` → `0`/WinError; native
//!   `advapi32!RegQueryValueExA` (REG_SZ reads, size queries,
//!   `ERROR_MORE_DATA`).
//! * `17 RegCloseKey(rcx = hKey)` → `0`/`6`; native `advapi32!RegCloseKey`
//!   over the four-slot key-handle table.
//! * `18 RegCreateKeyExA(rcx = hKey, rdx = subkey, r8 = reserved,
//!   r9 = class, r10 = options, r11 = samDesired, r12 = security attrs,
//!   r13 = result, r14 = disposition)` → `0`/WinError; native
//!   `advapi32!RegCreateKeyExA` (first nine-argument service,
//!   `docs/WIN32_REGISTRY_WRITE.md`).
//! * `19 RegSetValueExA(rcx = hKey, rdx = value name, r8 = reserved,
//!   r9 = type, r10 = data, r11 = size)` → `0`/WinError; native
//!   `advapi32!RegSetValueExA` (REG_SZ writes persisted through
//!   `config::save()`).
//! * `20 RegDeleteValueA(rcx = hKey, rdx = value name)` → `0`/WinError;
//!   native `advapi32!RegDeleteValueA`.
//!
//! Every user pointer is validated against the live page tables before the
//! kernel reads it (mapped, user-accessible, canonical, bounded span). A
//! violation, an unknown service, or a fault taken from CPL3 kills the calling
//! thread (`sched::exit`) and the kernel continues — that is the isolation
//! property this milestone proves.

use core::sync::atomic::{AtomicU64, Ordering};

use alloc::string::String;
use alloc::vec::Vec;

use crate::arch::x86_64::idt::InterruptFrame;
use crate::arch::x86_64::{halt, interrupts_enable, read_cr2};
use crate::console;
use crate::pe_loader::COMMAND_LINE_VA;
use crate::registry;
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
pub const SYS_READFILE: u64 = 14;
pub const SYS_REG_OPEN_KEY_EX: u64 = 15;
pub const SYS_REG_QUERY_VALUE_EX: u64 = 16;
pub const SYS_REG_CLOSE_KEY: u64 = 17;
pub const SYS_REG_CREATE_KEY_EX: u64 = 18;
pub const SYS_REG_SET_VALUE_EX: u64 = 19;
pub const SYS_REG_DELETE_VALUE: u64 = 20;

/// Maximum accepted console write, small enough to bound the kernel copy.
pub const MAX_WRITE: u64 = 4096;
/// Maximum accepted console read; the line buffer never exceeds this.
pub const MAX_READ: u64 = 4096;

/// `STD_OUTPUT_HANDLE` is `((DWORD)-11)`; `STD_INPUT_HANDLE` is `((DWORD)-10)`.
const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5;
const STD_INPUT_HANDLE: u32 = 0xFFFF_FFF6;
/// The fabricated console output handle `GetStdHandle(-11)` returns.
const CONSOLE_HANDLE: u64 = 1;
/// The fabricated console input handle `GetStdHandle(-10)` returns.
const INPUT_HANDLE: u64 = 2;

/// WinError.h `ERROR_INVALID_HANDLE`.
const ERROR_INVALID_HANDLE: u32 = 6;
/// WinError.h `ERROR_INVALID_PARAMETER`.
const ERROR_INVALID_PARAMETER: u32 = 87;
/// WinError.h `ERROR_FILE_NOT_FOUND`.
const ERROR_FILE_NOT_FOUND: u32 = 2;
/// WinError.h `ERROR_TOO_MANY_OPEN_FILES`.
const ERROR_TOO_MANY_OPEN_FILES: u32 = 4;
/// WinError.h `ERROR_NOT_ENOUGH_MEMORY`; the store's resource-exhaustion code.
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
/// WinError.h `ERROR_MORE_DATA`.
const ERROR_MORE_DATA: u32 = 234;
/// Registry value type `REG_SZ`; the only value type this stage supports.
const REG_SZ: u32 = 1;
/// `RegCreateKeyExA` disposition: the key did not exist and was created.
const REG_CREATED_NEW_KEY: u32 = 1;
/// `RegCreateKeyExA` disposition: the key existed and was opened.
const REG_OPENED_EXISTING_KEY: u32 = 2;
/// Maximum registry subkey or value-name length including the NUL.
const MAX_REG_NAME: usize = 128;
/// Maximum accepted `RegSetValueExA` `cbData`: a store value (128 bytes) plus
/// the terminating NUL.
const MAX_REG_VALUE: u64 = os_core::config::MAX_VALUE_BYTES as u64 + 1;

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
/// `SYS_READFILE` calls (successful and failing alike).
pub static WIN32_READFILE_CALLS: AtomicU64 = AtomicU64::new(0);
/// `SYS_REG_OPEN_KEY_EX` calls (successful and failing alike).
pub static WIN32_REG_OPEN_KEY_CALLS: AtomicU64 = AtomicU64::new(0);
/// `SYS_REG_QUERY_VALUE_EX` calls (successful and failing alike).
pub static WIN32_REG_QUERY_VALUE_CALLS: AtomicU64 = AtomicU64::new(0);
/// `SYS_REG_CLOSE_KEY` calls (successful and failing alike).
pub static WIN32_REG_CLOSE_KEY_CALLS: AtomicU64 = AtomicU64::new(0);
/// `SYS_REG_CREATE_KEY_EX` calls (successful and failing alike).
pub static WIN32_REG_CREATE_KEY_CALLS: AtomicU64 = AtomicU64::new(0);
/// `SYS_REG_SET_VALUE_EX` calls (successful and failing alike).
pub static WIN32_REG_SET_VALUE_CALLS: AtomicU64 = AtomicU64::new(0);
/// `SYS_REG_DELETE_VALUE` calls (successful and failing alike).
pub static WIN32_REG_DELETE_VALUE_CALLS: AtomicU64 = AtomicU64::new(0);
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
        SYS_READFILE => read_file(frame),
        SYS_REG_OPEN_KEY_EX => reg_open_key_ex(frame),
        SYS_REG_QUERY_VALUE_EX => reg_query_value_ex(frame),
        SYS_REG_CLOSE_KEY => reg_close_key(frame),
        SYS_REG_CREATE_KEY_EX => reg_create_key_ex(frame),
        SYS_REG_SET_VALUE_EX => reg_set_value_ex(frame),
        SYS_REG_DELETE_VALUE => reg_delete_value(frame),
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
/// docs), so only the input and output handles are recognized and anything
/// else returns `INVALID_HANDLE_VALUE` without killing the thread.
fn get_std_handle(frame: &mut InterruptFrame) {
    WIN32_GET_STD_HANDLE_CALLS.fetch_add(1, Ordering::SeqCst);
    if frame.rcx as u32 == STD_OUTPUT_HANDLE {
        serial::force_print(format_args!("win32: GetStdHandle(-11) -> 1\r\n"));
        frame.rax = CONSOLE_HANDLE;
    } else if frame.rcx as u32 == STD_INPUT_HANDLE {
        serial::force_print(format_args!("win32: GetStdHandle(-10) -> 2\r\n"));
        frame.rax = INPUT_HANDLE;
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

/// `14 ReadFile`: native `kernel32!ReadFile` for the console input handle.
/// Line mode with echo is the only mode: the call blocks until CR/LF, stores
/// the line plus CRLF and reports the count. `*lpNumberOfBytesRead` is zeroed
/// before the argument checks, as documented. Documented argument errors
/// return `0` and set the thread's last-error value without killing the
/// thread; unmapped spans are memory-safety violations and still kill it.
fn read_file(frame: &mut InterruptFrame) {
    let handle = frame.rcx;
    let buffer = frame.rdx;
    let length = frame.r8;
    let read_pointer = frame.r9;
    let overlapped = frame.r10;
    WIN32_READFILE_CALLS.fetch_add(1, Ordering::SeqCst);

    // The documented contract sets the count to zero before any work or error
    // checking, so the out-pointer must be validated first.
    let Some(cell) =
        validated_out_pointer(frame, read_pointer, "ReadFile out-pointer is not mapped")
    else {
        return;
    };
    // SAFETY: the out-pointer was validated as a mapped user page with 4
    // bytes available; the store goes through the HHDM.
    unsafe { core::ptr::write_volatile(cell, 0) };

    if handle != INPUT_HANDLE {
        sched::set_current_last_error(ERROR_INVALID_HANDLE);
        serial::force_print(format_args!(
            "win32: ReadFile({}) -> 0 (invalid handle)\r\n",
            handle
        ));
        frame.rax = 0;
        return;
    }
    if overlapped != 0 {
        sched::set_current_last_error(ERROR_INVALID_PARAMETER);
        serial::force_print(format_args!(
            "win32: ReadFile(2, {} bytes, overlapped) -> 0 (invalid parameter)\r\n",
            length
        ));
        frame.rax = 0;
        return;
    }
    if !(2..=MAX_READ).contains(&length) {
        sched::set_current_last_error(ERROR_INVALID_PARAMETER);
        serial::force_print(format_args!(
            "win32: ReadFile(2, {} bytes) -> 0 (invalid parameter)\r\n",
            length
        ));
        frame.rax = 0;
        return;
    }
    if !validate_user_span(frame, buffer, length) {
        return;
    }

    serial::force_print(format_args!(
        "win32: ReadFile(2, {} bytes) waiting for input\r\n",
        length
    ));

    // The `int 0x80` gate entered with interrupts disabled, so enable them
    // before sleeping: the serial RX IRQ delivers input and the PIT keeps the
    // scheduler alive. No lock is held across the wait.
    interrupts_enable();
    let capacity = length as usize;
    let mut count = 0usize;
    loop {
        // Leave room for CRLF: at most `length - 2` content bytes.
        if count >= capacity - 2 {
            break;
        }
        let Some(byte) = console::try_read_byte() else {
            halt();
            continue;
        };
        match byte {
            b'\r' | b'\n' => {
                // SAFETY: the span was validated as mapped, user-accessible
                // and bounded, and `count + 2 <= length`.
                unsafe {
                    let out = buffer as *mut u8;
                    core::ptr::write_volatile(out.add(count), b'\r');
                    core::ptr::write_volatile(out.add(count + 1), b'\n');
                }
                count += 2;
                serial::force_write_bytes(b"\r\n");
                break;
            }
            0x08 | 0x7f => {
                if count > 0 {
                    count -= 1;
                    serial::force_write_bytes(b"\x08 \x08");
                }
            }
            0x20..=0x7e => {
                // SAFETY: the span was validated and `count < length`.
                unsafe {
                    core::ptr::write_volatile((buffer as *mut u8).add(count), byte);
                }
                count += 1;
                serial::force_write_bytes(&[byte]);
            }
            _ => {}
        }
    }

    // SAFETY: the out-pointer was validated earlier in this call.
    unsafe { core::ptr::write_volatile(cell, count as u32) };
    serial::force_print(format_args!(
        "win32: ReadFile(2, {} bytes) -> 1 ({} bytes)\r\n",
        length, count
    ));
    frame.rax = 1;
}

/// Registry handle rendering for serial lines: `HKLM` as `0x80000002`, slot
/// handles as plain decimal.
struct RegHandle(u64);

impl core::fmt::Display for RegHandle {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.0 == registry::HKEY_LOCAL_MACHINE {
            formatter.write_str("0x80000002")
        } else {
            write!(formatter, "{}", self.0)
        }
    }
}

fn reg_error_label(code: u32) -> &'static str {
    match code {
        ERROR_FILE_NOT_FOUND => "file not found",
        ERROR_TOO_MANY_OPEN_FILES => "too many open keys",
        ERROR_INVALID_HANDLE => "invalid handle",
        ERROR_NOT_ENOUGH_MEMORY => "not enough memory",
        _ => "invalid parameter",
    }
}

/// The WinError value for a failed registry operation.
fn reg_key_error_code(error: registry::KeyError) -> u32 {
    match error {
        registry::KeyError::InvalidHandle => ERROR_INVALID_HANDLE,
        registry::KeyError::NotFound => ERROR_FILE_NOT_FOUND,
        registry::KeyError::TooManyKeys => ERROR_TOO_MANY_OPEN_FILES,
        registry::KeyError::BadArgument => ERROR_INVALID_PARAMETER,
        registry::KeyError::NoSpace => ERROR_NOT_ENOUGH_MEMORY,
    }
}

/// `15 RegOpenKeyExA`: native `advapi32!RegOpenKeyExA` over the config store.
/// `*phkResult` is zeroed before any work (our contract for failed opens).
/// Documented argument errors return the WinError value and set the
/// thread's last-error without killing it; unmapped pointers kill.
fn reg_open_key_ex(frame: &mut InterruptFrame) {
    let hkey = frame.rcx;
    let subkey_pointer = frame.rdx;
    let options = frame.r8;
    let result_pointer = frame.r10;
    WIN32_REG_OPEN_KEY_CALLS.fetch_add(1, Ordering::SeqCst);

    let Some(cell) = validated_out_pointer_u64(
        frame,
        result_pointer,
        "RegOpenKeyExA out-pointer is not mapped",
    ) else {
        return;
    };
    // SAFETY: the out-pointer was validated as a mapped user page with 8
    // bytes available; the store goes through the HHDM.
    unsafe { core::ptr::write_volatile(cell, 0) };

    let subkey = match read_user_string(frame, subkey_pointer) {
        Ok(subkey) => subkey,
        Err(()) => {
            reg_open_key_fail(frame, hkey, "<invalid>", ERROR_INVALID_PARAMETER);
            return;
        }
    };

    if options != 0 {
        reg_open_key_fail(frame, hkey, &subkey, ERROR_INVALID_PARAMETER);
        return;
    }

    match registry::open(hkey, &subkey) {
        Ok(handle) => {
            // SAFETY: the out-pointer was validated earlier in this call.
            unsafe { core::ptr::write_volatile(cell, handle) };
            serial::force_print(format_args!(
                "win32: RegOpenKeyExA({:#x}, \"{}\") -> 0 (handle {})\r\n",
                hkey,
                subkey,
                RegHandle(handle)
            ));
            frame.rax = 0;
        }
        Err(error) => {
            reg_open_key_fail(frame, hkey, &subkey, reg_key_error_code(error));
        }
    }
}

fn reg_open_key_fail(frame: &mut InterruptFrame, hkey: u64, subkey: &str, code: u32) {
    sched::set_current_last_error(code);
    serial::force_print(format_args!(
        "win32: RegOpenKeyExA({:#x}, \"{}\") -> {} ({})\r\n",
        hkey,
        subkey,
        code,
        reg_error_label(code)
    ));
    frame.rax = u64::from(code);
}

/// `16 RegQueryValueExA`: native `advapi32!RegQueryValueExA` over the config
/// store. Values are REG_SZ by construction (`bytes + NUL`); a NULL-data
/// query returns the required size, a short buffer returns
/// `ERROR_MORE_DATA` plus the required size. `*lpType` is filled whenever
/// the value exists and the pointer is non-NULL.
fn reg_query_value_ex(frame: &mut InterruptFrame) {
    let hkey = frame.rcx;
    let name_pointer = frame.rdx;
    let reserved = frame.r8;
    let type_pointer = frame.r9;
    let data_pointer = frame.r10;
    let size_pointer = frame.r11;
    WIN32_REG_QUERY_VALUE_CALLS.fetch_add(1, Ordering::SeqCst);

    let name = match read_user_string(frame, name_pointer) {
        Ok(name) => name,
        Err(()) => {
            reg_query_value_fail(frame, hkey, "<invalid>", ERROR_INVALID_PARAMETER);
            return;
        }
    };
    if name.is_empty() {
        // The unnamed/default value is not modeled.
        reg_query_value_fail(frame, hkey, "", ERROR_FILE_NOT_FOUND);
        return;
    }
    if reserved != 0 || size_pointer == 0 {
        reg_query_value_fail(frame, hkey, &name, ERROR_INVALID_PARAMETER);
        return;
    }

    let Some(size_cell) = validated_out_pointer(
        frame,
        size_pointer,
        "RegQueryValueExA size pointer is not mapped",
    ) else {
        return;
    };
    // SAFETY: the size pointer was validated as a mapped user page with 4
    // bytes available; the read goes through the HHDM.
    let capacity = unsafe { core::ptr::read_volatile(size_cell) };

    let value = match registry::value(hkey, &name) {
        Ok(value) => value,
        Err(registry::KeyError::InvalidHandle) => {
            reg_query_value_fail(frame, hkey, &name, ERROR_INVALID_HANDLE);
            return;
        }
        Err(_) => {
            // `NotFound`; `TooManyKeys` cannot occur (no slot is allocated).
            reg_query_value_fail(frame, hkey, &name, ERROR_FILE_NOT_FOUND);
            return;
        }
    };
    // REG_SZ data includes the terminating NUL.
    let required = value.len() as u32 + 1;

    if type_pointer != 0 {
        let Some(type_cell) = validated_out_pointer(
            frame,
            type_pointer,
            "RegQueryValueExA type pointer is not mapped",
        ) else {
            return;
        };
        // SAFETY: the type pointer was validated as a mapped user page with
        // 4 bytes available; the store goes through the HHDM.
        unsafe { core::ptr::write_volatile(type_cell, REG_SZ) };
    }

    if data_pointer == 0 {
        // SAFETY: the size pointer was validated earlier in this call.
        unsafe { core::ptr::write_volatile(size_cell, required) };
        serial::force_print(format_args!(
            "win32: RegQueryValueExA({}, \"{}\") -> 0 (REG_SZ, {} bytes)\r\n",
            RegHandle(hkey),
            name,
            required
        ));
        frame.rax = 0;
        return;
    }
    if capacity < required {
        // SAFETY: the size pointer was validated earlier in this call.
        unsafe { core::ptr::write_volatile(size_cell, required) };
        serial::force_print(format_args!(
            "win32: RegQueryValueExA({}, \"{}\") -> {} (more data, {} bytes needed)\r\n",
            RegHandle(hkey),
            name,
            ERROR_MORE_DATA,
            required
        ));
        frame.rax = u64::from(ERROR_MORE_DATA);
        return;
    }
    if !write_user_bytes(frame, data_pointer, value.as_bytes())
        || !write_user_bytes(frame, data_pointer + value.len() as u64, &[0])
    {
        return;
    }
    // SAFETY: the size pointer was validated earlier in this call.
    unsafe { core::ptr::write_volatile(size_cell, required) };
    serial::force_print(format_args!(
        "win32: RegQueryValueExA({}, \"{}\") -> 0 (REG_SZ, {} bytes) \"{}\"\r\n",
        RegHandle(hkey),
        name,
        required,
        value
    ));
    frame.rax = 0;
}

fn reg_query_value_fail(frame: &mut InterruptFrame, hkey: u64, name: &str, code: u32) {
    sched::set_current_last_error(code);
    serial::force_print(format_args!(
        "win32: RegQueryValueExA({}, \"{}\") -> {} ({})\r\n",
        RegHandle(hkey),
        name,
        code,
        reg_error_label(code)
    ));
    frame.rax = u64::from(code);
}

/// `17 RegCloseKey`: native `advapi32!RegCloseKey` over the four-slot key
/// table. `HKLM` closes as a no-op; an already-closed handle stays invalid.
fn reg_close_key(frame: &mut InterruptFrame) {
    let hkey = frame.rcx;
    WIN32_REG_CLOSE_KEY_CALLS.fetch_add(1, Ordering::SeqCst);
    match registry::close(hkey) {
        Ok(()) => {
            serial::force_print(format_args!(
                "win32: RegCloseKey({}) -> 0\r\n",
                RegHandle(hkey)
            ));
            frame.rax = 0;
        }
        Err(_) => {
            sched::set_current_last_error(ERROR_INVALID_HANDLE);
            serial::force_print(format_args!(
                "win32: RegCloseKey({}) -> {} (invalid handle)\r\n",
                RegHandle(hkey),
                ERROR_INVALID_HANDLE
            ));
            frame.rax = u64::from(ERROR_INVALID_HANDLE);
        }
    }
}

/// `18 RegCreateKeyExA`: native `advapi32!RegCreateKeyExA` over the config
/// store. The ninth argument rides in `r14` (the first nine-argument
/// service). `*phkResult` is zeroed before any work; `*lpdwDisposition` is
/// optional (NULL allowed) and filled on success. Documented argument errors
/// return the WinError value and set the thread's last-error without killing
/// it; unmapped pointers kill.
fn reg_create_key_ex(frame: &mut InterruptFrame) {
    let hkey = frame.rcx;
    let subkey_pointer = frame.rdx;
    let reserved = frame.r8;
    // r9 (`lpClass`) and r12 (`lpSecurityAttributes`) may be ignored.
    let options = frame.r10;
    // r11 (`samDesired`) is ignored: access is checked against the key's
    // security descriptor, which this stage does not have.
    let result_pointer = frame.r13;
    let disposition_pointer = frame.r14;
    WIN32_REG_CREATE_KEY_CALLS.fetch_add(1, Ordering::SeqCst);

    let Some(cell) = validated_out_pointer_u64(
        frame,
        result_pointer,
        "RegCreateKeyExA out-pointer is not mapped",
    ) else {
        return;
    };
    // SAFETY: the out-pointer was validated as a mapped user page with 8
    // bytes available; the store goes through the HHDM.
    unsafe { core::ptr::write_volatile(cell, 0) };

    let subkey = match read_user_string(frame, subkey_pointer) {
        Ok(subkey) => subkey,
        Err(()) => {
            reg_create_key_fail(frame, hkey, "<invalid>", ERROR_INVALID_PARAMETER);
            return;
        }
    };

    if reserved != 0 || options != 0 {
        reg_create_key_fail(frame, hkey, &subkey, ERROR_INVALID_PARAMETER);
        return;
    }

    match registry::create(hkey, &subkey) {
        Ok((handle, created)) => {
            if disposition_pointer != 0 {
                let Some(disposition_cell) = validated_out_pointer(
                    frame,
                    disposition_pointer,
                    "RegCreateKeyExA disposition pointer is not mapped",
                ) else {
                    return;
                };
                // SAFETY: the disposition pointer was validated as a mapped
                // user page with 4 bytes available; the store goes through
                // the HHDM.
                unsafe {
                    core::ptr::write_volatile(
                        disposition_cell,
                        if created {
                            REG_CREATED_NEW_KEY
                        } else {
                            REG_OPENED_EXISTING_KEY
                        },
                    )
                };
            }
            // SAFETY: the out-pointer was validated earlier in this call.
            unsafe { core::ptr::write_volatile(cell, handle) };
            serial::force_print(format_args!(
                "win32: RegCreateKeyExA({:#x}, \"{}\") -> 0 (handle {}, {})\r\n",
                hkey,
                subkey,
                RegHandle(handle),
                if created {
                    "created"
                } else {
                    "opened existing"
                }
            ));
            frame.rax = 0;
        }
        Err(error) => {
            reg_create_key_fail(frame, hkey, &subkey, reg_key_error_code(error));
        }
    }
}

fn reg_create_key_fail(frame: &mut InterruptFrame, hkey: u64, subkey: &str, code: u32) {
    sched::set_current_last_error(code);
    serial::force_print(format_args!(
        "win32: RegCreateKeyExA({:#x}, \"{}\") -> {} ({})\r\n",
        hkey,
        subkey,
        code,
        reg_error_label(code)
    ));
    frame.rax = u64::from(code);
}

/// `19 RegSetValueExA`: native `advapi32!RegSetValueExA` over the config
/// store. REG_SZ only; `cbData` includes the terminating NUL; the payload is
/// validated against the store's data rules, written and persisted through
/// `config::save()`.
fn reg_set_value_ex(frame: &mut InterruptFrame) {
    let hkey = frame.rcx;
    let name_pointer = frame.rdx;
    let reserved = frame.r8;
    let value_type = frame.r9 as u32;
    let data_pointer = frame.r10;
    let size = frame.r11;
    WIN32_REG_SET_VALUE_CALLS.fetch_add(1, Ordering::SeqCst);

    let name = match read_user_string(frame, name_pointer) {
        Ok(name) => name,
        Err(()) => {
            reg_set_value_fail(frame, hkey, "<invalid>", ERROR_INVALID_PARAMETER);
            return;
        }
    };
    if name.is_empty() {
        // The unnamed/default value is not modeled.
        reg_set_value_fail(frame, hkey, "", ERROR_INVALID_PARAMETER);
        return;
    }
    if reserved != 0 || value_type != REG_SZ || data_pointer == 0 {
        reg_set_value_fail(frame, hkey, &name, ERROR_INVALID_PARAMETER);
        return;
    }
    if !(1..=MAX_REG_VALUE).contains(&size) {
        reg_set_value_fail(frame, hkey, &name, ERROR_INVALID_PARAMETER);
        return;
    }

    let payload = match read_user_reg_value(frame, data_pointer, size) {
        Ok(payload) => payload,
        Err(()) => {
            reg_set_value_fail(frame, hkey, &name, ERROR_INVALID_PARAMETER);
            return;
        }
    };

    match registry::set_value(hkey, &name, &payload) {
        Ok(()) => {
            serial::force_print(format_args!(
                "win32: RegSetValueExA({}, \"{}\") -> 0 (REG_SZ, {} bytes)\r\n",
                RegHandle(hkey),
                name,
                payload.len()
            ));
            frame.rax = 0;
        }
        Err(error) => {
            reg_set_value_fail(frame, hkey, &name, reg_key_error_code(error));
        }
    }
}

fn reg_set_value_fail(frame: &mut InterruptFrame, hkey: u64, name: &str, code: u32) {
    sched::set_current_last_error(code);
    serial::force_print(format_args!(
        "win32: RegSetValueExA({}, \"{}\") -> {} ({})\r\n",
        RegHandle(hkey),
        name,
        code,
        reg_error_label(code)
    ));
    frame.rax = u64::from(code);
}

/// `20 RegDeleteValueA`: native `advapi32!RegDeleteValueA` over the config
/// store. Removing the last entry under a prefix makes the key itself cease
/// to exist (children-only rule); the removal is persisted.
fn reg_delete_value(frame: &mut InterruptFrame) {
    let hkey = frame.rcx;
    let name_pointer = frame.rdx;
    WIN32_REG_DELETE_VALUE_CALLS.fetch_add(1, Ordering::SeqCst);

    let name = match read_user_string(frame, name_pointer) {
        Ok(name) => name,
        Err(()) => {
            reg_delete_value_fail(frame, hkey, "<invalid>", ERROR_INVALID_PARAMETER);
            return;
        }
    };
    if name.is_empty() {
        // The unnamed/default value is not modeled.
        reg_delete_value_fail(frame, hkey, "", ERROR_INVALID_PARAMETER);
        return;
    }

    match registry::delete_value(hkey, &name) {
        Ok(()) => {
            serial::force_print(format_args!(
                "win32: RegDeleteValueA({}, \"{}\") -> 0\r\n",
                RegHandle(hkey),
                name
            ));
            frame.rax = 0;
        }
        Err(error) => {
            reg_delete_value_fail(frame, hkey, &name, reg_key_error_code(error));
        }
    }
}

fn reg_delete_value_fail(frame: &mut InterruptFrame, hkey: u64, name: &str, code: u32) {
    sched::set_current_last_error(code);
    serial::force_print(format_args!(
        "win32: RegDeleteValueA({}, \"{}\") -> {} ({})\r\n",
        RegHandle(hkey),
        name,
        code,
        reg_error_label(code)
    ));
    frame.rax = u64::from(code);
}

/// Read exactly `size` bytes of REG_SZ data from user memory: the span must
/// be mapped user-accessible, the last byte must be NUL, and the payload
/// must satisfy the store's value rules. Unmapped pages kill; violations are
/// `Err(())`.
fn read_user_reg_value(frame: &InterruptFrame, pointer: u64, size: u64) -> Result<String, ()> {
    let Some(end) = pointer.checked_add(size) else {
        kill(frame, "registry value overflows the address space");
        return Err(());
    };
    if pointer < 0x1000 || end > USER_LIMIT {
        kill(frame, "registry value is outside the user address space");
        return Err(());
    }
    let Some(hhdm) = vm::hhdm_offset() else {
        kill(frame, "HHDM offset is unknown");
        return Err(());
    };
    let mut bytes = Vec::with_capacity(size as usize);
    for offset in 0..size {
        let address = pointer + offset;
        let physical = match vm::translate_live(address) {
            Some((physical, flags)) if flags & os_core::paging::USER != 0 => physical,
            _ => {
                kill(frame, "registry value is not mapped for user access");
                return Err(());
            }
        };
        let Some(cell) = hhdm.checked_add(physical) else {
            kill(frame, "registry value HHDM overflow");
            return Err(());
        };
        // SAFETY: the page is mapped user-accessible in the live address
        // space; the kernel reads it through the HHDM.
        bytes.push(unsafe { core::ptr::read_volatile(cell as *const u8) });
    }
    if bytes.last() != Some(&0) {
        return Err(());
    }
    let payload = core::str::from_utf8(&bytes[..bytes.len() - 1]).map_err(|_| ())?;
    if !os_core::config::value_is_valid(payload) {
        return Err(());
    }
    Ok(String::from(payload))
}

/// Read a NUL-terminated user string (at most [`MAX_REG_NAME`] bytes
/// including the terminator) through the live page tables. A NULL pointer is
/// the empty string; an unmapped page kills the thread; a mapped span with
/// no terminator or with invalid UTF-8 is an argument error (`Err(())`).
fn read_user_string(frame: &InterruptFrame, pointer: u64) -> Result<String, ()> {
    if pointer == 0 {
        return Ok(String::new());
    }
    if !(0x1000..USER_LIMIT).contains(&pointer) {
        kill(
            frame,
            "registry string pointer is outside the user address space",
        );
        return Err(());
    }
    let Some(hhdm) = vm::hhdm_offset() else {
        kill(frame, "HHDM offset is unknown");
        return Err(());
    };
    let mut bytes = Vec::new();
    for offset in 0..MAX_REG_NAME as u64 {
        let address = pointer + offset;
        let physical = match vm::translate_live(address) {
            Some((physical, flags)) if flags & os_core::paging::USER != 0 => physical,
            _ => {
                kill(frame, "registry string is not mapped for user access");
                return Err(());
            }
        };
        let Some(cell) = hhdm.checked_add(physical) else {
            kill(frame, "registry string HHDM overflow");
            return Err(());
        };
        // SAFETY: the page is mapped user-accessible in the live address
        // space; the kernel reads it through the HHDM.
        let byte = unsafe { core::ptr::read_volatile(cell as *const u8) };
        if byte == 0 {
            return core::str::from_utf8(&bytes)
                .map(String::from)
                .map_err(|_| ());
        }
        bytes.push(byte);
    }
    Err(())
}

/// Write `bytes` to a user buffer through the HHDM after proving the span is
/// inside the user half and every touched page is mapped user-accessible;
/// any violation kills the thread. Returns false when the caller must stop.
fn write_user_bytes(frame: &InterruptFrame, pointer: u64, bytes: &[u8]) -> bool {
    let Some(end) = pointer.checked_add(bytes.len() as u64) else {
        kill(frame, "registry buffer overflows the address space");
        return false;
    };
    if pointer < 0x1000 || end > USER_LIMIT {
        kill(frame, "registry buffer is outside the user address space");
        return false;
    }
    let Some(hhdm) = vm::hhdm_offset() else {
        kill(frame, "HHDM offset is unknown");
        return false;
    };
    for (offset, byte) in bytes.iter().copied().enumerate() {
        let address = pointer + offset as u64;
        let physical = match vm::translate_live(address) {
            Some((physical, flags)) if flags & os_core::paging::USER != 0 => physical,
            _ => {
                kill(frame, "registry buffer is not mapped for user access");
                return false;
            }
        };
        let Some(cell) = hhdm.checked_add(physical) else {
            kill(frame, "registry buffer HHDM overflow");
            return false;
        };
        // SAFETY: the page is mapped user-accessible in the live address
        // space; the kernel writes it through the HHDM.
        unsafe { core::ptr::write_volatile(cell as *mut u8, byte) };
    }
    true
}

/// Like [`validated_out_pointer`] for 8-byte handle out-parameters.
fn validated_out_pointer_u64(
    frame: &InterruptFrame,
    pointer: u64,
    reason: &'static str,
) -> Option<*mut u64> {
    let Some(hhdm) = vm::hhdm_offset() else {
        kill(frame, reason);
        return None;
    };
    if pointer < 0x1000
        || pointer.checked_add(8).is_none_or(|end| end > USER_LIMIT)
        || pointer % 4096 > 4088
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
    Some(cell as *mut u64)
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
