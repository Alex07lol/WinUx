/* PROJECT_OS kernel image layout (x86-64, Limine protocol).
 *
 * The kernel is a fixed-address, non-relocatable ELF. Limine maps each PT_LOAD
 * segment with the permissions below:
 *   text   R-X  code
 *   rodata R--  constants, font, strings
 *   data   RW-  Limine requests, mutable state, BSS
 *
 * The Limine request structures must live in a writable segment because the
 * bootloader writes response pointers into them after scanning the image.
 */
OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

PHDRS
{
    text   PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data   PT_LOAD FLAGS(6);
}

SECTIONS
{
    . = 0xffffffff80000000;
    _kernel_start = .;

    .text : ALIGN(4096)
    {
        KEEP(*(.text.entry))
        *(.text .text.*)
    } :text

    . = ALIGN(4096);
    .rodata :
    {
        *(.rodata .rodata.*)
    } :rodata

    . = ALIGN(4096);
    .limine_requests_start : ALIGN(8)
    {
        KEEP(*(.limine_requests_start))
    } :data

    .limine_requests : ALIGN(8)
    {
        KEEP(*(.limine_requests))
    } :data

    .limine_requests_end : ALIGN(8)
    {
        KEEP(*(.limine_requests_end))
    } :data

    .data : ALIGN(4096)
    {
        *(.data .data.*)
    } :data

    .bss : ALIGN(4096)
    {
        *(.bss .bss.*)
        *(COMMON)
    } :data

    _kernel_end = .;

    /DISCARD/ :
    {
        *(.eh_frame)
        *(.eh_frame_hdr)
        *(.note .note.*)
        *(.comment)
    }
}
