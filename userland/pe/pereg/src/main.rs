//! PROJECT_OS PE32+ fixture (Phase 8, stage 3): Win32 registry read surface.
//!
//! Built for `x86_64-pc-windows-msvc` and linked by `rust-lld` with
//! `/nodefaultlib /entry:_start /subsystem:console`. `RegOpenKeyExA`,
//! `RegQueryValueExA` and `RegCloseKey` are declared with `raw-dylib`
//! linkage against `advapi32.dll` — the first second-DLL allowlist entries and
//! the first six-argument stub (`RegQueryValueExA` forwards `lpData` through
//! `r10` and `lpcbData` through `r11`) — while `WriteFile`/`ExitThread` stay
//! on `kernel32.dll`. The kernel resolves every name against its own native
//! implementation over the config store and patches the IAT to its
//! `int 0x80` stubs. Every failure exits with a distinct code so the serial
//! log names the broken step.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::ffi::c_void;
use core::ptr::{addr_of, addr_of_mut, null_mut};

/// `HKEY_LOCAL_MACHINE` (WinReg.h).
const HKEY_LOCAL_MACHINE: u64 = 0x8000_0002;

/// `KEY_READ` (WinReg.h).
const KEY_READ: u32 = 0x20019;

/// `ERROR_FILE_NOT_FOUND` (WinError.h).
const ERROR_FILE_NOT_FOUND: i32 = 2;
/// `ERROR_TOO_MANY_OPEN_FILES` (WinError.h), our slot-exhaustion code.
const ERROR_TOO_MANY_OPEN_FILES: i32 = 4;
/// `ERROR_INVALID_HANDLE` (WinError.h).
const ERROR_INVALID_HANDLE: i32 = 6;
/// `ERROR_INVALID_PARAMETER` (WinError.h).
const ERROR_INVALID_PARAMETER: i32 = 87;
/// `ERROR_MORE_DATA` (WinError.h).
const ERROR_MORE_DATA: i32 = 234;
/// `REG_SZ` (WinNT.h).
const REG_SZ: u32 = 1;

/// Kernel slot handles (`0x1000 + slot`).
const HANDLE_ZERO: u64 = 0x1000;
const HANDLE_ONE: u64 = 0x1001;
const HANDLE_TWO: u64 = 0x1002;
const HANDLE_THREE: u64 = 0x1003;

/// Console output handle: the kernel's `GetStdHandle(-11)` returns `1`.
const CONSOLE_HANDLE: u64 = 1;

/// The phase summary line written through `WriteFile` (33 bytes).
const L2: &[u8; 33] = b"win32: registry surface verified\n";

static SYSTEM: &[u8] = b"System\0";
static MISSING: &[u8] = b"Missing\0";
static NAME: &[u8] = b"Name\0";
static THEME_PATH: &[u8] = b"Display/Theme\0";

/// REG_SZ contents the fixture compares byte by byte (`bytes + NUL`).
const NAME_BYTES: &[u8] = b"PROJECT_OS\0";
const THEME_BYTES: &[u8] = b"classic\0";

/// Destination for full value reads (writable `.data`).
static mut VALUE_BUFFER: [u8; 64] = [0; 64];

