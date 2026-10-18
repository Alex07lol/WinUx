//! Interrupt descriptor table and exception entry stubs.
//!
//! All exceptions push a uniform frame: 15 general-purpose registers, the
//! vector, the error code, then the CPU frame (rip/cs/rflags for a same-ring
//! interrupt). Vector 8 (double fault) runs on IST1 so it can report even when
//! the normal kernel stack is unusable.

use core::arch::{asm, global_asm};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use super::{pic, pit};
use crate::kprintln;
use crate::serial;

#[repr(C)]
pub struct InterruptFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub vector: u64,
    pub error: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
}

global_asm!(
    r#"
.section .text
.p2align 4

.macro ISR_NOERR num
.global isr_\num
isr_\num:
    push 0
    push \num
    jmp isr_common
.endm

.macro ISR_ERR num
.global isr_\num
isr_\num:
    push \num
    jmp isr_common
.endm

ISR_NOERR 0
ISR_NOERR 1
ISR_NOERR 2
ISR_NOERR 3
ISR_NOERR 4
ISR_NOERR 5
ISR_NOERR 6
ISR_NOERR 7
ISR_ERR   8
ISR_NOERR 9
ISR_ERR   10
ISR_ERR   11
ISR_ERR   12
ISR_ERR   13
ISR_ERR   14
ISR_NOERR 15
ISR_NOERR 16
ISR_ERR   17
ISR_NOERR 18
ISR_NOERR 19
ISR_NOERR 20
ISR_ERR   21
ISR_NOERR 22
ISR_NOERR 23
ISR_NOERR 24
ISR_NOERR 25
ISR_NOERR 26
ISR_NOERR 27
ISR_NOERR 28
ISR_ERR   29
ISR_ERR   30
ISR_NOERR 31
ISR_NOERR 32
ISR_NOERR 36
ISR_NOERR 128

isr_common:
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15

    cld
    mov rdi, rsp
    mov rbp, rsp
    and rsp, -16
    call exception_dispatch
    mov rsp, rbp

    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax
    add rsp, 16
    iretq
"#
);

unsafe extern "C" {
    fn isr_0();
    fn isr_1();
    fn isr_2();
    fn isr_3();
    fn isr_4();
    fn isr_5();
    fn isr_6();
    fn isr_7();
    fn isr_8();
    fn isr_9();
    fn isr_10();
    fn isr_11();
    fn isr_12();
    fn isr_13();
    fn isr_14();
    fn isr_15();
    fn isr_16();
    fn isr_17();
    fn isr_18();
    fn isr_19();
    fn isr_20();
    fn isr_21();
    fn isr_22();
    fn isr_23();
    fn isr_24();
    fn isr_25();
    fn isr_26();
    fn isr_27();
    fn isr_28();
    fn isr_29();
    fn isr_30();
    fn isr_31();
    fn isr_32();
    fn isr_36();
    fn isr_128();
}

#[repr(C)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const fn empty() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            ist: 0,
            type_attr: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    fn for_handler(handler: u64, ist: u8, user_callable: bool) -> Self {
        Self {
            offset_low: handler as u16,
            selector: 0x08,
            ist,
            // present, DPL0 (or DPL3 for `int 0x80`), 64-bit interrupt gate
            type_attr: if user_callable { 0xee } else { 0x8e },
            offset_mid: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            reserved: 0,
        }
    }
}

#[repr(C, align(16))]
struct Idt([IdtEntry; 256]);

struct SyncCell<T>(UnsafeCell<T>);
// SAFETY: installed once on the boot CPU before interrupts are enabled.
unsafe impl<T> Sync for SyncCell<T> {}

static IDT: SyncCell<Idt> = SyncCell(UnsafeCell::new(Idt([IdtEntry::empty(); 256])));

/// Set by the breakpoint self-test handler; read by `kmain` after `int3`.
pub static BREAKPOINT_HANDLED: AtomicBool = AtomicBool::new(false);

#[repr(C, packed)]
struct IdtPointer {
    limit: u16,
    base: u64,
}

