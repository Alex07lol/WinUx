#!/usr/bin/env python3
"""Check a PROJECT_OS userland PE32+ artifact against the kernel loader contract.

Verifies: MS-DOS stub with "PE\\0\\0" at e_lfanew, COFF header for AMD64
(0x8664) marked EXECUTABLE and not a DLL, PE32+ optional header (0x20B) with
CUI subsystem, valid alignments and SizeOfImage, sections whose page-rounded
ranges live inside the kernel's user code window (0x400000..0x500000) without
write+execute flags, an entry point inside an executable section, no
import/delay-import/TLS/CLR directories, and (because the fixture links at
0x140000000) a base-relocation directory whose DIR64 targets each sit inside a
writable section and do not cross a page boundary.

Usage: check_user_pe.py <file>

Exit status: 0 on pass (prints a one-line summary), 1 on any violation.
"""

import struct
import sys
from pathlib import Path

PAGE_SIZE = 4096
CODE_BASE = 0x400000
CODE_END = 0x500000

COFF_HEADER_SIZE = 20
SECTION_HEADER_SIZE = 40
OPTIONAL_MIN_SIZE = 112

IMAGE_FILE_RELOCS_STRIPPED = 0x0001
IMAGE_FILE_EXECUTABLE_IMAGE = 0x0002
IMAGE_FILE_DLL = 0x2000
IMAGE_FILE_MACHINE_AMD64 = 0x8664
IMAGE_SUBSYSTEM_WINDOWS_CUI = 3

IMAGE_SCN_MEM_EXECUTE = 0x20000000
IMAGE_SCN_MEM_WRITE = 0x80000000

DIR_IMPORT = 1
DIR_BASERELOC = 5
DIR_TLS = 9
DIR_DELAY_IMPORT = 13
DIR_CLR = 14

REL_BASED_ABSOLUTE = 0
REL_BASED_DIR64 = 10


def fail(message: str) -> None:
    print(f"FAIL: {message}", file=sys.stderr)
    raise SystemExit(1)


