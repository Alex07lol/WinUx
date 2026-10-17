#!/usr/bin/env python3
"""Decode PROJECT_OS boot-dashboard text out of a framebuffer screenshot.

The dashboard draws with the project's own 5x7 bitmap font, so a screenshot can
be decoded back to text: match glyph bitmaps against the framebuffer pixels and
report every glyph with its pixel position. This is how the dashboard layout is
verified without eyeballing a VM window.
"""

import re
import sys
from pathlib import Path

from PIL import Image

FONT_RE = re.compile(r"\[((?:0b[01]{5},\s*){6}0b[01]{5})\]")


def load_font(path: Path):
    text = path.read_text(encoding="utf-8")
    body = text.split("pub const FONT", 1)[1]
    rows = FONT_RE.findall(body)
    table = []
    for entry in rows[:64]:
        bits = [int(value, 2) for value in re.findall(r"0b([01]{5})", entry)]
        table.append(bits)
    if len(table) != 64:
        raise SystemExit(f"expected 64 glyphs, parsed {len(table)}")
    return table


BACKGROUNDS = {(11, 15, 20), (17, 22, 29), (33, 38, 45), (48, 54, 61), (18, 38, 26), (42, 19, 21)}


def ink(image, x, y):
    pixel = image.getpixel((x, y))[:3]
    return pixel not in BACKGROUNDS and sum(pixel) > 90


def decode_band(image, table, x0, x1, y0, y1, scale):
    out = []
    x = x0
    while x + 5 * scale <= x1:
        best = None
        for index, bits in enumerate(table):
            score = 0
            for row in range(7):
                for column in range(5):
                    want = bool(bits[row] & (1 << (4 - column)))
                    for dy in range(scale):
                        for dx in range(scale):
                            if ink(image, x + column * scale + dx, y0 + row * scale + dy) == want:
                                score += 1
            total = 7 * 5 * scale * scale
            if best is None or score > best[1]:
                best = (index, score, total)
        if best is None or best[1] < best[2] * 0.8:
            break
        index = best[0]
        char = " " if index == 0 else chr(0x20 + index)
        out.append((char, x, y0, best[1] / best[2]))
        x += 6 * scale
    return out


def main():
    shot = Path(sys.argv[1] if len(sys.argv) > 1 else "artifacts/dashboard-after.png")
    font_path = Path("kernel/src/font.rs")
    table = load_font(font_path)
    image = Image.open(shot).convert("RGB")
    width, height = image.size

    ink_rows = [
        y
        for y in range(height)
        if any(ink(image, x, y) for x in range(0, width, 2))
    ]
    bands = []
    for y in ink_rows:
        if bands and y - bands[-1][1] <= 2:
            bands[-1][1] = y
        else:
            bands.append([y, y])

    print(f"{shot}: {width}x{height}, {len(bands)} text bands")
    for y0, y1 in bands:
        best = None
        for scale in (2, 1):
            for top in range(y0, max(y0, y1 - 7 * scale + 2)):
                chars = None
                for x in range(0, width):
                    if not any(ink(image, x, y) for y in range(y0, y1 + 1)):
                        continue
                    decoded = decode_band(image, table, x, width, top, y1, scale)
                    if decoded and decoded[0][3] > 0.85:
                        chars = decoded
                        break
                if not chars:
                    continue
                good = [entry for entry in chars if entry[3] > 0.9]
                score = (len(good), sum(entry[3] for entry in chars) / len(chars) - top * 1e-6)
                if best is None or score > best[0]:
                    best = (score, scale, top, chars)
        if best is None:
            print(f"y={y0:>4}-{y1:<4} :: <undecoded>")
            continue
        _, scale, top, chars = best
        text = "".join(char for char, *_ in chars)
        print(
            f"y={top:>4}-{top + 7 * scale - 1:<4} scale={scale} "
            f"x={chars[0][1]:>4} conf={sum(c[3] for c in chars) / len(chars):.2f} :: {text}"
        )


main()