// Win64 (`extern "system"`) shapes as the kernel gate implements them:
// `rcx/rdx/r8/r9`, fifth argument captured from `[rsp+0x28]` into `r10`,
// sixth from `[rsp+0x30]` into `r11`.
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
    fn RegOpenKeyExA(
        hkey: u64,
        subkey: *const u8,
        options: u32,
        sam_desired: u32,
        result: *mut u64,
    ) -> i32;
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

        // 1. Bad handle: the error is returned and *phkResult is zeroed.
        let status = RegOpenKeyExA(1, SYSTEM.as_ptr(), 0, KEY_READ, &mut key);
        if status != ERROR_INVALID_HANDLE || key != 0 {
            ExitThread(1);
        }

        // 2. Nonzero ulOptions is rejected (REG_OPTION_OPEN_LINK unsupported).
        let status = RegOpenKeyExA(HKEY_LOCAL_MACHINE, SYSTEM.as_ptr(), 1, 0, &mut key);
        if status != ERROR_INVALID_PARAMETER || key != 0 {
            ExitThread(2);
        }

        // 3. No store entry under "Missing": the key does not exist.
        let status = RegOpenKeyExA(HKEY_LOCAL_MACHINE, MISSING.as_ptr(), 0, 0, &mut key);
        if status != ERROR_FILE_NOT_FOUND || key != 0 {
            ExitThread(3);
        }

        // 4. "System" exists through "System/Name"; slot 0.
        let status = RegOpenKeyExA(HKEY_LOCAL_MACHINE, SYSTEM.as_ptr(), 0, KEY_READ, &mut key);
        if status != 0 || key != HANDLE_ZERO {
            ExitThread(4);
        }
        let system = key;

        // 5. Missing value under a live key.
        let mut required: u32 = 0;
        let status = RegQueryValueExA(
            system,
            MISSING.as_ptr(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut required,
        );
        if status != ERROR_FILE_NOT_FOUND {
            ExitThread(5);
        }

        // 6. Size query: NULL data returns the required size (bytes + NUL).
        required = 0;
        let status = RegQueryValueExA(
            system,
            NAME.as_ptr(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut required,
        );
        if status != 0 || required != 11 {
            ExitThread(6);
        }

        // 7. Short buffer: ERROR_MORE_DATA, required size, data untouched.
        let buffer = addr_of_mut!(VALUE_BUFFER) as *mut u8;
        core::ptr::write_volatile(buffer, 0x5A);
        required = 4;
        let status = RegQueryValueExA(
            system,
            NAME.as_ptr(),
            null_mut(),
            null_mut(),
            buffer,
            &mut required,
        );
        if status != ERROR_MORE_DATA || required != 11 || core::ptr::read_volatile(buffer) != 0x5A {
            ExitThread(7);
        }

        // 8. Full read: REG_SZ, 11 bytes including the NUL, exact contents.
        let mut kind: u32 = 0;
        required = 64;
        let status = RegQueryValueExA(
            system,
            NAME.as_ptr(),
            null_mut(),
            &mut kind,
            buffer,
            &mut required,
        );
        if status != 0 || required != 11 || kind != REG_SZ || !bytes_eq(buffer, NAME_BYTES) {
            ExitThread(8);
        }

        // 9. Non-NULL lpReserved is rejected (documented "must be NULL").
        let mut reserved: u32 = 0;
        required = 64;
        let status = RegQueryValueExA(
            system,
            NAME.as_ptr(),
            &mut reserved,
            &mut kind,
            buffer,
            &mut required,
        );
        if status != ERROR_INVALID_PARAMETER {
            ExitThread(9);
        }

        // 10. NULL lpcbData is rejected (our contract).
        let status = RegQueryValueExA(
            system,
            NAME.as_ptr(),
            null_mut(),
            null_mut(),
            buffer,
            null_mut(),
        );
        if status != ERROR_INVALID_PARAMETER {
            ExitThread(10);
        }

        // 11. A NULL subkey refreshes HKLM and returns the same handle.
        let status = RegOpenKeyExA(HKEY_LOCAL_MACHINE, null_mut(), 0, 0, &mut key);
        if status != 0 || key != HKEY_LOCAL_MACHINE {
            ExitThread(11);
        }

        // 12. The root reads a path-shaped value name in one call.
        required = 64;
        let status = RegQueryValueExA(
            HKEY_LOCAL_MACHINE,
            THEME_PATH.as_ptr(),
            null_mut(),
            &mut kind,
            buffer,
            &mut required,
        );
        if status != 0 || required != 8 || kind != REG_SZ || !bytes_eq(buffer, THEME_BYTES) {
            ExitThread(12);
        }

        // 13. Closing a predefined root is a no-op success.
        if RegCloseKey(HKEY_LOCAL_MACHINE) != 0 {
            ExitThread(13);
        }

        // 14. Three more "System" opens take slots 1..3.
        let mut key_one: u64 = 0;
        let mut key_two: u64 = 0;
        let mut key_three: u64 = 0;
        if RegOpenKeyExA(HKEY_LOCAL_MACHINE, SYSTEM.as_ptr(), 0, 0, &mut key_one) != 0
            || key_one != HANDLE_ONE
        {
            ExitThread(14);
        }
        if RegOpenKeyExA(HKEY_LOCAL_MACHINE, SYSTEM.as_ptr(), 0, 0, &mut key_two) != 0
            || key_two != HANDLE_TWO
        {
            ExitThread(14);
        }
        if RegOpenKeyExA(HKEY_LOCAL_MACHINE, SYSTEM.as_ptr(), 0, 0, &mut key_three) != 0
            || key_three != HANDLE_THREE
        {
            ExitThread(14);
        }

        // 15. Fifth concurrent open: slot exhaustion, result zeroed.
        let status = RegOpenKeyExA(HKEY_LOCAL_MACHINE, SYSTEM.as_ptr(), 0, 0, &mut key);
        if status != ERROR_TOO_MANY_OPEN_FILES || key != 0 {
            ExitThread(15);
        }

        // 16. Close the first handle.
        if RegCloseKey(system) != 0 {
            ExitThread(16);
        }

        // 17. Double close is rejected.
        if RegCloseKey(system) != ERROR_INVALID_HANDLE {
            ExitThread(17);
        }

        // 18. Reopening reuses the freed slot 0.
        let status = RegOpenKeyExA(HKEY_LOCAL_MACHINE, SYSTEM.as_ptr(), 0, 0, &mut key);
        if status != 0 || key != HANDLE_ZERO {
            ExitThread(18);
        }

        // 19. Close the reopened handle and the three extras.
        if RegCloseKey(key) != 0 {
            ExitThread(19);
        }
        if RegCloseKey(key_one) != 0 {
            ExitThread(19);
        }
        if RegCloseKey(key_two) != 0 {
            ExitThread(19);
        }
        if RegCloseKey(key_three) != 0 {
            ExitThread(19);
        }

        // 20. Summary line; the kernel's counter assertion pins count/bytes.
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
            ExitThread(20);
        }

        ExitThread(0)
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // SAFETY: `ExitThread` is resolved by the kernel before this image runs.
    unsafe { ExitThread(0xBEEF) }
}
