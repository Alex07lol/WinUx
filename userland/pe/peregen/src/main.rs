//! PROJECT_OS PE32+ fixture (Phase 8, stage 5): Win32 registry enumeration
//! and non-recursive deletion.
//!
//! Built for `x86_64-pc-windows-msvc` and linked by `rust-lld` with
//! `/nodefaultlib /entry:_start /subsystem:console`. `RegDeleteKeyA` (the
//! first two-argument import) and `RegEnumKeyExA`/`RegEnumValueA` (the first
//! eight-argument imports: arguments five to eight arrive in `r10`–`r13`)
//! are declared with `raw-dylib` linkage against `advapi32.dll`, while
//! `RegCreateKeyExA`/`RegSetValueExA`/`RegQueryValueExA`/`RegCloseKey`
//! (setup and read-back) and `WriteFile`/`ExitThread` stay on the existing
//! services. Every failure exits with a distinct code so the serial log
//! names the broken step; the kernel's counter assertion pins the call
//! counts, the summary length and the persisted cleanup.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::ffi::c_void;
use core::ptr::{addr_of, addr_of_mut, null_mut};

/// `HKEY_LOCAL_MACHINE` (WinReg.h).
const HKEY_LOCAL_MACHINE: u64 = 0x8000_0002;

/// `REG_SZ` (WinNT.h).
const REG_SZ: u32 = 1;

/// `REG_CREATED_NEW_KEY` / `REG_OPENED_EXISTING_KEY` (WinReg.h).
const REG_CREATED_NEW_KEY: u32 = 1;
const REG_OPENED_EXISTING_KEY: u32 = 2;

/// `ERROR_FILE_NOT_FOUND` (WinError.h).
const ERROR_FILE_NOT_FOUND: i32 = 2;
/// `ERROR_ACCESS_DENIED` (WinError.h).
const ERROR_ACCESS_DENIED: i32 = 5;
/// `ERROR_INVALID_HANDLE` (WinError.h).
const ERROR_INVALID_HANDLE: i32 = 6;
/// `ERROR_INVALID_PARAMETER` (WinError.h).
const ERROR_INVALID_PARAMETER: i32 = 87;
/// `ERROR_MORE_DATA` (WinError.h).
const ERROR_MORE_DATA: i32 = 234;
/// `ERROR_NO_MORE_ITEMS` (WinError.h).
const ERROR_NO_MORE_ITEMS: i32 = 259;

/// Kernel slot handles (`0x1000 + slot`).
const HANDLE_ZERO: u64 = 0x1000;
const HANDLE_ONE: u64 = 0x1001;
const HANDLE_TWO: u64 = 0x1002;
const HANDLE_THREE: u64 = 0x1003;

/// Console output handle: the kernel's `GetStdHandle(-11)` returns `1`.
const CONSOLE_HANDLE: u64 = 1;

/// The phase summary line written through `WriteFile` (45 bytes).
const SUMMARY: &[u8; 45] = b"win32: registry enumeration surface verified\n";

static CURRENT: &[u8] = b"Session/Current\0";
static SESSION: &[u8] = b"Session\0";
static ALPHA_PATH: &[u8] = b"Session/Current/Enum/Alpha\0";
static BETA_PATH: &[u8] = b"Session/Current/Enum/Beta\0";
static ENUM_PATH: &[u8] = b"Session/Current/Enum\0";
static KEEP: &[u8] = b"Keep\0";
static REMOVED: &[u8] = b"Removed\0";
static ITEM: &[u8] = b"Item\0";
static ENUM_NAME: &[u8] = b"Enum\0";
static ALPHA_NAME: &[u8] = b"Alpha\0";
static BETA_NAME: &[u8] = b"Beta\0";
static GAMMA_NAME: &[u8] = b"Gamma\0";
static SLASHED_ALPHA: &[u8] = b"Enum/Alpha/Item\0";
static SLASHED_BETA: &[u8] = b"Enum/Beta/Item\0";

/// REG_SZ payloads the fixture compares byte by byte (`bytes + NUL`).
const ONE: &[u8] = b"one\0";
const TWO: &[u8] = b"two\0";
const ALPHA_DATA: &[u8] = b"alpha\0";
const BETA_DATA: &[u8] = b"beta\0";

