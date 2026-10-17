//! Kernel thread context switch.
//!
//! A cooperative/preemptive context switch only needs the callee-saved
//! registers: every other register is already spilled by the caller (Rust's
//! ABI) or lives in the interrupt frame when the switch happens from an ISR.
//! The switch saves rbp/rbx/r12..r15 on the current stack, stores the stack
//! pointer through `old_rsp`, loads `new_rsp`, restores the six registers and
//! returns into whatever the new stack says.
//!
//! A freshly spawned thread has no saved context yet, so its stack is
//! pre-fabricated by the scheduler to look like a switch-in from
//! [`project_os_context_switch`] whose return address is the trampoline.
//!
//! Interrupt state is not touched here: callers hold interrupts disabled
//! across the switch (see `crate::sched`).

use core::arch::global_asm;

global_asm!(
    r#"
.section .text
.p2align 4
.global project_os_context_switch
project_os_context_switch:
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    mov [rdi], rsp
    mov rsp, rsi
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    ret
"#
);

unsafe extern "C" {
    /// Save the current context's stack pointer through `old_rsp` and resume
    /// the context whose stack pointer is `new_rsp`.
    ///
    /// # Safety
    ///
    /// * `old_rsp` must point to writable storage for one `u64`.
    /// * `new_rsp` must point to a stack prepared by this module's layout
    ///   (saved registers + return address), or to a context previously saved
    ///   by this function.
    /// * The switch must not make a live stack unreachable (each context's
    ///   stack stays owned by its thread).
    pub fn project_os_context_switch(old_rsp: *mut u64, new_rsp: u64);
}
