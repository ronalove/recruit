# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (C) 2026 Ronan Lamour
"""Draws the styled screens `tests/mux_gallery.rs` writes (JSON, `TestTerm::styled`) as PNG files.

    python3 -I tests/tools/gallery.py <folder>

Every `<name>.json` in the folder gives a `<name>.png`: the cells as the terminal drew them, with their colors, bold,
dim, underline and reverse video, in Menlo (Arial Unicode for the signs it lacks). For a look at the chrome, not a
pixel-exact copy of any terminal.
"""
import json
import sys
from pathlib import Path

from fontTools.ttLib import TTCollection, TTFont
from PIL import Image, ImageDraw, ImageFont

FG = (0xC0, 0xC1, 0xC2)
BG = (0x1E, 0x1E, 0x1E)
ANSI = [
    (0x1E, 0x1E, 0x1E), (0xCD, 0x31, 0x31), (0x0D, 0xBC, 0x79), (0xE5, 0xE5, 0x10),
    (0x24, 0x72, 0xC8), (0xBC, 0x3F, 0xBC), (0x11, 0xA8, 0xCD), (0xE5, 0xE5, 0xE5),
    (0x66, 0x66, 0x66), (0xF1, 0x4C, 0x4C), (0x23, 0xD1, 0x8B), (0xF5, 0xF5, 0x43),
    (0x3B, 0x8E, 0xEA), (0xD6, 0x70, 0xD6), (0x29, 0xB8, 0xDB), (0xFF, 0xFF, 0xFF),
]
LIGHT_FG = (0x2B, 0x2E, 0x33)
LIGHT_BG = (0xFA, 0xFA, 0xF7)
LIGHT_ANSI = [
    (0x1E, 0x1E, 0x1E), (0xC4, 0x1E, 0x1E), (0x0A, 0x84, 0x4E), (0xA0, 0x7A, 0x00),
    (0x1D, 0x5F, 0xB8), (0xA0, 0x2E, 0xA0), (0x0B, 0x7F, 0x9B), (0x6B, 0x6B, 0x6B),
    (0x55, 0x55, 0x55), (0xD1, 0x2F, 0x2F), (0x13, 0x9C, 0x62), (0xB8, 0x8A, 0x00),
    (0x2E, 0x6F, 0xD0), (0xB5, 0x42, 0xB5), (0x14, 0x8F, 0xAE), (0x1E, 0x1E, 0x1E),
]
NAMES = ["Black", "Red", "Green", "Yellow", "Blue", "Magenta", "Cyan", "White"]
DARK_FG, DARK_BG, DARK_ANSI = FG, BG, ANSI
SIZE = 15
CELL = (9, 19)

MENLO = "/System/Library/Fonts/Menlo.ttc"
FALLBACKS = ["/System/Library/Fonts/Apple Symbols.ttf", "/System/Library/Fonts/Supplemental/Arial Unicode.ttf"]


def indexed(n):
    if n < 16:
        return ANSI[n]
    if n < 232:
        n -= 16
        level = [0, 95, 135, 175, 215, 255]
        return (level[n // 36], level[n // 6 % 6], level[n % 6])
    v = 8 + (n - 232) * 10
    return (v, v, v)


def color(text, default):
    kind, _, value = text.partition(":")
    if kind == "rgb":
        return tuple(int(v) for v in value.split(","))
    if kind == "idx":
        return indexed(int(value))
    name = value.removeprefix("Dim").removeprefix("Bright")
    if value in ("Foreground", "BrightForeground"):
        return FG
    if value in ("Background", "Cursor"):
        return BG
    if name in NAMES:
        i = NAMES.index(name)
        return ANSI[i + 8] if value.startswith("Bright") else ANSI[i]
    return default


def dim(rgb):
    return tuple(int(c * 0.6 + b * 0.4) for c, b in zip(rgb, BG))


class Fonts:
    """The fonts in the order they are tried for a character, each with the characters it has."""

    def __init__(self):
        def load(path, index, bold=False):
            cmap = set(TTCollection(path).fonts[index].getBestCmap()) if path.endswith(".ttc") else set(
                TTFont(path).getBestCmap()
            )
            return bold, cmap, ImageFont.truetype(path, SIZE, index=index) if path.endswith(".ttc") else ImageFont.truetype(path, SIZE)

        self.bold = [load(MENLO, 1, True)]
        self.regular = [load(MENLO, 0)] + [load(path, 0) for path in FALLBACKS]

    def pick(self, char, bold):
        chain = (self.bold if bold else []) + self.regular
        for _, cmap, font in chain:
            if ord(char[0]) in cmap:
                return font
        return self.regular[0][2]


def draw(path, fonts):
    global FG, BG, ANSI
    data = json.loads(path.read_text())
    if data.get("light"):
        FG, BG, ANSI = LIGHT_FG, LIGHT_BG, LIGHT_ANSI
    else:
        FG, BG, ANSI = DARK_FG, DARK_BG, DARK_ANSI
    rows = data["rows"]
    width, height = len(rows[0]) * CELL[0], len(rows) * CELL[1]
    image = Image.new("RGB", (width, height), BG)
    pen = ImageDraw.Draw(image)
    for y, row in enumerate(rows):
        for x, (text, fg, bg, flags) in enumerate(row):
            if "w" in flags:
                continue
            f, b = color(fg, FG), color(bg, BG)
            if "r" in flags:
                f, b = b, f
            if "d" in flags:
                f = dim(f)
            left, top = x * CELL[0], y * CELL[1]
            wide = 2 if x + 1 < len(row) and "w" in row[x + 1][3] else 1
            if b != BG:
                pen.rectangle([left, top, left + wide * CELL[0] - 1, top + CELL[1] - 1], fill=b)
            if text.strip():
                pen.text((left, top + 1), text[0], font=fonts.pick(text, "b" in flags), fill=f)
                if "u" in flags:
                    pen.line([left, top + CELL[1] - 2, left + wide * CELL[0] - 1, top + CELL[1] - 2], fill=f)
    image.save(path.with_suffix(".png"))


def main():
    folder = Path(sys.argv[1])
    fonts = Fonts()
    for path in sorted(folder.glob("*.json")):
        draw(path, fonts)
        print(path.with_suffix(".png"))


main()
