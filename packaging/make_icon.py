#!/usr/bin/env python3
"""Pack rendered PNGs into a Windows .ico, with nothing but the standard library.

    packaging/make_icon.py out.ico 16.png 32.png ...

Sizes up to 64 are written as BMP (DIB) entries, which every Windows shell
since 95 reads; 128 and 256 are written as the PNG itself, which is what
Vista and later expect and what keeps the file small. `--check` re-reads the
result and prints one line per entry, so the script can prove itself.
"""

import struct
import sys
import zlib


CHANNELS = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}


def read_png(path):
    """Return (width, height, rgba_rows) for any 8-bit PNG.

    Rasterisers disagree about what they emit -- Inkscape writes a palette for a
    16-pixel icon and RGBA for a 256-pixel one -- so every 8-bit colour type is
    accepted and turned into RGBA here."""
    blob = open(path, "rb").read()
    if blob[:8] != b"\x89PNG\r\n\x1a\n":
        raise SystemExit(f"{path}: not a PNG")
    pos, idat, width, height, colour = 8, b"", 0, 0, 0
    palette, transparency = b"", b""
    while pos < len(blob):
        (length,) = struct.unpack(">I", blob[pos : pos + 4])
        kind = blob[pos + 4 : pos + 8]
        body = blob[pos + 8 : pos + 8 + length]
        if kind == b"IHDR":
            width, height, depth, colour = struct.unpack(">IIBB", body[:10])
            if depth != 8 or colour not in CHANNELS:
                raise SystemExit(f"{path}: want an 8-bit PNG, got depth {depth} colour {colour}")
        elif kind == b"PLTE":
            palette = body
        elif kind == b"tRNS":
            transparency = body
        elif kind == b"IDAT":
            idat += body
        elif kind == b"IEND":
            break
        pos += 12 + length

    channels = CHANNELS[colour]
    raw = zlib.decompress(idat)
    stride = width * channels
    rows, previous = [], bytearray(stride)
    at = 0
    for _ in range(height):
        filt = raw[at]
        line = bytearray(raw[at + 1 : at + 1 + stride])
        at += 1 + stride
        for i in range(stride):
            left = line[i - channels] if i >= channels else 0
            up = previous[i]
            upleft = previous[i - channels] if i >= channels else 0
            if filt == 1:
                line[i] = (line[i] + left) & 0xFF
            elif filt == 2:
                line[i] = (line[i] + up) & 0xFF
            elif filt == 3:
                line[i] = (line[i] + (left + up) // 2) & 0xFF
            elif filt == 4:
                p = left + up - upleft
                pa, pb, pc = abs(p - left), abs(p - up), abs(p - upleft)
                best = left if (pa <= pb and pa <= pc) else (up if pb <= pc else upleft)
                line[i] = (line[i] + best) & 0xFF
            elif filt != 0:
                raise SystemExit(f"{path}: unknown row filter {filt}")
        previous = line
        rgba = bytearray()
        if colour == 6:
            rgba = line
        elif colour == 2:
            for i in range(0, stride, 3):
                rgba += line[i : i + 3] + b"\xff"
        elif colour == 4:
            for i in range(0, stride, 2):
                grey = line[i]
                rgba += bytes((grey, grey, grey, line[i + 1]))
        elif colour == 0:
            for grey in line:
                rgba += bytes((grey, grey, grey, 255))
        else:
            for index in line:
                at = index * 3
                alpha = transparency[index] if index < len(transparency) else 255
                rgba += palette[at : at + 3] + bytes((alpha,))
        rows.append(bytes(rgba))
    return width, height, rows


def as_dib(width, height, rows):
    """A 32-bit bottom-up DIB with the AND mask an .ico entry still needs."""
    header = struct.pack(
        "<IiiHHIIiiII", 40, width, height * 2, 1, 32, 0, width * height * 4, 0, 0, 0, 0
    )
    pixels = bytearray()
    for row in reversed(rows):
        for i in range(0, len(row), 4):
            r, g, b, a = row[i : i + 4]
            pixels += bytes((b, g, r, a))
    mask_stride = ((width + 31) // 32) * 4
    mask = bytes(mask_stride * height)
    return header + bytes(pixels) + mask


def build(out, pngs):
    entries, blobs = [], []
    for path in pngs:
        width, height, rows = read_png(path)
        if width != height:
            raise SystemExit(f"{path}: {width}x{height} is not square")
        # A rasteriser that cannot read the SVG returns something nearly
        # transparent rather than failing, and the icon then shows as a blank
        # or black square with no error anywhere. Refuse it here instead.
        opaque = sum(1 for row in rows for i in range(3, len(row), 4) if row[i] > 128)
        if opaque * 20 < width * height:
            raise SystemExit(
                f"{path}: only {opaque} of {width * height} pixels are opaque -- "
                "the rasteriser did not draw the image"
            )
        blobs.append(open(path, "rb").read() if width >= 128 else as_dib(width, height, rows))
        entries.append(width)

    offset = 6 + 16 * len(entries)
    out_bytes = struct.pack("<HHH", 0, 1, len(entries))
    for size, blob in zip(entries, blobs):
        out_bytes += struct.pack(
            "<BBBBHHII", size & 0xFF, size & 0xFF, 0, 0, 1, 32, len(blob), offset
        )
        offset += len(blob)
    open(out, "wb").write(out_bytes + b"".join(blobs))
    return entries


def check(path):
    blob = open(path, "rb").read()
    reserved, kind, count = struct.unpack("<HHH", blob[:6])
    if reserved != 0 or kind != 1 or count == 0:
        raise SystemExit(f"{path}: not an icon file")
    for i in range(count):
        w, h, _, _, planes, bpp, size, offset = struct.unpack(
            "<BBBBHHII", blob[6 + 16 * i : 22 + 16 * i]
        )
        body = blob[offset : offset + size]
        if len(body) != size:
            raise SystemExit(f"{path}: entry {i} runs past the end of the file")
        shape = "png" if body[:8] == b"\x89PNG\r\n\x1a\n" else "dib"
        print(f"  {w or 256:>3}x{h or 256:<3} {bpp}bpp {shape} {size} bytes")
    return count


if __name__ == "__main__":
    args = sys.argv[1:]
    if args[:1] == ["--check"]:
        check(args[1])
    elif len(args) >= 2:
        build(args[0], args[1:])
        check(args[0])
    else:
        raise SystemExit(__doc__)
