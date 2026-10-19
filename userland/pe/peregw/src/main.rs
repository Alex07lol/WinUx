//! PROJECT_OS PE32+ fixture (Phase 8, stage 4): Win32 registry write surface.
//!
//! Built for `x86_64-pc-windows-msvc` and linked by `rust-lld` with
//! `/nodefaultlib /entry:_start /subsystem:console`. `RegCreateKeyExA`,
//! `RegSetValueExA` and `RegDeleteValueA` are declared with `raw-dylib`
//! linkage against `advapi32.dll` — `RegCreateKeyExA` is the first
//! nine-argument import (the kernel forwards arguments five to nine through
//! `r10`–`r14`) — while `RegQueryValueExA`/`RegCloseKey` (read-back) and
//! `WriteFile`/`ExitThread` stay on the existing services. The kernel writes
//! every mutation into the config store and persists it through
//! `config::save()`. Every failure exits with a distinct code so the serial
//! log names the broken step.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::ffi::c_void;
use core::ptr::{addr_of, addr_of_mut, null_mut};

/// `HKEY_LOCAL_MACHINE` (WinReg.h).
const HKEY_LOCAL_MACHINE: u64 = 0x8000_0002;

/// `REG_SZ` / `REG_DWORD` (WinNT.h).
const REG_SZ: u32 = 1;
const REG_DWORD: u32 = 4;

/// `REG_CREATED_NEW_KEY` / `REG_OPENED_EXISTING_KEY` (WinReg.h).
const REG_CREATED_NEW_KEY: u32 = 1;
const REG_OPENED_EXISTING_KEY: u32 = 2;

/// `ERROR_FILE_NOT_FOUND` (WinError.h).
const ERROR_FILE_NOT_FOUND: i32 = 2;
/// `ERROR_INVALID_HANDLE` (WinError.h).
const ERROR_INVALID_HANDLE: i32 = 6;
/// `ERROR_INVALID_PARAMETER` (WinError.h).
const ERROR_INVALID_PARAMETER: i32 = 87;

/// Kernel slot handles (`0x1000 + slot`).
const HANDLE_ZERO: u64 = 0x1000;
const HANDLE_ONE: u64 = 0x1001;

/// Console output handle: the kernel's `GetStdHandle(-11)` returns `1`.
const CONSOLE_HANDLE: u64 = 1;

/// The phase summary line written through `WriteFile` (39 bytes).
const L2: &[u8; 39] = b"win32: registry write surface verified\n";

static SESSION_CURRENT: &[u8] = b"Session/Current\0";
static FLAG: &[u8] = b"Flag\0";
static NUMBER: &[u8] = b"Number\0";
static BAD_NAME: &[u8] = b"Bad Name\0";
static MISSING: &[u8] = b"Missing\0";
static PERSISTED: &[u8] = b"Persisted\0";

/// REG_SZ payloads the fixture compares byte by byte (`bytes + NUL`).
const ENABLED: &[u8] = b"enabled\0";
const ON: &[u8] = b"on\0";
const YES: &[u8] = b"yes\0";

/// Destination for full value reads (writable `.data`).
static mut VALUE_BUFFER: [u8; 64] = [0; 64];

// Win64 (`extern "system"`) shapes as the kernel gate implements them:
// `rcx/rdx/r8/r9`, fifth argument captured from `[rsp+0x28]` into `r10`,
// sixth from `[rsp+0x30]` into `r11`, seventh to ninth from
// `[rsp+0x38..0x48]` into `r12`/`r13`/`r14`.
#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn WriteFile(
        handle: u64,
        buffer: *const u8,
        length: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn ExitThread(exit_code: u32) -> !;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegCreateKeyExA(
        hkey: u64,
        subkey: *const u8,
        reserved: u32,
        class: *mut u8,
        options: u32,
        sam_desired: u32,
        security_attributes: *mut c_void,
        result: *mut u64,
        disposition: *mut u32,
    ) -> i32;
    fn RegSetValueExA(
        hkey: u64,
        value_name: *const u8,
        reserved: u32,
        kind: u32,
        data: *const u8,
        count: u32,
    ) -> i32;
    fn RegDeleteValueA(hkey: u64, value_name: *const u8) -> i32;
    fn RegQueryValueExA(
        hkey: u64,
        value_name: *const u8,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        count: *mut u32,
    ) -> i32;
    fn RegCloseKey(hkey: u64) -> i32;
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