pub fn init() {
    let idt = unsafe { &mut *IDT.0.get() };
    let handlers: [unsafe extern "C" fn(); 33] = [
        isr_0, isr_1, isr_2, isr_3, isr_4, isr_5, isr_6, isr_7, isr_8, isr_9, isr_10, isr_11,
        isr_12, isr_13, isr_14, isr_15, isr_16, isr_17, isr_18, isr_19, isr_20, isr_21, isr_22,
        isr_23, isr_24, isr_25, isr_26, isr_27, isr_28, isr_29, isr_30, isr_31, isr_32,
    ];

    for (vector, handler) in handlers.iter().enumerate() {
        // Vector 8 uses IST1; all other gates use the current stack.
        let ist = if vector == 8 { 1 } else { 0 };
        idt.0[vector] = IdtEntry::for_handler(*handler as usize as u64, ist, false);
    }
    // COM1 receive: IRQ4 => vector 36.
    idt.0[36] = IdtEntry::for_handler(isr_36 as *const () as usize as u64, 0, false);
    // Syscall gate: user-callable interrupt gate (DPL3) at vector 0x80.
    idt.0[128] = IdtEntry::for_handler(isr_128 as *const () as usize as u64, 0, true);

    let pointer = IdtPointer {
        limit: (core::mem::size_of::<Idt>() - 1) as u16,
        base: idt.0.as_ptr() as u64,
    };
    // SAFETY: the IDT is static, 256 gates are initialised, and the gate
    // handlers are real functions emitted above.
    unsafe {
        asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
    }
}

fn exception_name(vector: u64) -> &'static str {
    match vector {
        0 => "divide-error",
        1 => "debug",
        2 => "nmi",
        3 => "breakpoint",
        4 => "overflow",
        5 => "bound-range",
        6 => "invalid-opcode",
        7 => "device-not-available",
        8 => "double-fault",
        10 => "invalid-tss",
        11 => "segment-not-present",
        12 => "stack-segment",
        13 => "general-protection",
        14 => "page-fault",
        16 => "x87-floating-point",
        17 => "alignment-check",
        18 => "machine-check",
        19 => "simd-floating-point",
        20 => "virtualization",
        21 => "control-protection",
        29 => "vmm-communication",
        30 => "security",
        32 => "timer-irq0",
        _ => "reserved",
    }
}

/// Called from `isr_common` with a pointer to the saved frame.
#[unsafe(no_mangle)]
pub extern "C" fn exception_dispatch(frame: *mut InterruptFrame) {
    // SAFETY: the stub supplies a pointer to the frame it just built on the
    // interrupted stack; it is valid for the duration of this call.
    let frame = unsafe { &mut *frame };

    match frame.vector {
        32 => {
            pit::handle_interrupt();
            pic::eoi_master();
            crate::sched::on_timer_tick();
            // Perform a pending reschedule here, at the tail of the
            // dispatcher: the interrupted thread's frame stays on its stack
            // and is resumed by the normal iretq path when it is picked again.
            crate::sched::preempt_if_needed();
        }
        3 => {
            // Deliberate self-test: proves descriptor loading, gate dispatch,
            // frame layout and iretq all work.
            BREAKPOINT_HANDLED.store(true, Ordering::SeqCst);
            kprintln!("selftest: int3 exception handled (rip={:#x})", frame.rip);
        }
        36 => {
            // COM1 receive (IRQ4).
            crate::console::on_rx_interrupt();
            pic::eoi_master();
        }
        128 => crate::syscall::dispatch(frame),
        vector => {
            // A fault taken from CPL3 is a user-process fault, not a kernel
            // bug: report it, kill the offending thread and keep the kernel
            // running. Kernel-mode faults stay fatal.
            if frame.cs & 3 == 3 {
                crate::syscall::user_fault(frame);
            }
            serial::force_print(format_args!(
                "\r\nPROJECT_OS_FATAL: exception {} ({}) error={:#x} rip={:#x} cs={:#x} rflags={:#x}\r\n",
                vector,
                exception_name(vector),
                frame.error,
                frame.rip,
                frame.cs,
                frame.rflags
            ));
            if vector == 14 {
                serial::force_print(format_args!(
                    "PROJECT_OS_FATAL: page fault address cr2={:#x}\r\n",
                    super::read_cr2()
                ));
            }
            crate::arch::halt_forever();
        }
    }
}
