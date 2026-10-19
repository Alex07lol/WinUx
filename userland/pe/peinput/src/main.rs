//! PROJECT_OS PE32+ fixture (Phase 8, stage 2): Win32 console input.
//!
//! Built for `x86_64-pc-windows-msvc` and linked by `rust-lld` with
//! `/nodefaultlib /entry:_start /subsystem:console`. `ReadFile` is declared
//! with `raw-dylib` linkage, so the compiler and linker synthesize a genuine
//! import entry against `kernel32.dll` with no import library or SDK; the
//! kernel resolves the name against its own native implementation and patches
//! the IAT to its `int 0x80` stub. Every failure exits with a distinct code so
//! the serial log names the broken step.
//!
//! The phase harness types `hello, win32` + CR while this image is blocked in
//! `ReadFile`; the fixture verifies only structure (the buffer ends in CRLF)
//! and plays the line back. The kernel's own summary line pins the byte count.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::ffi::c_void;
use core::ptr::{addr_of, addr_of_mut, null_mut};

/// `STD_INPUT_HANDLE` is `((DWORD)-10)`.
const STD_INPUT_HANDLE: u32 = 0xFFFF_FFF6;

/// The distinct handle the kernel's `GetStdHandle(-10)` returns.
const INPUT_HANDLE: u64 = 2;

/// `ERROR_INVALID_HANDLE` (WinError.h).
const ERROR_INVALID_HANDLE: u32 = 6;
/// `ERROR_INVALID_PARAMETER` (WinError.h).
const ERROR_INVALID_PARAMETER: u32 = 87;

/// Console output handle: the kernel's `GetStdHandle(-11)` returns `1`.
const CONSOLE_HANDLE: u64 = 1;

const L2: &[u8; 30] = b"win32: input surface verified\n";

/// Size of the blocking line-read destination.
const BUFFER_LEN: usize = 64;

/// Destination for the blocking line read (writable `.data`).
static mut BUFFER: [u8; BUFFER_LEN] = [0; BUFFER_LEN];

// Win64 (`extern "system"`) shapes as the kernel gate implements them:
// `rcx/rdx/r8/r9`, fifth argument captured from `[rsp+0x28]` into `r10`.
#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(n_std_handle: u32) -> u64;
    fn ReadFile(
        handle: u64,
        buffer: *mut u8,
        length: u32,
        read: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn WriteFile(
        handle: u64,
        buffer: *const u8,
        length: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn GetLastError() -> u32;
    fn ExitThread(exit_code: u32) -> !;
}

/// A read-only marker whose address the relocated slot must match.
#[unsafe(no_mangle)]
pub static reloc_marker: u8 = 0x42;

// An absolute 64-bit reference in `.data`: forces a genuine DIR64 base
// relocation, so the image meets the loader's "a rebase must be possible"
// rule (same probe as the stage-1/2/3 fixtures).
global_asm!(
    ".section .data",
    ".globl marker_slot",
    ".p2align 3",
    "marker_slot:",
    ".quad reloc_marker",
);

unsafe extern "C" {
    static marker_slot: u64;
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    // SAFETY: the imported functions are resolved by the kernel before this
    // image runs; every pointer argument is a live local, the writable static
    // buffer, or the forced-relocation slot. `ExitThread` does not return.
    unsafe {
        // The volatile read keeps the forced relocation observable.
        core::hint::black_box(core::ptr::read_volatile(addr_of!(marker_slot)));

        let stdin = GetStdHandle(STD_INPUT_HANDLE);
        if stdin != INPUT_HANDLE {
            ExitThread(1);
        }

        let buffer = addr_of_mut!(BUFFER) as *mut u8;

        // Documented: the count is zeroed before any work or error checking,
        // even when the handle itself is rejected.
        let mut count: u32 = 0xDEAD_BEEF;
        let ok = ReadFile(1, buffer, 16, &mut count, null_mut());
        if ok != 0 || count != 0 {
            ExitThread(2);
        }
        if GetLastError() != ERROR_INVALID_HANDLE {
            ExitThread(3);
        }

        // An overlapped request is rejected explicitly (no async I/O).
        let ok = ReadFile(
            stdin,
            buffer,
            16,
            &mut count,
            core::ptr::dangling_mut::<c_void>(),
        );
        if ok != 0 || GetLastError() != ERROR_INVALID_PARAMETER {
            ExitThread(4);
        }

        // Blocks until the harness types a line and the kernel delivers CRLF.
        count = 0xDEAD_BEEF;
        let ok = ReadFile(stdin, buffer, BUFFER_LEN as u32, &mut count, null_mut());
        if ok == 0 || count == 0xDEAD_BEEF {
            ExitThread(5);
        }
        if count < 2
            || core::ptr::read_volatile(buffer.add(count as usize - 2)) != b'\r'
            || core::ptr::read_volatile(buffer.add(count as usize - 1)) != b'\n'
        {
            ExitThread(6);
        }

        // Play the line back; the serial transcript shows both the kernel's
        // echo of the typed bytes and this second, fixture-owned copy.
        let mut written: u32 = 0;
        if WriteFile(CONSOLE_HANDLE, buffer, count, &mut written, null_mut()) == 0
            || written != count
        {
            ExitThread(7);
        }
        if WriteFile(
            CONSOLE_HANDLE,
            L2.as_ptr(),
            L2.len() as u32,
            &mut written,
            null_mut(),
        ) == 0
            || written != L2.len() as u32
        {
            ExitThread(8);
        }

        ExitThread(0)
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // SAFETY: `ExitThread` is resolved by the kernel before this image runs.
    unsafe { ExitThread(0xBEEF) }
}
