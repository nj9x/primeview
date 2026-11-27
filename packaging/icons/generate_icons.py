#!/usr/bin/env python3
"""Generate the PrimeView app icon set using only the Python 3 standard library.

The mark is three rising candlesticks on a deep-blue rounded square. Every
size is rendered directly from signed distance fields (not downscaled), so
small sizes stay crisp; at 32 px and below the wicks are dropped and the
bodies widened so the mark still reads.

Outputs (next to this script):
  icon-1024.png, icon-512.png, icon-256.png, icon-128.png, icon-64.png, icon-32.png
                     full-bleed variant: Windows/Linux and the window icon
  icon.ico           multi-size Windows icon (16..256)
  icon.icns          macOS icon (built with `iconutil` when available)
  macos/icon-1024.png  macOS variant with the Big Sur grid padding and shadow

Usage: python3 packaging/icons/generate_icons.py
"""

import math
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))


# --------------------------------------------------------------------------
# PNG / ICO writers
# --------------------------------------------------------------------------

def png_bytes(width, height, rgba):
    """rgba: bytearray of width*height*4 (straight alpha)."""
    raw = bytearray()
    stride = width * 4
    for y in range(height):
        raw.append(0)  # filter: none
        raw += rgba[y * stride:(y + 1) * stride]

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b""))


