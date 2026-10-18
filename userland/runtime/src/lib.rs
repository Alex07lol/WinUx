//! PROJECT_OS userland runtime: syscall shims, program entry and panic exit.
//!
//! The syscall ABI is the kernel's `int 0x80` gate: `rax` selects the service
//! (1 = write, 2 = exit) and the arguments travel in `rcx`, `rdx`, `r8`, `r9`.
//! The kernel restores every register except `rax` (the result), so the
//! argument registers are declared as read-writes with ignored outputs.

#![no_std]
#![forbid(unsafe_op_in_unsafe_fn)]

use core::arch::asm;

/// `write(fd, buffer, length)`; returns the number of bytes written.
pub fn sys_write(fd: u64, buffer: &[u8]) -> u64 {
    let written: u64;
    // SAFETY: int 0x80 is the kernel's documented syscall gate. It preserves
    // the stack, the kernel validates every buffer page before reading it,
    // and the result arrives in rax.
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") 1u64 => written,
            inlateout("rcx") fd => _,
            inlateout("rdx") buffer.as_ptr() as u64 => _,
            inlateout("r8") buffer.len() as u64 => _,
            lateout("r9") _,
            options(nostack),
        );
    }
    written
}

/// `exit(code)`; the kernel ends the calling thread inside the gate.
pub fn sys_exit(code: u64) -> ! {
    // SAFETY: the gate kills the calling thread on SYS_EXIT, so execution
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

/// Program entry: runs `program`, then exits 0.
pub fn entry(program: fn()) -> ! {
    program();
    sys_exit(0)
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    sys_exit(0xBEEF)
}
