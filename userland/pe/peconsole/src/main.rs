//! PROJECT_OS PE32+ fixture (Phase 7, stage 2): the native Win32 console
//! surface through a real import table.
//!
//! Built for `x86_64-pc-windows-msvc` and linked by `rust-lld` with
//! `/nodefaultlib /entry:_start /subsystem:console`. The three functions below
//! are declared with `raw-dylib` linkage, so the compiler and linker
//! synthesize a genuine import directory, ILT and IAT against `kernel32.dll`
//! with no import library, MSVC, MinGW or Windows SDK. The kernel resolves
//! each imported name against its own native implementations and patches the
//! IAT to point at its `int 0x80` stub page; nothing here calls the syscall
//! gate directly.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::ffi::c_void;
use core::ptr::{addr_of, null_mut};

/// `STD_OUTPUT_HANDLE` is `((DWORD)-11)`.
const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5;

/// The handle the kernel's `GetStdHandle` returns.
const CONSOLE_HANDLE: u64 = 1;

const M1: &[u8; 44] = b"win32: hello from the Win32 console surface\n";
const M2: &[u8; 32] = b"win32: console surface verified\n";
const M3: &[u8; 32] = b"win32: base relocation mismatch\n";

// Win64 (`extern "system"`) signatures; `HANDLE`/`LPVOID` as raw pointers,
// `BOOL`/`DWORD` as 32-bit values, exactly the shapes the kernel gate
// already implements (`rcx/rdx/r8/r9`).
#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(n_std_handle: u32) -> u64;
    fn WriteConsoleA(
        handle: u64,
        buffer: *const u8,
        length: u32,
        written: *mut u32,
        reserved: *mut c_void,
    ) -> i32;
    fn ExitProcess(exit_code: u32) -> !;
}

/// A read-only marker whose address the relocated slot must match.
#[unsafe(no_mangle)]
pub static reloc_marker: u8 = 0x42;

// An absolute 64-bit reference to `reloc_marker` in `.data`: forces a genuine
// DIR64 base relocation in the real toolchain output (same probe as the
// stage-1 fixture; `.data` is writable, which the kernel loader requires for
// relocation targets).
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
    // image runs; `written` is a live stack slot; `lpReserved` is null as the
    // API contract requires.
    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        if handle != CONSOLE_HANDLE {
            ExitProcess(1);
        }

        let mut written: u32 = 0;
        let ok = WriteConsoleA(
            handle,
            M1.as_ptr(),
            M1.len() as u32,
            &mut written,
            null_mut(),
        );
        if ok == 0 || written != M1.len() as u32 {
            ExitProcess(2);
        }

        let relocated = core::ptr::read_volatile(addr_of!(marker_slot));
        if relocated != addr_of!(reloc_marker) as u64 {
            let _ = WriteConsoleA(
                handle,
                M3.as_ptr(),
                M3.len() as u32,
                &mut written,
                null_mut(),
            );
            ExitProcess(3);
        }

        let ok = WriteConsoleA(
            handle,
            M2.as_ptr(),
            M2.len() as u32,
            &mut written,
            null_mut(),
        );
        if ok == 0 || written != M2.len() as u32 {
            ExitProcess(5);
        }

        ExitProcess(0)
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // SAFETY: `ExitProcess` is resolved by the kernel before this image runs.
    unsafe { ExitProcess(0xBEEF) }
}
