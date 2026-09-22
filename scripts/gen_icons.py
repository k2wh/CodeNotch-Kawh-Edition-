#!/usr/bin/env python3
"""Generate CodeNotch's app icons (PNG + ICO) with no third-party dependencies.

The mark is a dark rounded "notch" tile with a cyan usage ring and an amber
attention dot -- the same visual language the HUD itself uses.

Usage:  python3 scripts/gen_icons.py
Output: src-tauri/icons/*.png, src-tauri/icons/icon.ico
"""

import math
import os
import struct
import zlib

OUT_DIR = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src-tauri", "icons")

BG = (16, 19, 23)          # near-black tile, matches HUD surface
NOTCH = (6, 7, 9)          # the pill cut-out
RING = (34, 211, 238)      # cyan-400, the "generating" accent
RING_TRACK = (39, 45, 54)  # unfilled part of the ring
DOT = (251, 191, 36)       # amber-400, the "waiting for input" pulse

SS = 4  # supersampling factor for anti-aliasing


def rounded_rect(px, py, x0, y0, x1, y1, r):
    """True when point (px, py) is inside the rounded rectangle."""
    cx = min(max(px, x0 + r), x1 - r)
    cy = min(max(py, y0 + r), y1 - r)
    return (px - cx) ** 2 + (py - cy) ** 2 <= r * r


def blend(dst, src, a):
    return tuple(int(round(d + (s - d) * a)) for d, s in zip(dst, src))


def render(size):
    """Render the mark at `size` px using SSxSS supersampling. Returns RGBA bytes."""
    n = size * SS
    u = n / 128.0  # design grid is 128 units

    # Accumulate coverage per output pixel for each layer.
    layers = [
        ("bg", BG),
        ("track", RING_TRACK),
        ("ring", RING),
        ("notch", NOTCH),
        ("dot", DOT),
    ]
    cov = {name: [0.0] * (size * size) for name, _ in layers}

    cx, cy = n / 2.0, n / 2.0 + 6 * u
    r_outer = 40 * u
    r_inner = 30 * u
    # Ring sweep: 270 degrees of track, ~200 degrees filled, starting at top.
    start = -math.pi / 2

    for sy in range(n):
        py = sy + 0.5
        oy = sy // SS
        for sx in range(n):
            px = sx + 0.5
            idx = (oy * size) + (sx // SS)

            if rounded_rect(px, py, 6 * u, 6 * u, n - 6 * u, n - 6 * u, 26 * u):
                cov["bg"][idx] += 1.0

            d = math.hypot(px - cx, py - cy)
            if r_inner <= d <= r_outer:
                ang = (math.atan2(py - cy, px - cx) - start) % (2 * math.pi)
                if ang <= math.radians(270):
                    cov["track"][idx] += 1.0
                if ang <= math.radians(200):
                    cov["ring"][idx] += 1.0

            # The notch: a pill hanging from the top edge.
            if rounded_rect(px, py, 40 * u, 4 * u, n - 40 * u, 26 * u, 11 * u):
                cov["notch"][idx] += 1.0

            if math.hypot(px - (cx + 30 * u), py - (cy - 26 * u)) <= 7 * u:
                cov["dot"][idx] += 1.0

    total = float(SS * SS)
    px_data = [(0, 0, 0, 0.0)] * (size * size)
    for i in range(size * size):
        rgb, alpha = (0, 0, 0), 0.0
        for name, color in layers:
            a = cov[name][i] / total
            if a <= 0:
                continue
            rgb = blend(rgb, color, a if alpha == 0 else a)
            alpha = alpha + a * (1.0 - alpha)
        px_data[i] = (rgb[0], rgb[1], rgb[2], alpha)

    out = bytearray()
    for i, (r, g, b, a) in enumerate(px_data):
        out += bytes((r, g, b, int(round(max(0.0, min(1.0, a)) * 255))))
    return bytes(out)


def write_png(path, size, rgba):
    raw = bytearray()
    stride = size * 4
    for y in range(size):
        raw.append(0)  # filter type 0 (None)
        raw += rgba[y * stride:(y + 1) * stride]

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)
    return png


def write_ico(path, entries):
    """entries: list of (size, png_bytes). ICO may embed PNG payloads directly."""
    header = struct.pack("<HHH", 0, 1, len(entries))
    offset = 6 + 16 * len(entries)
    dir_bytes, blob = b"", b""
    for size, png in entries:
        dim = 0 if size >= 256 else size
        dir_bytes += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(png), offset)
        blob += png
        offset += len(png)
    with open(path, "wb") as f:
        f.write(header + dir_bytes + blob)


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    cache = {}

    def png_for(size):
        if size not in cache:
            cache[size] = write_png(os.path.join(OUT_DIR, "_tmp.png"), size, render(size))
        return cache[size]

    named = {
        32: ["32x32.png"],
        128: ["128x128.png"],
        256: ["128x128@2x.png", "Square150x150Logo.png"],
        512: ["icon.png"],
        44: ["Square44x44Logo.png"],
        71: ["Square71x71Logo.png"],
        89: ["Square89x89Logo.png"],
        107: ["Square107x107Logo.png"],
        142: ["Square142x142Logo.png"],
        284: ["Square284x284Logo.png"],
        310: ["Square310x310Logo.png"],
        30: ["StoreLogo.png"],
    }
    for size, names in sorted(named.items()):
        data = png_for(size)
        for name in names:
            with open(os.path.join(OUT_DIR, name), "wb") as f:
                f.write(data)
            print("wrote", name, size)

    write_ico(os.path.join(OUT_DIR, "icon.ico"),
              [(s, png_for(s)) for s in (16, 32, 48, 64, 128, 256)])
    print("wrote icon.ico")

    tmp = os.path.join(OUT_DIR, "_tmp.png")
    if os.path.exists(tmp):
        os.remove(tmp)


if __name__ == "__main__":
    main()