def ico_dib(size, rgba):
    """Classic 32-bit BGRA DIB entry (bottom-up) with a 1-bit AND mask."""
    header = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    pixels = bytearray()
    for y in range(size - 1, -1, -1):
        row = rgba[y * size * 4:(y + 1) * size * 4]
        for x in range(size):
            r, g, b, a = row[x * 4:x * 4 + 4]
            pixels += bytes((b, g, r, a))
    mask_stride = ((size + 31) // 32) * 4
    mask = bytearray()
    for y in range(size - 1, -1, -1):
        bits = bytearray(mask_stride)
        for x in range(size):
            if rgba[(y * size + x) * 4 + 3] == 0:
                bits[x // 8] |= 0x80 >> (x % 8)
        mask += bits
    return header + bytes(pixels) + bytes(mask)


def write_ico(path, images):
    """images: list of (size, rgba). 256 is stored as PNG, smaller as DIB
    (widest compatibility: NSIS, WiX, rc.exe, Explorer)."""
    entries = []
    for size, rgba in images:
        data = png_bytes(size, size, rgba) if size >= 256 else ico_dib(size, rgba)
        entries.append((size, data))
    out = bytearray(struct.pack("<HHH", 0, 1, len(entries)))
    offset = 6 + 16 * len(entries)
    for size, data in entries:
        dim = 0 if size >= 256 else size
        out += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    for _, data in entries:
        out += data
    with open(path, "wb") as f:
        f.write(out)


# --------------------------------------------------------------------------
# Rendering
# --------------------------------------------------------------------------

def hexrgb(h):
    h = h.lstrip("#")
    return tuple(int(h[i:i + 2], 16) / 255.0 for i in (0, 2, 4))


BG_TOP = hexrgb("#2F6BFF")      # bright royal blue (top-left)
BG_BOTTOM = hexrgb("#0A1B4D")   # deep navy (bottom-right)
CANDLE_COLORS = [hexrgb("#7FA6FF"), hexrgb("#C3D6FF"), hexrgb("#3DF2B0")]


def sd_round_box(px, py, cx, cy, hw, hh, r):
    qx = abs(px - cx) - (hw - r)
    qy = abs(py - cy) - (hh - r)
    ox = qx if qx > 0 else 0.0
    oy = qy if qy > 0 else 0.0
    inside = qx if qx > qy else qy
    if inside > 0:
        inside = 0.0
    return math.sqrt(ox * ox + oy * oy) + inside - r


def candles(small):
    """Candles in body units (0..1, y down): list of shapes per candle.
    Each shape is (cx, cy, hw, hh, r)."""
    xs = [0.275, 0.50, 0.725]
    # (wick_top, body_top, body_bottom, wick_bottom)
    spans = [(0.49, 0.565, 0.765, 0.835),
             (0.32, 0.395, 0.655, 0.725),
             (0.14, 0.215, 0.535, 0.605)]
    out = []
    for i, x in enumerate(xs):
        wt, bt, bb, wb = spans[i]
        shapes = []
        if small:
            hw = 0.085
            shapes.append((x, (wt + wb) / 2, hw, (wb - wt) / 2, 0.03))
        else:
            hw = 0.074
            shapes.append((x, (bt + bb) / 2, hw, (bb - bt) / 2, 0.022))
            ww = 0.0165
            shapes.append((x, (wt + wb) / 2, ww, (wb - wt) / 2, ww))
        out.append(shapes)
    return out


def render(size, variant):
    """variant: 'full' (Windows/Linux, near full-bleed) or 'macos' (Big Sur grid)."""
    small = size <= 32
    if variant == "macos":
        body = size * 824.0 / 1024.0
        off = (size - body) / 2.0
        off_y = off - size * 0.006  # leave a little room for the shadow below
        radius = body * 0.2237
    else:
        pad = 0.0 if small else size * 0.03
        body = size - 2 * pad
        off = off_y = pad
        radius = body * 0.2

    shapes = candles(small)
    px_per_unit = body
    aa = 0.5  # half-pixel coverage ramp

    # Precompute candle shapes in pixel space with bounding boxes.
    cshapes = []
    for i, group in enumerate(shapes):
        g = []
        for (cx, cy, hw, hh, r) in group:
            phw = hw * body
            if len(g) == 1:  # wick: keep at least ~1.3 px wide
                phw = max(phw, 0.65)
            g.append((off + cx * body, off_y + cy * body, phw, hh * body, min(r * body, phw)))
        x0 = min(s[0] - s[2] for s in g) - 2
        x1 = max(s[0] + s[2] for s in g) + 2
        y0 = min(s[1] - s[3] for s in g) - 2
        y1 = max(s[1] + s[3] for s in g) + 2
        cshapes.append((g, (x0, y0, x1, y1), CANDLE_COLORS[i]))

    shadow_dy = body * 0.018
    shadow_blur = max(1.0, body * 0.025)
    bcx = off + body / 2
    bcy = off_y + body / 2
    hb = body / 2

    buf = bytearray(size * size * 4)
    for y in range(size):
        py = y + 0.5
        for x in range(size):
            px = x + 0.5
            d = sd_round_box(px, py, bcx, bcy, hb, hb, radius)
            body_a = min(1.0, max(0.0, aa - d))

            # macOS drop shadow under the tile.
            shadow_a = 0.0
            if variant == "macos" and d > -2:
                ds = sd_round_box(px, py - size * 0.012, bcx, bcy, hb, hb, radius)
                t = min(1.0, max(0.0, 1.0 - (ds + size * 0.004) / (size * 0.022)))
                shadow_a = 0.28 * t * t

            if body_a <= 0.0:
                a = shadow_a
                i4 = (y * size + x) * 4
                buf[i4:i4 + 4] = bytes((0, 0, 0, int(a * 255 + 0.5)))
                continue

            # Diagonal gradient with a soft top highlight.
            u = ((px - off) + (py - off_y)) / (2 * body)
            u = min(1.0, max(0.0, u))
            t = u * u * (3 - 2 * u)
            r = BG_TOP[0] + (BG_BOTTOM[0] - BG_TOP[0]) * t
            g = BG_TOP[1] + (BG_BOTTOM[1] - BG_TOP[1]) * t
            b = BG_TOP[2] + (BG_BOTTOM[2] - BG_TOP[2]) * t
            # Radial glow behind the tallest candle.
            gx = (px - (off + 0.66 * body)) / body
            gy = (py - (off_y + 0.30 * body)) / body
            glow = math.exp(-(gx * gx + gy * gy) / 0.06) * 0.22
            r += (0.24 - r) * glow
            g += (0.95 - g) * glow * 0.6
            b += (0.80 - b) * glow * 0.5

            for group, (x0, y0, x1, y1), col in cshapes:
                # soft shadow (large sizes only)
                if size >= 128 and x0 - shadow_blur <= px <= x1 + shadow_blur and y0 <= py - shadow_dy <= y1 + shadow_blur:
                    ds = min(sd_round_box(px, py - shadow_dy, *s) for s in group)
                    sa = min(1.0, max(0.0, 1.0 - (ds + 0.5) / shadow_blur)) * 0.35
                    r *= 1 - sa
                    g *= 1 - sa
                    b *= 1 - sa
                if x0 <= px <= x1 and y0 <= py <= y1:
                    dc = min(sd_round_box(px, py, *s) for s in group)
                    ca = min(1.0, max(0.0, aa - dc))
                    if ca > 0:
                        # subtle vertical shading on each candle
                        top = group[0][1] - group[0][3] if len(group) == 1 else group[1][1] - group[1][3]
                        h = (group[0][3] if len(group) == 1 else group[1][3]) * 2
                        k = min(1.0, max(0.0, (py - top) / h)) if h > 0 else 0
                        shade = 1.0 - 0.10 * k
                        r = r + (col[0] * shade - r) * ca
                        g = g + (col[1] * shade - g) * ca
                        b = b + (col[2] * shade - b) * ca

            # Composite tile over shadow (straight alpha).
            a = body_a + shadow_a * (1 - body_a)
            if a > 0:
                r = r * body_a / a
                g = g * body_a / a
                b = b * body_a / a
            i4 = (y * size + x) * 4
            buf[i4] = int(min(1.0, r) * 255 + 0.5)
            buf[i4 + 1] = int(min(1.0, g) * 255 + 0.5)
            buf[i4 + 2] = int(min(1.0, b) * 255 + 0.5)
            buf[i4 + 3] = int(a * 255 + 0.5)
    return buf


def write_png(path, size, rgba):
    with open(path, "wb") as f:
        f.write(png_bytes(size, size, rgba))


def main():
    cache = {}

    def get(size, variant):
        key = (size, variant)
        if key not in cache:
            print("rendering %s %dpx" % (variant, size), file=sys.stderr)
            cache[key] = render(size, variant)
        return cache[key]

    # Full-bleed PNGs (Linux packages, window icon, docs).
    for s in (1024, 512, 256, 128, 64, 32):
        write_png(os.path.join(HERE, "icon-%d.png" % s), s, get(s, "full"))

    # Windows .ico
    write_ico(os.path.join(HERE, "icon.ico"), [(s, get(s, "full")) for s in (16, 24, 32, 48, 64, 128, 256)])

    # macOS .iconset -> .icns
    os.makedirs(os.path.join(HERE, "macos"), exist_ok=True)
    write_png(os.path.join(HERE, "macos", "icon-1024.png"), 1024, get(1024, "macos"))
    tmp = tempfile.mkdtemp()
    iconset = os.path.join(tmp, "PrimeView.iconset")
    os.makedirs(iconset)
    for base in (16, 32, 128, 256, 512):
        write_png(os.path.join(iconset, "icon_%dx%d.png" % (base, base)), base, get(base, "macos"))
        write_png(os.path.join(iconset, "icon_%dx%d@2x.png" % (base, base)), base * 2, get(base * 2, "macos"))
    if shutil.which("iconutil"):
        subprocess.check_call(["iconutil", "-c", "icns", iconset, "-o", os.path.join(HERE, "icon.icns")])
    else:
        print("iconutil not found (not macOS?) - skipped icon.icns", file=sys.stderr)
    shutil.rmtree(tmp)
    print("done", file=sys.stderr)


if __name__ == "__main__":
    main()
