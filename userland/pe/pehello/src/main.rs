//! PROJECT_OS PE32+ fixture: proves the kernel PE loader at runtime.
//!
//! Built for `x86_64-pc-windows-msvc` and linked by the `rust-lld` shipped in
//! the pinned Rust toolchain with `/nodefaultlib /entry:_start
//! /subsystem:console`: a genuine PE32+ console image with no MSVC, MinGW,
//! Windows SDK or CRT behind it. The only services it uses are the kernel's
//! `int 0x80` gate (`rax` = service, `rcx/rdx/r8/r9` = arguments), which was
//! already shaped Windows-`fastcall`-like for the later API stages.

#![no_std]
#![no_main]

use core::arch::{asm, global_asm};

/// A read-only marker whose address the relocated slot must match.
#[unsafe(no_mangle)]
pub static reloc_marker: u8 = 0x42;

// An absolute 64-bit reference to `reloc_marker` in `.data`. Rust const-eval
// forbids initializing a static with a pointer-to-integer value, so the slot
// is emitted by the assembler: this forces a genuine DIR64 base relocation in
// the real toolchain output, and `.data` is writable, which the kernel loader
// requires for relocation targets.
global_asm!(
    ".section .data",
    ".globl marker_slot",
    ".p2align 3",
    "marker_slot:",
    ".quad reloc_marker",
);

/// `rax = 1`, `rcx = fd`, `rdx = buffer`, `r8 = length`.
fn sys_write(buffer: &[u8]) {
    // SAFETY: int 0x80 is the kernel's documented syscall gate. It preserves
    // the stack, the kernel validates every buffer page before reading it,
    // and the result arrives in rax.
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") 1u64 => _,
            inlateout("rcx") 1u64 => _,
            inlateout("rdx") buffer.as_ptr() as u64 => _,
            inlateout("r8") buffer.len() as u64 => _,
            lateout("r9") _,
            options(nostack),
        );
    }
}

/// `rax = 2`, `rcx = exit code`; the kernel ends the calling thread.
fn sys_exit(code: u64) -> ! {
    // SAFETY: the gate kills the calling thread on service 2, so execution
    // never resumes after this instruction.
    unsafe {
        asm!(
            "int 0x80",
            in("rax") 2u64,
            in("rcx") code,
            options(noreturn, nostack),
        );
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    sys_write(b"pe: hello from a PE32+ image via int 0x80\n");

    // SAFETY: `marker_slot` is a 64-bit slot in `.data`; the kernel loader
    // base-relocates it before this code ever runs, so the read is of an
    // initialized value. `reloc_marker`'s runtime address is computed with a
    // rip-relative `lea` that needs no relocation.
    let relocated = unsafe { core::ptr::read_volatile(core::ptr::addr_of!(marker_slot)) };
    let runtime = core::ptr::addr_of!(reloc_marker) as u64;
    if relocated == runtime {
        sys_write(b"pe: base relocation verified\n");
        sys_exit(0)
    } else {
        sys_write(b"pe: base relocation mismatch\n");
        sys_exit(1)
    }
}

unsafe extern "C" {
    static marker_slot: u64;
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    sys_exit(0xBEEF)
}
