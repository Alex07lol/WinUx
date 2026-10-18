#!/usr/bin/env python3
"""Check a PROJECT_OS userland ELF64 artifact against the kernel loader contract.

Verifies: 64-bit little-endian ELF, EM_X86_64, ET_EXEC, valid header sizes,
PT_LOAD segments that are page-aligned, non-overlapping, inside the kernel's
user code window (0x400000..0x500000), without write+execute flags, with
filesz <= memsz, and an entry point inside an executable PT_LOAD.

Usage: check_user_elf.py <file>

Exit status: 0 on pass (prints a one-line summary), 1 on any violation.
"""

import struct
import sys
from pathlib import Path

EHDR_SIZE = 64
PHDR_SIZE = 56
PAGE_SIZE = 4096
CODE_BASE = 0x400000
CODE_END = 0x500000
PT_LOAD = 1
PF_X = 1
PF_W = 2
PF_R = 4


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

    if len(image) < EHDR_SIZE:
        fail("truncated: shorter than the ELF header")
    if image[0:4] != b"\x7fELF":
        fail("bad magic")
    if image[4] != 2:
        fail("not 64-bit")
    if image[5] != 1:
        fail("not little endian")
    if image[6] != 1:
        fail("bad ident version")

    (e_type, e_machine, e_version) = struct.unpack_from("<HHI", image, 16)
    (e_entry,) = struct.unpack_from("<Q", image, 24)
    (e_phoff,) = struct.unpack_from("<Q", image, 32)
    (e_ehsize, e_phentsize, e_phnum) = struct.unpack_from("<HHH", image, 52)

    if e_type != 2:
        fail(f"unsupported type {e_type}")
    if e_machine != 62:
        fail(f"wrong machine {e_machine}")
    if e_version != 1 or e_ehsize != EHDR_SIZE or e_phentsize != PHDR_SIZE:
        fail("bad header sizes")
    if e_phnum == 0:
        fail("no program headers")
    if e_phoff + e_phnum * PHDR_SIZE > len(image):
        fail("truncated: program header table outside the file")

    segments = []
    for index in range(e_phnum):
        base = e_phoff + index * PHDR_SIZE
        (p_type, p_flags) = struct.unpack_from("<II", image, base)
        (p_offset, p_vaddr, _p_paddr, p_filesz, p_memsz, p_align) = struct.unpack_from(
            "<QQQQQQ", image, base + 8
        )
        if p_type != PT_LOAD:
            continue
        if p_flags & ~7:
            fail(f"bad segment flags {p_flags:#x}")
        if p_flags & PF_W and p_flags & PF_X:
            fail("write+execute segment")
        if p_filesz > p_memsz:
            fail("segment size mismatch")
        if p_offset + p_filesz > len(image):
            fail("segment out of file")
        if p_align not in (0, PAGE_SIZE) or p_vaddr % PAGE_SIZE != 0:
            fail("bad segment alignment")
        start = p_vaddr
        end = p_vaddr + p_memsz
        if start < CODE_BASE or end > CODE_END:
            fail(f"segment {start:#x}..{end:#x} outside the user code window")
        segments.append((p_flags, start, end))

    if not segments:
        fail("no loadable segments")
    for i, (_, start_a, end_a) in enumerate(segments):
        for _, start_b, end_b in segments[i + 1 :]:
            if start_a < end_b and start_b < end_a:
                fail("overlapping segments")

    if not any(flags & PF_X and start <= e_entry < end for flags, start, end in segments):
        fail(f"entry {e_entry:#x} is not inside an executable segment")

    def describe(flags: int) -> str:
        text = "R" + ("W" if flags & PF_W else "-") + ("X" if flags & PF_X else "-")
        return text

    listing = ", ".join(
        f"{describe(flags)} {start:#x}-{end:#x}" for flags, start, end in segments
    )
    print(f"OK: {path.name}: entry {e_entry:#x}, {len(segments)} PT_LOAD ({listing})")


if __name__ == "__main__":
    main()
