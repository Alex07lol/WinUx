#!/usr/bin/env python3
"""Validate a QEMU PPM screendump against the framebuffer console transcript.

Checks (docs/FRAMEBUFFER_CONSOLE.md §9):
  * the PPM dimensions equal the `framebuffer: WxH` serial line;
  * the grid rectangle derived from `fbcon: enabled CxR cells at scale S`
    contains at least MIN_FG_PIXELS pixels in the pinned foreground colour;
  * every pixel outside the grid rectangle is the pinned background colour.

Usage: check_screendump.py <screendump.ppm> <session-log>
"""

import re
import sys

BG = (0x0b, 0x0f, 0x14)
FG = (0xd0, 0xd7, 0xde)
MIN_FG_PIXELS = 500

FRAMEBUFFER = re.compile(rb"framebuffer: (\d+)x(\d+) pitch (\d+) bpp (\d+)")
ENABLED = re.compile(rb"fbcon: enabled (\d+)x(\d+) cells at scale (\d+)")


def read_ppm(path):
    with open(path, "rb") as handle:
        data = handle.read()
    if not data.startswith(b"P6"):
        raise ValueError("not a binary PPM (missing P6 magic)")
    tokens = []
    index = 2
    while len(tokens) < 3:
        while index < len(data) and data[index : index + 1].isspace():
            index += 1
        if index < len(data) and data[index : index + 1] == b"#":
            while index < len(data) and data[index : index + 1] != b"\n":
                index += 1
            continue
        start = index
        while index < len(data) and not data[index : index + 1].isspace():
            index += 1
        if start == index:
            raise ValueError("truncated PPM header")
        tokens.append(int(data[start:index]))
    index += 1
    width, height, maxval = tokens
    if maxval != 255:
        raise ValueError("unsupported PPM maxval %d" % maxval)
    pixels = data[index:]
    if len(pixels) < width * height * 3:
        raise ValueError("PPM pixel data is truncated")
    return width, height, pixels


def main():
    if len(sys.argv) != 3:
        sys.stderr.write("usage: check_screendump.py <screendump.ppm> <session-log>\n")
        return 2
    ppm_path, log_path = sys.argv[1:]

    with open(log_path, "rb") as handle:
        transcript = handle.read()
    framebuffer = FRAMEBUFFER.search(transcript)
    enabled = ENABLED.search(transcript)
    if framebuffer is None or enabled is None:
        sys.stderr.write(
            "FAIL: framebuffer/fbcon sizing lines missing from %s\n" % log_path
        )
        return 1
    fb_width, fb_height = int(framebuffer.group(1)), int(framebuffer.group(2))
    cols, rows, scale = (int(enabled.group(index)) for index in (1, 2, 3))

    try:
        width, height, pixels = read_ppm(ppm_path)
    except (OSError, ValueError) as error:
        sys.stderr.write("FAIL: cannot read screendump: %s\n" % error)
        return 1

    if (width, height) != (fb_width, fb_height):
        sys.stderr.write(
            "FAIL: screendump is %dx%d, serial framebuffer line says %dx%d\n"
            % (width, height, fb_width, fb_height)
        )
        return 1

    grid_width = cols * 6 * scale
    grid_height = rows * 8 * scale
    if grid_width > width or grid_height > height:
        sys.stderr.write(
            "FAIL: grid %dx%d does not fit the %dx%d screendump\n"
            % (grid_width, grid_height, width, height)
        )
        return 1

    bg_bytes = bytes(BG)
    fg_bytes = bytes(FG)
    row_stride = width * 3
    grid_stride = grid_width * 3
    border_width = (width - grid_width) * 3
    outside_non_bg = 0
    for y in range(grid_height):
        row = pixels[y * row_stride : (y + 1) * row_stride]
        if row[grid_stride:] != bg_bytes * (border_width // 3):
            outside_non_bg += 1
    bottom = pixels[grid_height * row_stride :]
    bg_row = bg_bytes * width
    for y in range(height - grid_height):
        if bottom[y * row_stride : (y + 1) * row_stride] != bg_row:
            outside_non_bg += 1
    if outside_non_bg:
        sys.stderr.write(
            "FAIL: %d scanline(s) outside the grid are not the background colour\n"
            % outside_non_bg
        )
        return 1

    fg_pixels = 0
    for y in range(grid_height):
        fg_pixels += pixels[y * row_stride : y * row_stride + grid_stride].count(fg_bytes)
    if fg_pixels < MIN_FG_PIXELS:
        sys.stderr.write(
            "FAIL: the grid contains %d foreground pixels, expected at least %d\n"
            % (fg_pixels, MIN_FG_PIXELS)
        )
        return 1

    sys.stdout.write(
        "screendump: %dx%d, grid %dx%d cells at scale %d (%dx%d px), %d foreground pixels\n"
        % (width, height, cols, rows, scale, grid_width, grid_height, fg_pixels)
    )
    sys.stdout.write("PASS: screendump matches the framebuffer console grid\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
