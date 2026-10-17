//! The ring-3 syscall boundary (`int 0x80`) and user-fault handling (Phase 5b).
//!
//! ABI (Windows-style argument registers chosen so the later PE compatibility
//! layer can reuse the same gate):
//!
//! * `rax` — service number
//! * `rcx`, `rdx`, `r8`, `r9` — arguments
//! * `rax` — return value / status after the call
//!
//! Services:
//!
//! * `1 write(rcx = fd, rdx = buffer, r8 = length)` → bytes written; only
//!   fd 1 (console) and lengths of at most [`MAX_WRITE`] are accepted.
//! * `2 exit(rcx = code)` — the calling thread ends; the value is recorded.
//!
//! Every user pointer is validated against the live page tables before the
//! kernel reads it (mapped, user-accessible, canonical, bounded span). A
//! violation, an unknown service, or a fault taken from CPL3 kills the calling
//! thread (`sched::exit`) and the kernel continues — that is the isolation
//! property this milestone proves.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::idt::InterruptFrame;
use crate::arch::x86_64::read_cr2;
use crate::sched;
use crate::serial;
use crate::vm;

pub const SYS_WRITE: u64 = 1;
pub const SYS_EXIT: u64 = 2;

/// Maximum accepted console write, small enough to bound the kernel copy.
pub const MAX_WRITE: u64 = 4096;

/// Highest address the kernel accepts from user space (canonical user half:
/// PML4 entries 0..256).
const USER_LIMIT: u64 = 0x0000_8000_0000_0000;

/// Last exit code passed to `SYS_EXIT` (single user thread in this milestone).
pub static LAST_EXIT_CODE: AtomicU64 = AtomicU64::new(u64::MAX);
/// Successful `SYS_WRITE` calls.
pub static WRITE_CALLS: AtomicU64 = AtomicU64::new(0);
/// Bytes accepted from user buffers.
pub static BYTES_WRITTEN: AtomicU64 = AtomicU64::new(0);
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

fn write(frame: &mut InterruptFrame) {
    let fd = frame.rcx;
    let buffer = frame.rdx;
    let length = frame.r8;
    if fd != 1 || length == 0 || length > MAX_WRITE {
        kill(frame, "invalid write arguments");
        return;
    }
    let end = match buffer.checked_add(length) {
        Some(end) => end,
        None => {
            kill(frame, "write buffer overflows the address space");
            return;
        }
    };
    if buffer < 0x1000 || end > USER_LIMIT {
        kill(frame, "write buffer outside the user address space");
        return;
    }
    // Every page the buffer touches must be mapped user-accessible in the
    // live address space.
    let mut page = buffer & !0xfff;
    while page < end {
        match vm::translate_live(page) {
            Some((_, flags)) if flags & os_core::paging::USER != 0 => {}
            _ => {
                kill(frame, "write buffer is not mapped for user access");
                return;
            }
        }
        page += 4096;
    }

    // SAFETY: the span was validated above: in the user half, mapped, user
    // accessible, and bounded. The kernel reads it through the live CR3.
    let bytes = unsafe { core::slice::from_raw_parts(buffer as *const u8, length as usize) };
    serial::force_write_bytes(bytes);
    WRITE_CALLS.fetch_add(1, Ordering::SeqCst);
    BYTES_WRITTEN.fetch_add(length, Ordering::SeqCst);
    frame.rax = length;
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
