#!/usr/bin/env python3
"""Draws the placeholder app icons (a plain magnifier) with the standard library only.

Replace these with the real artwork when it exists, then run `npx tauri icon` on it.
Usage: python3 scripts/make-placeholder-icons.py  (writes to src-tauri/icon-sources/)
"""

import math
import os
import struct
import zlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "src-tauri", "icon-sources")


def png(path, size, pixel):
    rows = bytearray()
    for y in range(size):
        rows.append(0)
        for x in range(size):
            rows.extend(pixel(x + 0.5, y + 0.5))
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)
    data = b"\x89PNG\r\n\x1a\n"
    data += chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
    data += chunk(b"IDAT", zlib.compress(bytes(rows), 9))
    data += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(data)


def rounded_box(px, py, cx, cy, half, radius):
    qx = abs(px - cx) - half + radius
    qy = abs(py - cy) - half + radius
    outside = math.hypot(max(qx, 0), max(qy, 0))
    return outside + min(max(qx, qy), 0) - radius


def capsule(px, py, ax, ay, bx, by, r):
    pax, pay, bax, bay = px - ax, py - ay, bx - ax, by - ay
    h = max(0.0, min(1.0, (pax * bax + pay * bay) / (bax * bax + bay * bay)))
    return math.hypot(pax - bax * h, pay - bay * h) - r


def magnifier(px, py, s):
    """Signed distance to the glyph, in a 1024-unit square scaled by s."""
    x, y = px / s, py / s
    ring = abs(math.hypot(x - 440, y - 440) - 190) - 44
    handle = capsule(x, y, 600, 600, 790, 790, 56)
    return min(ring, handle) * s


def coverage(d):
    return max(0.0, min(1.0, 0.5 - d))


def app_icon(background):
    size = 1024
    def pixel(x, y):
        bg = coverage(rounded_box(x, y, 512, 512, 448, 200))
        glyph = coverage(magnifier(x, y, 1.0)) * bg
        r = round(background[0] * (1 - glyph) + 255 * glyph)
        g = round(background[1] * (1 - glyph) + 255 * glyph)
        b = round(background[2] * (1 - glyph) + 255 * glyph)
        return (r, g, b, round(255 * bg))
    return size, pixel


def tray_template():
    size = 44
    # The glyph spans about 206..846 units; fit it into the square with a 2 px margin.
    scale = (size - 4) / 640
    offset = 2 - 206 * scale
    def pixel(x, y):
        a = coverage(magnifier(x - offset, y - offset, scale))
        return (0, 0, 0, round(255 * a))
    return size, pixel


def main():
    os.makedirs(OUT, exist_ok=True)
    for name, colour in (("icon.png", (31, 41, 55)), ("icon-test.png", (180, 83, 9))):
        size, pixel = app_icon(colour)
        png(os.path.join(OUT, name), size, pixel)
    size, pixel = tray_template()
    os.makedirs(os.path.join(ROOT, "src-tauri", "icons"), exist_ok=True)
    png(os.path.join(ROOT, "src-tauri", "icons", "tray-template.png"), size, pixel)


if __name__ == "__main__":
    main()
