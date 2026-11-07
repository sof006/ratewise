#!/usr/bin/env python3
"""Draws the Ratewise icon as SVG and as a 256 px PNG from the same shapes.

A rounded blue square with level bars. Standard library only:
    python3 packaging/make-icon.py packaging/ratewise.svg packaging/ratewise.png
"""

import struct
import sys
import zlib

SIZE = 256
RADIUS = 56
TOP = (0x2B, 0x7B, 0xE0)  # gradient, top
BOTTOM = (0x0B, 0x4F, 0xA8)  # gradient, bottom
BAR = (0xFF, 0xFF, 0xFF)
BAR_W = 20
GAP = 10
# Bar heights: a sound wave rising and settling.
HEIGHTS = [52, 96, 148, 120, 168, 104, 64]


def bars():
    total = len(HEIGHTS) * BAR_W + (len(HEIGHTS) - 1) * GAP
    x0 = (SIZE - total) / 2
    for i, h in enumerate(HEIGHTS):
        x = x0 + i * (BAR_W + GAP)
        y = (SIZE - h) / 2
        yield x, y, BAR_W, h


def svg():
    hexc = lambda c: "#%02x%02x%02x" % c
    rects = "\n".join(
        f'  <rect x="{x:g}" y="{y:g}" width="{w}" height="{h}" rx="{w / 2:g}" fill="{hexc(BAR)}"/>'
        for x, y, w, h in bars()
    )
    return f"""<svg xmlns="http://www.w3.org/2000/svg" width="{SIZE}" height="{SIZE}" viewBox="0 0 {SIZE} {SIZE}">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="{hexc(TOP)}"/>
      <stop offset="1" stop-color="{hexc(BOTTOM)}"/>
    </linearGradient>
  </defs>
  <rect width="{SIZE}" height="{SIZE}" rx="{RADIUS}" fill="url(#bg)"/>
{rects}
</svg>
"""


def in_round_rect(px, py, x, y, w, h, r):
    if px < x or px >= x + w or py < y or py >= y + h:
        return False
    cx = min(max(px, x + r), x + w - r)
    cy = min(max(py, y + r), y + h - r)
    return (px - cx) ** 2 + (py - cy) ** 2 <= r * r


def png():
    ss = 4  # samples per axis
    shapes = list(bars())
    rows = []
    for py in range(SIZE):
        t = py / (SIZE - 1)
        bg = tuple(round(a + (b - a) * t) for a, b in zip(TOP, BOTTOM))
        row = bytearray([0])  # filter: none
        for px in range(SIZE):
            cover = bar = 0
            for sy in range(ss):
                for sx in range(ss):
                    fx = px + (sx + 0.5) / ss
                    fy = py + (sy + 0.5) / ss
                    if in_round_rect(fx, fy, 0, 0, SIZE, SIZE, RADIUS):
                        cover += 1
                        if any(in_round_rect(fx, fy, x, y, w, h, w / 2) for x, y, w, h in shapes):
                            bar += 1
            n = ss * ss
            if cover == 0:
                row += bytes(4)
                continue
            k = bar / cover
            rgb = [round(b * (1 - k) + f * k) for b, f in zip(bg, BAR)]
            row += bytes(rgb + [round(255 * cover / n)])
        rows.append(bytes(row))

    def chunk(kind, data):
        c = struct.pack(">I", len(data)) + kind + data
        return c + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(b"".join(rows), 9))
        + chunk(b"IEND", b"")
    )


if __name__ == "__main__":
    svg_path, png_path = sys.argv[1], sys.argv[2]
    with open(svg_path, "w") as f:
        f.write(svg())
    with open(png_path, "wb") as f:
        f.write(png())
