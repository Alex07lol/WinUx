/* x86-64 kernel entry.
 *
 * Limine enters here with long mode, paging and NX enabled, a bootloader-owned
 * stack of at least the requested size, and a pushed invalid return address.
 * The System V ABI requires rsp % 16 == 8 on function entry, so align the stack
 * to 16 bytes and let `call` push the return address that Rust expects.
 */
.section .text.entry, "ax"
.global _start
.type _start, @function
_start:
    and rsp, -16
    xor ebp, ebp
    cld
    call kmain

.hang:
    cli
    hlt
    jmp .hang
.size _start, .-_start

.section .note.GNU-stack, "", @progbits
