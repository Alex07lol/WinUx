//! PROJECT_OS PE32+ fixture (Phase 7, stage 3): the native Win32 program
//! basics through a real import table.
//!
//! Built for `x86_64-pc-windows-msvc` and linked by `rust-lld` with
//! `/nodefaultlib /entry:_start /subsystem:console`. The functions below are
//! declared with `raw-dylib` linkage, so the compiler and linker synthesize a
//! genuine import directory, ILT and IAT against `kernel32.dll` with no
//! import library, MSVC, MinGW or Windows SDK. The kernel resolves each
//! imported name against its own native implementations and patches the IAT
//! to point at its `int 0x80` stub page; nothing here calls the syscall gate
//! directly. Every failure exits with a distinct code so the serial log names
//! the broken step.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::ffi::c_void;
use core::ptr::{addr_of, null_mut};

/// `STD_OUTPUT_HANDLE` is `((DWORD)-11)`.
const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5;

/// The handle the kernel's `GetStdHandle` returns.
const CONSOLE_HANDLE: u64 = 1;

/// `ERROR_INVALID_HANDLE` (WinError.h).
const ERROR_INVALID_HANDLE: u32 = 6;
/// `ERROR_INVALID_PARAMETER` (WinError.h).
const ERROR_INVALID_PARAMETER: u32 = 87;

const L1: &[u8; 28] = b"win32: hello from WriteFile\n";
const L2: &[u8; 31] = b"win32: program basics verified\n";

/// The command line the loader writes for this module, terminator included.
const EXPECTED_COMMAND_LINE: &[u8; 19] = b"/boot/pebasics.exe\0";

// Win64 (`extern "system"`) signatures; `HANDLE`/`LPVOID` as raw pointers,
// `BOOL`/`DWORD` as 32-bit values, exactly the shapes the kernel gate
// implements (`rcx/rdx/r8/r9`, fifth argument at `[rsp+0x28]`).
#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(n_std_handle: u32) -> u64;
    fn WriteFile(
        handle: u64,
        buffer: *const u8,
        length: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn GetLastError() -> u32;
    fn SetLastError(code: u32);
    fn ExitThread(exit_code: u32) -> !;
    fn GetCurrentProcessId() -> u32;
    fn GetCurrentThreadId() -> u32;
    fn GetTickCount() -> u32;
    fn GetCommandLineA() -> *const u8;
}

/// A read-only marker whose address the relocated slot must match.
#[unsafe(no_mangle)]
pub static reloc_marker: u8 = 0x42;

// An absolute 64-bit reference to `reloc_marker` in `.data`: forces a genuine
// DIR64 base relocation in the real toolchain output, so the image meets the
// loader's "a rebase must be possible" rule (same probe as the stage-1 and
// stage-2 fixtures).
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
    // image runs; `written` is a live stack slot; every pointer argument is a
    // live local or a static. `ExitThread` does not return.
    unsafe {
        // The volatile read keeps the forced relocation observable.
        core::hint::black_box(core::ptr::read_volatile(addr_of!(marker_slot)));

        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        if handle != CONSOLE_HANDLE {
            ExitThread(1);
        }

        SetLastError(42);
        if GetLastError() != 42 {
            ExitThread(2);
        }

        let mut written: u32 = 0;
        let ok = WriteFile(
            handle,
            L1.as_ptr(),
            L1.len() as u32,
            &mut written,
            null_mut(),
        );
        if ok == 0 || written != L1.len() as u32 {
            ExitThread(3);
        }
        // A successful call does not clear the last-error value.
        if GetLastError() != 42 {
            ExitThread(4);
        }

        let ok = WriteFile(0, L1.as_ptr(), L1.len() as u32, &mut written, null_mut());
        if ok != 0 || GetLastError() != ERROR_INVALID_HANDLE {
            ExitThread(5);
        }

        let ok = WriteFile(
            handle,
            L1.as_ptr(),
            L1.len() as u32,
            &mut written,
            core::ptr::dangling_mut::<c_void>(),
        );
        if ok != 0 || GetLastError() != ERROR_INVALID_PARAMETER {
            ExitThread(6);
        }

        if GetCurrentProcessId() != 1 || GetCurrentThreadId() != 1 {
            ExitThread(7);
        }

        let t1 = GetTickCount();
        let t2 = GetTickCount();
        if (t2.wrapping_sub(t1) as i32) < 0 {
            ExitThread(8);
        }

        let line = GetCommandLineA();
        let mut offset = 0usize;
        loop {
            let actual = core::ptr::read(line.add(offset));
            if actual != EXPECTED_COMMAND_LINE[offset] {
                ExitThread(9);
            }
            if actual == 0 {
                break;
            }
            offset += 1;
            if offset >= EXPECTED_COMMAND_LINE.len() {
                ExitThread(9);
            }
        }

        SetLastError(0);
        if GetLastError() != 0 {
            ExitThread(10);
        }

        let ok = WriteFile(
            handle,
            L2.as_ptr(),
            L2.len() as u32,
            &mut written,
            null_mut(),
        );
        if ok == 0 || written != L2.len() as u32 {
            ExitThread(11);
        }

        ExitThread(0)
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // SAFETY: `ExitThread` is resolved by the kernel before this image runs.
    unsafe { ExitThread(0xBEEF) }
}