/// Writable buffers: enumeration names (64) and value data (64).
static mut NAME_BUFFER: [u8; 64] = [0; 64];
static mut DATA_BUFFER: [u8; 64] = [0; 64];
static mut RESERVED: u32 = 1;

// Win64 (`extern "system"`) shapes as the kernel gate implements them:
// `rcx/rdx/r8/r9` plus arguments five to eight from `[rsp+0x28..0x40]` into
// `r10`/`r11`/`r12`/`r13`.
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
    fn RegQueryValueExA(
        hkey: u64,
        value_name: *const u8,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        count: *mut u32,
    ) -> i32;
    fn RegDeleteKeyA(hkey: u64, subkey: *const u8) -> i32;
    fn RegEnumKeyExA(
        hkey: u64,
        index: u32,
        name: *mut u8,
        name_size: *mut u32,
        reserved: *mut u32,
        class: *mut u8,
        class_size: *mut u32,
        last_write: *mut c_void,
    ) -> i32;
    fn RegEnumValueA(
        hkey: u64,
        index: u32,
        name: *mut u8,
        name_size: *mut u32,
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
// rule (same probe as the earlier fixtures).
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
        // SAFETY: `buffer` points to one of the 64-byte static buffers and
        // `expected` is shorter than the buffer in every call below.
        if unsafe { core::ptr::read_volatile(buffer.add(index)) } != *byte {
            return false;
        }
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    // SAFETY: the imported functions are resolved by the kernel before this
    // image runs; every pointer argument is a live local, a writable static
    // buffer, a static NUL-terminated name, or NULL. `ExitThread` does not
    // return.
    unsafe {
        // The volatile read keeps the forced relocation observable.
        core::hint::black_box(core::ptr::read_volatile(addr_of!(marker_slot)));

        let mut current: u64 = 0;
        let mut alpha: u64 = 0;
        let mut beta: u64 = 0;
        let mut enumeration: u64 = 0;
        let mut disposition: u32 = 0;
        let name = addr_of_mut!(NAME_BUFFER) as *mut u8;
        let data = addr_of_mut!(DATA_BUFFER) as *mut u8;

        // 1. Create Current: created, slot 0.
        let status = RegCreateKeyExA(
            HKEY_LOCAL_MACHINE,
            CURRENT.as_ptr(),
            0,
            null_mut(),
            0,
            0,
            null_mut(),
            &mut current,
            &mut disposition,
        );
        if status != 0 || disposition != REG_CREATED_NEW_KEY || current != HANDLE_ZERO {
            ExitThread(1);
        }

        // 2. Create Enum/Alpha: created, slot 1.
        let status = RegCreateKeyExA(
            HKEY_LOCAL_MACHINE,
            ALPHA_PATH.as_ptr(),
            0,
            null_mut(),
            0,
            0,
            null_mut(),
            &mut alpha,
            &mut disposition,
        );
        if status != 0 || disposition != REG_CREATED_NEW_KEY || alpha != HANDLE_ONE {
            ExitThread(2);
        }

        // 3. Create Enum/Beta: created, slot 2.
        let status = RegCreateKeyExA(
            HKEY_LOCAL_MACHINE,
            BETA_PATH.as_ptr(),
            0,
            null_mut(),
            0,
            0,
            null_mut(),
            &mut beta,
            &mut disposition,
        );
        if status != 0 || disposition != REG_CREATED_NEW_KEY || beta != HANDLE_TWO {
            ExitThread(3);
        }

        // 4-7. Four direct values in insertion order: Keep, Removed,
        // Alpha/Item, Beta/Item (before Enum is opened, so it "exists").
        if RegSetValueExA(current, KEEP.as_ptr(), 0, REG_SZ, ONE.as_ptr(), 4) != 0 {
            ExitThread(4);
        }
        if RegSetValueExA(current, REMOVED.as_ptr(), 0, REG_SZ, TWO.as_ptr(), 4) != 0 {
            ExitThread(5);
        }
        if RegSetValueExA(alpha, ITEM.as_ptr(), 0, REG_SZ, ALPHA_DATA.as_ptr(), 6) != 0 {
            ExitThread(6);
        }
        if RegSetValueExA(beta, ITEM.as_ptr(), 0, REG_SZ, BETA_DATA.as_ptr(), 5) != 0 {
            ExitThread(7);
        }

        // 8. Create Enum: lazy keys already have entries, opened existing.
        let status = RegCreateKeyExA(
            HKEY_LOCAL_MACHINE,
            ENUM_PATH.as_ptr(),
            0,
            null_mut(),
            0,
            0,
            null_mut(),
            &mut enumeration,
            &mut disposition,
        );
        if status != 0 || disposition != REG_OPENED_EXISTING_KEY || enumeration != HANDLE_THREE {
            ExitThread(8);
        }

        // 9. Read Keep back (the only query of this fixture).
        let mut kind: u32 = 0;
        let mut count: u32 = 64;
        let status = RegQueryValueExA(
            current,
            KEEP.as_ptr(),
            null_mut(),
            &mut kind,
            data,
            &mut count,
        );
        if status != 0 || count != 4 || kind != REG_SZ || !bytes_eq(data, ONE) {
            ExitThread(9);
        }

        // 10. Enumerate child keys of Enum in insertion order.
        let mut size: u32 = 64;
        let status = RegEnumKeyExA(
            enumeration,
            0,
            name,
            &mut size,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
        );
        if status != 0 || size != 5 || !bytes_eq(name, ALPHA_NAME) {
            ExitThread(10);
        }

        // 11. Second child.
        size = 64;
        let status = RegEnumKeyExA(
            enumeration,
            1,
            name,
            &mut size,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
        );
        if status != 0 || size != 4 || !bytes_eq(name, BETA_NAME) {
            ExitThread(11);
        }

        // 12. Past the end.
        size = 64;
        if RegEnumKeyExA(
            enumeration,
            2,
            name,
            &mut size,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
        ) != ERROR_NO_MORE_ITEMS
        {
            ExitThread(12);
        }

        // 13. Short name buffer: required size includes the NUL.
        size = 3;
        if RegEnumKeyExA(
            enumeration,
            0,
            name,
            &mut size,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
        ) != ERROR_MORE_DATA
            || size != 6
        {
            ExitThread(13);
        }

        // 14. The same item succeeds once the buffer fits; size excludes NUL.
        size = 64;
        if RegEnumKeyExA(
            enumeration,
            0,
            name,
            &mut size,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
        ) != 0
            || size != 5
            || !bytes_eq(name, ALPHA_NAME)
        {
            ExitThread(14);
        }

        // 15. A non-NULL reserved pointer is rejected.
        size = 64;
        let status = RegEnumKeyExA(
            enumeration,
            0,
            name,
            &mut size,
            addr_of_mut!(RESERVED),
            null_mut(),
            null_mut(),
            null_mut(),
        );
        if status != ERROR_INVALID_PARAMETER {
            ExitThread(15);
        }

        // 16. A zero name capacity is rejected.
        size = 0;
        if RegEnumKeyExA(
            enumeration,
            0,
            name,
            &mut size,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
        ) != ERROR_INVALID_PARAMETER
        {
            ExitThread(16);
        }

        // 17. Values: name/type only (NULL data, NULL data size). The store
        // order is canonical ascending byte order, so the slash-containing
        // tails come first.
        let mut name_size: u32 = 64;
        let mut type_out: u32 = 0;
        let status = RegEnumValueA(
            current,
            0,
            name,
            &mut name_size,
            null_mut(),
            &mut type_out,
            null_mut(),
            null_mut(),
        );
        if status != 0 || name_size != 15 || type_out != REG_SZ || !bytes_eq(name, SLASHED_ALPHA) {
            ExitThread(17);
        }

        // 18. Full value read of the second flattened entry: "beta\0".
        name_size = 64;
        count = 64;
        let status = RegEnumValueA(
            current,
            1,
            name,
            &mut name_size,
            null_mut(),
            &mut type_out,
            data,
            &mut count,
        );
        if status != 0
            || name_size != 14
            || count != 5
            || type_out != REG_SZ
            || !bytes_eq(name, SLASHED_BETA)
            || !bytes_eq(data, BETA_DATA)
        {
            ExitThread(18);
        }

        // 19. Size query for a plain direct value, "Keep".
        name_size = 64;
        count = 0;
        let status = RegEnumValueA(
            current,
            2,
            name,
            &mut name_size,
            null_mut(),
            &mut type_out,
            null_mut(),
            &mut count,
        );
        if status != 0
            || name_size != 4
            || count != 4
            || type_out != REG_SZ
            || !bytes_eq(name, KEEP)
        {
            ExitThread(19);
        }

        // 20. Past the last value.
        name_size = 64;
        count = 64;
        let status = RegEnumValueA(
            current,
            4,
            name,
            &mut name_size,
            null_mut(),
            &mut type_out,
            data,
            &mut count,
        );
        if status != ERROR_NO_MORE_ITEMS {
            ExitThread(20);
        }

        // 21. Short name buffer: 234 with the required name size.
        name_size = 1;
        let status = RegEnumValueA(
            alpha,
            0,
            name,
            &mut name_size,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
        );
        if status != ERROR_MORE_DATA || name_size != 5 {
            ExitThread(21);
        }

        // 22. Full value enumeration on the child handle.
        name_size = 64;
        count = 64;
        let status = RegEnumValueA(
            alpha,
            0,
            name,
            &mut name_size,
            null_mut(),
            &mut type_out,
            data,
            &mut count,
        );
        if status != 0
            || name_size != 4
            || count != 6
            || !bytes_eq(name, ITEM)
            || !bytes_eq(data, ALPHA_DATA)
        {
            ExitThread(22);
        }

        // 23. Deleting a key with child keys is denied.
        if RegDeleteKeyA(current, ENUM_NAME.as_ptr()) != ERROR_ACCESS_DENIED {
            ExitThread(23);
        }

        // 24. Removing the Alpha key deletes its direct value with it.
        if RegDeleteKeyA(enumeration, ALPHA_NAME.as_ptr()) != 0 {
            ExitThread(24);
        }

        // 25. Alpha no longer exists as a key.
        if RegDeleteKeyA(enumeration, ALPHA_NAME.as_ptr()) != ERROR_FILE_NOT_FOUND {
            ExitThread(25);
        }

        // 26. Remove Beta the same way.
        if RegDeleteKeyA(enumeration, BETA_NAME.as_ptr()) != 0 {
            ExitThread(26);
        }

        // 27. Gamma was created nowhere (lazy keys): not found.
        if RegDeleteKeyA(enumeration, GAMMA_NAME.as_ptr()) != ERROR_FILE_NOT_FOUND {
            ExitThread(27);
        }

        // 28. An invalid handle is rejected (and does not touch the store).
        if RegDeleteKeyA(1, ITEM.as_ptr()) != ERROR_INVALID_HANDLE {
            ExitThread(28);
        }

        // 29. Enum disappeared with its last child; deleting it is not found.
        if RegDeleteKeyA(current, ENUM_NAME.as_ptr()) != ERROR_FILE_NOT_FOUND {
            ExitThread(29);
        }

        // 30. Session still has the Current child key: denied.
        if RegDeleteKeyA(HKEY_LOCAL_MACHINE, SESSION.as_ptr()) != ERROR_ACCESS_DENIED {
            ExitThread(30);
        }

        // 31. Current's only entries are direct values, so deleting the key
        // removes them all (and Session disappears with its last child).
        if RegDeleteKeyA(HKEY_LOCAL_MACHINE, CURRENT.as_ptr()) != 0 {
            ExitThread(31);
        }

        // 32. The deleted handle is stale: enumeration ends immediately.
        // (Still the eighth RegEnumKeyExA call.)
        size = 64;
        if RegEnumKeyExA(
            enumeration,
            0,
            name,
            &mut size,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
        ) != ERROR_NO_MORE_ITEMS
        {
            ExitThread(32);
        }

        // 33. All four handles still close cleanly.
        if RegCloseKey(current) != 0
            || RegCloseKey(alpha) != 0
            || RegCloseKey(beta) != 0
            || RegCloseKey(enumeration) != 0
        {
            ExitThread(33);
        }

        // 34. Summary line; the kernel's counter assertion pins count/bytes.
        let mut written: u32 = 0;
        if WriteFile(
            CONSOLE_HANDLE,
            SUMMARY.as_ptr(),
            SUMMARY.len() as u32,
            &mut written,
            null_mut(),
        ) == 0
            || written != SUMMARY.len() as u32
        {
            ExitThread(34);
        }

        ExitThread(0)
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // SAFETY: `ExitThread` is resolved by the kernel before this image runs.
    unsafe { ExitThread(0xBEEF) }
}
