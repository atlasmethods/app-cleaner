#!/usr/bin/env python3
"""Generate the ClearSweep source icon (teal disc with a white four-point sparkle).

Usage: python3 scripts/gen-icon.py out.png   then   pnpm tauri icon out.png
Pure stdlib (zlib PNG writer), 4x4 supersampling for smooth edges.
"""
import math
import struct
import sys
import zlib

SIZE = 1024
SS = 3
TEAL = (13, 148, 136)
WHITE = (255, 255, 255)


def in_star(px, py, cx, cy, radius):
    """Four-point star: |x|^0.6 + |y|^0.6 <= 1 gives pointed, concave arms."""
    dx, dy = abs(px - cx) / radius, abs(py - cy) / radius
    return dx ** 0.6 + dy ** 0.6 <= 1.0


def sample(x, y):
    c = SIZE / 2
    if math.hypot(x - c, y - c) > SIZE * 0.47:
        return None
    if in_star(x, y, c, c, SIZE * 0.34):
        return WHITE
    if in_star(x, y, SIZE * 0.72, SIZE * 0.28, SIZE * 0.09):
        return WHITE
    return TEAL


def main(out):
    rows = []
    for j in range(SIZE):
        row = bytearray([0])
        for i in range(SIZE):
            r = g = b = a = 0
            for sj in range(SS):
                for si in range(SS):
                    s = sample(i + (si + 0.5) / SS, j + (sj + 0.5) / SS)
                    if s is not None:
                        r += s[0]
                        g += s[1]
                        b += s[2]
                        a += 1
            n = SS * SS
            if a:
                row += bytes((r // a, g // a, b // a, 255 * a // n))
            else:
                row += bytes((0, 0, 0, 0))
        rows.append(bytes(row))
    raw = b"".join(rows)

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
    open(out, "wb").write(png)


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "icon-source.png")