fn bytes_eq(buffer: *const u8, expected: &[u8]) -> bool {
    for (index, byte) in expected.iter().enumerate() {
        // SAFETY: `buffer` points to VALUE_BUFFER (64 bytes) and `expected`
        // is shorter than the buffer in every call below.
        if unsafe { core::ptr::read_volatile(buffer.add(index)) } != *byte {
            return false;
        }
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    // SAFETY: the imported functions are resolved by the kernel before this
    // image runs; every pointer argument is a live local, the writable static
    // buffer, a static NUL-terminated name, or NULL. `ExitThread` does not
    // return.
    unsafe {
        // The volatile read keeps the forced relocation observable.
        core::hint::black_box(core::ptr::read_volatile(addr_of!(marker_slot)));

        let mut key: u64 = 0;
        let mut disposition: u32 = 0;

        // 1. First create: the key did not exist, disposition 1, slot 0.
        let status = RegCreateKeyExA(
            HKEY_LOCAL_MACHINE,
            SESSION_CURRENT.as_ptr(),
            0,
            null_mut(),
            0,
            0,
            null_mut(),
            &mut key,
            &mut disposition,
        );
        if status != 0 || disposition != REG_CREATED_NEW_KEY || key != HANDLE_ZERO {
            ExitThread(1);
        }
        let session = key;

        // 2. Set REG_SZ "enabled" (8 bytes including the NUL).
        if RegSetValueExA(session, FLAG.as_ptr(), 0, REG_SZ, ENABLED.as_ptr(), 8) != 0 {
            ExitThread(2);
        }

        // 3. Create again: an entry now lives under the prefix, so the key
        // "exists" (children-only model) — opened existing, second slot.
        let mut second: u64 = 0;
        let status = RegCreateKeyExA(
            HKEY_LOCAL_MACHINE,
            SESSION_CURRENT.as_ptr(),
            0,
            null_mut(),
            0,
            0,
            null_mut(),
            &mut second,
            &mut disposition,
        );
        if status != 0 || disposition != REG_OPENED_EXISTING_KEY || second != HANDLE_ONE {
            ExitThread(3);
        }
        if RegCloseKey(second) != 0 {
            ExitThread(3);
        }

        // 4. Read it back: REG_SZ, 8 bytes including the NUL, exact contents.
        let buffer = addr_of_mut!(VALUE_BUFFER) as *mut u8;
        let mut kind: u32 = 0;
        let mut required: u32 = 64;
        let status = RegQueryValueExA(
            session,
            FLAG.as_ptr(),
            null_mut(),
            &mut kind,
            buffer,
            &mut required,
        );
        if status != 0 || required != 8 || kind != REG_SZ || !bytes_eq(buffer, ENABLED) {
            ExitThread(4);
        }

        // 5. Update to "on" (overwrite semantics).
        if RegSetValueExA(session, FLAG.as_ptr(), 0, REG_SZ, ON.as_ptr(), 3) != 0 {
            ExitThread(5);
        }

        // 6. Size query reflects the update (bytes + NUL).
        required = 0;
        let status = RegQueryValueExA(
            session,
            FLAG.as_ptr(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut required,
        );
        if status != 0 || required != 3 {
            ExitThread(6);
        }

        // 7. REG_DWORD is rejected: only REG_SZ is supported.
        let mut dword: u32 = 7;
        let status = RegSetValueExA(
            session,
            NUMBER.as_ptr(),
            0,
            REG_DWORD,
            addr_of_mut!(dword) as *mut u8,
            4,
        );
        if status != ERROR_INVALID_PARAMETER {
            ExitThread(7);
        }

        // 8. NULL data is rejected (null values not modeled).
        let status = RegSetValueExA(session, FLAG.as_ptr(), 0, REG_SZ, null_mut(), 0);
        if status != ERROR_INVALID_PARAMETER {
            ExitThread(8);
        }

        // 9. Bad handle is rejected.
        let status = RegSetValueExA(1, FLAG.as_ptr(), 0, REG_SZ, ON.as_ptr(), 3);
        if status != ERROR_INVALID_HANDLE {
            ExitThread(9);
        }

        // 10. A value name that joins to an invalid path is rejected.
        let status = RegSetValueExA(session, BAD_NAME.as_ptr(), 0, REG_SZ, ON.as_ptr(), 3);
        if status != ERROR_INVALID_PARAMETER {
            ExitThread(10);
        }

        // 11. NULL value name (default value not modeled) is rejected.
        let status = RegSetValueExA(session, null_mut(), 0, REG_SZ, ON.as_ptr(), 3);
        if status != ERROR_INVALID_PARAMETER {
            ExitThread(11);
        }

        // 12. Deleting a missing value reports not found.
        let status = RegDeleteValueA(session, MISSING.as_ptr());
        if status != ERROR_FILE_NOT_FOUND {
            ExitThread(12);
        }

        // 13. Delete with a bad handle is rejected.
        let status = RegDeleteValueA(1, FLAG.as_ptr());
        if status != ERROR_INVALID_HANDLE {
            ExitThread(13);
        }

        // 14. Delete the only value: the key ceases to exist (children-only).
        if RegDeleteValueA(session, FLAG.as_ptr()) != 0 {
            ExitThread(14);
        }
        required = 64;
        let status = RegQueryValueExA(
            session,
            FLAG.as_ptr(),
            null_mut(),
            null_mut(),
            buffer,
            &mut required,
        );
        if status != ERROR_FILE_NOT_FOUND {
            ExitThread(14);
        }

        // 15. Create again: no children anymore, "created", slot reuse.
        let mut third: u64 = 0;
        let status = RegCreateKeyExA(
            HKEY_LOCAL_MACHINE,
            SESSION_CURRENT.as_ptr(),
            0,
            null_mut(),
            0,
            0,
            null_mut(),
            &mut third,
            &mut disposition,
        );
        if status != 0 || disposition != REG_CREATED_NEW_KEY || third != HANDLE_ONE {
            ExitThread(15);
        }

        // 16. Persist a value for the boot phase to verify after teardown.
        if RegSetValueExA(third, PERSISTED.as_ptr(), 0, REG_SZ, YES.as_ptr(), 4) != 0 {
            ExitThread(16);
        }

        // 17. Close both live handles.
        if RegCloseKey(session) != 0 {
            ExitThread(17);
        }
        if RegCloseKey(third) != 0 {
            ExitThread(17);
        }

        // 18. Summary line; the kernel's counter assertion pins count/bytes.
        let mut written: u32 = 0;
        if WriteFile(
            CONSOLE_HANDLE,
            L2.as_ptr(),
            L2.len() as u32,
            &mut written,
            null_mut(),
        ) == 0
            || written != L2.len() as u32
        {
            ExitThread(18);
        }

        ExitThread(0)
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // SAFETY: `ExitThread` is resolved by the kernel before this image runs.
    unsafe { ExitThread(0xBEEF) }
}
