#!/usr/bin/env python3
"""Verify the PROJECT_OS hybrid ISO without third-party dependencies.

Checks the ISO9660 primary volume descriptor, the El Torito boot record and the
boot catalog, including the validation-entry checksum and the presence of both
a BIOS (platform 0x00) and a UEFI (platform 0xEF) no-emulation entry.
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path

SECTOR = 2048


def fail(message: str) -> None:
    print(f"FAIL: {message}", file=sys.stderr)
    raise SystemExit(1)


def read_sector(image: bytes, lba: int) -> bytes:
    start = lba * SECTOR
    end = start + SECTOR
    if end > len(image):
        fail(f"sector {lba} is outside the image")
    return image[start:end]


def parse_catalog(catalog: bytes) -> list[tuple[int, int, int, int]]:
    """Return (platform, media, sectors, load_lba) for every catalog entry."""
    if len(catalog) < 64:
        fail("boot catalog is truncated")
    validation = catalog[:32]
    if validation[0] != 0x01:
        fail(f"unexpected validation header id {validation[0]:#x}")
    checksum = sum(struct.unpack_from("<16H", validation)) & 0xFFFF
    if checksum != 0:
        fail(f"validation entry checksum is invalid ({checksum:#x})")

    entries: list[tuple[int, int, int, int]] = []
    default = catalog[32:64]
    if default[0] not in (0x00, 0x88):
        fail(f"unexpected default entry id {default[0]:#x}")
    entries.append(
        (
            validation[1],
            default[1],
            struct.unpack_from("<H", default, 6)[0],
            struct.unpack_from("<I", default, 8)[0],
        )
    )

    offset = 64
    more_sections = True
    while more_sections:
        if offset + 32 > len(catalog):
            fail("boot catalog ended before the final section header")
        header = catalog[offset : offset + 32]
        if header[0] not in (0x90, 0x91):
            fail(f"unexpected section header id {header[0]:#x} at offset {offset}")
        platform = header[1]
        count = struct.unpack_from("<H", header, 2)[0]
        # Per the El Torito specification (and edk2's ElTorito.h), 0x90 means
        # more section headers follow and 0x91 is the final section header.
        more_sections = header[0] == 0x90
        offset += 32
        for _ in range(count):
            if offset + 32 > len(catalog):
                fail("boot catalog section is truncated")
            entry = catalog[offset : offset + 32]
            if entry[0] not in (0x00, 0x88):
                fail(f"unexpected section entry id {entry[0]:#x}")
            entries.append(
                (
                    platform,
                    entry[1],
                    struct.unpack_from("<H", entry, 6)[0],
                    struct.unpack_from("<I", entry, 8)[0],
                )
            )
            offset += 32
    return entries


def main() -> None:
    if len(sys.argv) != 2:
        print(f"usage: {Path(sys.argv[0]).name} <iso>", file=sys.stderr)
        raise SystemExit(2)
    path = Path(sys.argv[1])
    if not path.is_file():
        fail(f"ISO not found: {path}")

    image = path.read_bytes()
    pvd = read_sector(image, 16)
    if pvd[0] != 0x01 or pvd[1:6] != b"CD001":
        fail("primary volume descriptor not found at sector 16")

    boot_record = read_sector(image, 17)
    if boot_record[0] != 0x00 or boot_record[7:30] != b"EL TORITO SPECIFICATION":
        fail("El Torito boot record not found at sector 17")
    catalog_lba = struct.unpack_from("<I", boot_record, 0x47)[0]
    catalog = read_sector(image, catalog_lba)

    entries = parse_catalog(catalog)
    for platform, media, sectors, lba in entries:
        print(
            f"  catalog entry: platform=0x{platform:02x} media={media} "
            f"sectors={sectors} load_lba={lba}"
        )

    platforms = {platform for platform, _, _, _ in entries}
    if 0x00 not in platforms:
        fail("no BIOS (platform 0x00) boot entry")
    if 0xEF not in platforms:
        fail("no UEFI (platform 0xEF) boot entry")
    if any(media != 0 for _, media, _, _ in entries):
        fail("expected no-emulation boot entries")
    print(f"OK: {path} contains BIOS and UEFI El Torito no-emulation entries")


if __name__ == "__main__":
    main()