def main() -> None:
    if len(sys.argv) != 2:
        print(f"usage: {Path(sys.argv[0]).name} <file>", file=sys.stderr)
        raise SystemExit(2)
    path = Path(sys.argv[1])
    if not path.is_file():
        fail(f"file not found: {path}")
    image = path.read_bytes()

    if len(image) < 0x40:
        fail("truncated: shorter than the DOS header")
    if image[0:2] != b"MZ":
        fail("bad dos magic")

    (e_lfanew,) = struct.unpack_from("<I", image, 0x3C)
    if e_lfanew + 4 > len(image) or image[e_lfanew : e_lfanew + 4] != b"PE\x00\x00":
        fail("bad signature")
    coff = e_lfanew + 4
    if coff + COFF_HEADER_SIZE > len(image):
        fail("truncated: COFF header outside the file")

    (machine, num_sections) = struct.unpack_from("<HH", image, coff)
    (size_optional,) = struct.unpack_from("<H", image, coff + 16)
    (characteristics,) = struct.unpack_from("<H", image, coff + 18)
    if machine != IMAGE_FILE_MACHINE_AMD64:
        fail(f"wrong machine {machine:#x}")
    if not characteristics & IMAGE_FILE_EXECUTABLE_IMAGE:
        fail("not an executable image")
    if characteristics & IMAGE_FILE_DLL:
        fail("is a dll")

    optional = coff + COFF_HEADER_SIZE
    if optional + size_optional > len(image) or size_optional < OPTIONAL_MIN_SIZE:
        fail("bad optional header")
    (magic,) = struct.unpack_from("<H", image, optional)
    if magic != 0x20B:
        fail(f"not PE32+ (magic {magic:#x})")
    (entry_rva,) = struct.unpack_from("<I", image, optional + 16)
    (image_base,) = struct.unpack_from("<Q", image, optional + 24)
    (section_align, file_align) = struct.unpack_from("<II", image, optional + 32)
    (size_of_image,) = struct.unpack_from("<I", image, optional + 56)
    (subsystem,) = struct.unpack_from("<H", image, optional + 68)
    (num_dirs,) = struct.unpack_from("<I", image, optional + 108)
    if num_dirs < DIR_BASERELOC + 1 or OPTIONAL_MIN_SIZE + num_dirs * 8 > size_optional:
        fail("bad data directories")
    if file_align < 512 or file_align > 65536 or file_align & (file_align - 1):
        fail(f"bad file alignment {file_align:#x}")
    if section_align & (section_align - 1) or section_align < file_align:
        fail(f"bad section alignment {section_align:#x}")
    if size_of_image == 0 or size_of_image % section_align:
        fail(f"bad size of image {size_of_image:#x}")
    if subsystem != IMAGE_SUBSYSTEM_WINDOWS_CUI:
        fail(f"unsupported subsystem {subsystem}")
    if num_sections == 0:
        fail("no sections")

    def directory(index: int) -> tuple:
        if index >= num_dirs:
            return (0, 0)
        base = optional + OPTIONAL_MIN_SIZE + index * 8
        return struct.unpack_from("<II", image, base)

    for index, name in (
        (DIR_IMPORT, "import"),
        (DIR_DELAY_IMPORT, "delay-import"),
        (DIR_TLS, "tls"),
        (DIR_CLR, "clr"),
    ):
        if directory(index) != (0, 0):
            fail(f"{name} directory is not empty")

    table = optional + size_optional
    if table + num_sections * SECTION_HEADER_SIZE > len(image):
        fail("truncated: section table outside the file")

    sections = []
    for index in range(num_sections):
        base = table + index * SECTION_HEADER_SIZE
        name = image[base : base + 8].rstrip(b"\x00").decode("ascii", "replace")
        (vsize, va, raw_size, raw_offset) = struct.unpack_from("<IIII", image, base + 8)
        (chars,) = struct.unpack_from("<I", image, base + 36)
        if va % section_align:
            fail(f"section {name}: misaligned virtual address")
        memory_size = max(vsize, raw_size)
        if memory_size == 0:
            fail(f"section {name}: zero memory size")
        if raw_size:
            if raw_offset % file_align or raw_size % file_align:
                fail(f"section {name}: misaligned raw range")
            if raw_offset + raw_size > len(image):
                fail(f"section {name}: raw data outside the file")
        elif raw_offset:
            fail(f"section {name}: raw offset without raw size")
        if va + memory_size > size_of_image:
            fail(f"section {name}: memory range outside SizeOfImage")
        if sections:
            previous = sections[-1]
            previous_end = previous["va"] + (
                previous["memory_size"] + section_align - 1
            ) // section_align * section_align
            if va < previous_end:
                fail(f"section {name}: overlapping or out of order")
        if chars & IMAGE_SCN_MEM_EXECUTE and chars & IMAGE_SCN_MEM_WRITE:
            fail(f"section {name}: write+execute")
        start = CODE_BASE + va
        end = CODE_BASE + va + (memory_size + PAGE_SIZE - 1) // PAGE_SIZE * PAGE_SIZE
        if start < CODE_BASE or end > CODE_END:
            fail(f"section {name}: {start:#x}..{end:#x} outside the user code window")
        sections.append(
            {
                "name": name,
                "va": va,
                "memory_size": memory_size,
                "chars": chars,
                "raw_offset": raw_offset,
                "raw_size": raw_size,
            }
        )

    if not any(
        section["chars"] & IMAGE_SCN_MEM_EXECUTE
        and section["va"] <= entry_rva < section["va"] + section["memory_size"]
        for section in sections
    ):
        fail(f"entry {entry_rva:#x} is not inside an executable section")

    if characteristics & IMAGE_FILE_RELOCS_STRIPPED:
        fail("relocations stripped")

    def section_for_rva(rva: int) -> dict | None:
        for section in sections:
            if section["va"] <= rva < section["va"] + max(section["raw_size"], section["memory_size"]):
                return section
        return None

    (reloc_rva, reloc_size) = directory(DIR_BASERELOC)
    relocations = []
    if reloc_rva and reloc_size:
        anchor = section_for_rva(reloc_rva)
        if anchor is None or not anchor["raw_size"]:
            fail("relocation directory outside section raw data")
        offset = anchor["raw_offset"] + (reloc_rva - anchor["va"])
        if reloc_rva - anchor["va"] + reloc_size > anchor["raw_size"]:
            fail("relocation directory outside section raw data")
        end = offset + reloc_size
        while offset < end:
            if offset + 8 > end:
                fail("bad relocation block header")
            (page_rva, block_size) = struct.unpack_from("<II", image, offset)
            if block_size < 8 or block_size % 2 or offset + block_size > end:
                fail("bad relocation block size")
            for entry_index in range((block_size - 8) // 2):
                (entry,) = struct.unpack_from("<H", image, offset + 8 + entry_index * 2)
                kind, offset_in_page = entry >> 12, entry & 0x0FFF
                if kind == REL_BASED_ABSOLUTE:
                    continue
                if kind != REL_BASED_DIR64:
                    fail(f"unsupported relocation type {kind}")
                target = page_rva + offset_in_page
                target_section = section_for_rva(target)
                if target_section is None or not target_section["chars"] & IMAGE_SCN_MEM_WRITE:
                    fail(f"relocation target {target:#x} not in a writable section")
                if target % PAGE_SIZE + 8 > PAGE_SIZE:
                    fail(f"relocation target {target:#x} crosses a page boundary")
                relocations.append(target)
            offset += block_size

    print(
        f"OK: {path.name}: entry {entry_rva:#x}, image base {image_base:#x}, "
        f"{len(sections)} sections ({', '.join(s['name'] for s in sections)}), "
        f"{len(relocations)} relocations"
    )


if __name__ == "__main__":
    main()
