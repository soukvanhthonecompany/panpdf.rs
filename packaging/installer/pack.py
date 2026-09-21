#!/usr/bin/env python3
"""Build the installer's payload, with nothing but the standard library.

    pack.py archive <stage-dir> <archive.bin>   pack a directory
    pack.py append <installer.exe> <archive.bin> <out.exe>
                                                glue the payload onto the stub
    pack.py list <installer.exe>                read the payload back
    pack.py --self-test <cases.bin>             known answers for inflate_test

The format is ours and is described here once, because `installer.c` reads it
and nothing else ever will:

    "PANPDFAR"              8 bytes
    u32 count
    count x table entry:
        u32 name_len, name (UTF-8, '/' between folders)
        u8  method           0 stored, 1 raw deflate
        u32 compressed_len
        u32 raw_len
        u32 crc32 of the raw bytes
    the compressed bodies, in table order, one after another

and the last sixteen bytes of the finished installer are

    u64 offset of "PANPDFAR" from the start of the file
    "PANPDFTR"              8 bytes

Every number is little-endian. A file whose deflate is not smaller than the
file is stored instead, so an already-compressed font is not paid for twice.
"""

import os
import struct
import sys
import zlib

MAGIC = b"PANPDFAR"
TRAILER = b"PANPDFTR"


def pack(stage, out):
    names = []
    for folder, _, files in os.walk(stage):
        for name in files:
            full = os.path.join(folder, name)
            names.append((os.path.relpath(full, stage).replace(os.sep, "/"), full))
    names.sort()

    table, bodies, raw_total = b"", [], 0
    for name, full in names:
        raw = open(full, "rb").read()
        squeezed = zlib.compress(raw, 9)[2:-4]  # raw deflate: no zlib wrapper
        method, body = (1, squeezed) if len(squeezed) < len(raw) else (0, raw)
        encoded = name.encode()
        table += struct.pack("<I", len(encoded)) + encoded
        table += struct.pack("<BIII", method, len(body), len(raw), zlib.crc32(raw) & 0xFFFFFFFF)
        bodies.append(body)
        raw_total += len(raw)

    blob = MAGIC + struct.pack("<I", len(names)) + table + b"".join(bodies)
    open(out, "wb").write(blob)
    print(f"  payload: {len(names)} files, {raw_total / 1e6:.1f} MB -> {len(blob) / 1e6:.1f} MB")


def append(stub, archive, out):
    body = open(stub, "rb").read()
    offset = len(body)
    body += open(archive, "rb").read()
    body += struct.pack("<Q", offset) + TRAILER
    open(out, "wb").write(body)
    os.chmod(out, 0o755)
    print(f"  {os.path.basename(out)}: {len(body) / 1e6:.1f} MB")


def read(path):
    """Read a finished installer back the way installer.c does, and check it."""
    blob = open(path, "rb").read()
    if blob[-8:] != TRAILER:
        raise SystemExit(f"{path}: no payload trailer")
    (offset,) = struct.unpack("<Q", blob[-16:-8])
    if blob[offset : offset + 8] != MAGIC:
        raise SystemExit(f"{path}: the trailer points at no archive")
    at = offset + 8
    (count,) = struct.unpack("<I", blob[at : at + 4])
    at += 4
    entries = []
    for _ in range(count):
        (name_len,) = struct.unpack("<I", blob[at : at + 4])
        at += 4
        name = blob[at : at + name_len].decode()
        at += name_len
        method, comp, raw, crc = struct.unpack("<BIII", blob[at : at + 13])
        at += 13
        entries.append((name, method, comp, raw, crc))
    for name, method, comp, raw, crc in entries:
        body = blob[at : at + comp]
        at += comp
        got = zlib.decompress(body, -15) if method else body
        if len(got) != raw or zlib.crc32(got) & 0xFFFFFFFF != crc:
            raise SystemExit(f"{path}: {name} does not check out")
        print(f"  {'deflate' if method else 'stored ':>7} {comp:>9} -> {raw:>9}  {name}")
    print(f"  {count} files, all of them check out")


def self_test(out):
    """Known answers for `inflate_test`: every block kind, and corrupt input."""
    import random

    random.seed(20260921)
    samples = [
        ("empty", b""),
        ("one byte", b"A"),
        ("repetitive", b"PanPDF " * 5000),
        ("text", ("the quick brown fox jumps over the lazy dog. " * 300).encode()),
        ("incompressible", bytes(random.randrange(256) for _ in range(40000))),
        ("long run", b"\x00" * 300000),
        ("mixed", (b"\xff\x00" * 1000) + bytes(range(256)) * 40 + b"tail"),
    ]
    cases = []
    for name, raw in samples:
        for level, tag in ((0, "stored"), (1, "fast"), (9, "best")):
            comp = zlib.compress(raw, level)[2:-4]
            cases.append((f"{name}/{tag}", 0, comp, raw))
    # A fixed-Huffman block, which zlib only emits for tiny inputs.
    tiny = b"aaaaaaaa"
    cases.append(("tiny/fixed", 0, zlib.compress(tiny, 9)[2:-4], tiny))
    # Input that must be refused: truncated, and a wrong block type.
    good = zlib.compress(b"PanPDF " * 500, 9)[2:-4]
    cases.append(("truncated", 1, good[: len(good) // 2], b"PanPDF " * 500))
    cases.append(("reserved block type", 1, b"\x07\x00\x00\x00", b"x" * 8))
    cases.append(("claims too few bytes", 1, good, b"PanPDF " * 400))

    blob = struct.pack("<I", len(cases))
    for name, must_fail, comp, raw in cases:
        encoded = name.encode()
        blob += struct.pack("<I", len(encoded)) + encoded
        blob += struct.pack("<BII", must_fail, len(comp), len(raw)) + comp + raw
    open(out, "wb").write(blob)
    print(f"  {len(cases)} cases written")


if __name__ == "__main__":
    args = sys.argv[1:]
    if args[:1] == ["archive"] and len(args) == 3:
        pack(args[1], args[2])
    elif args[:1] == ["append"] and len(args) == 4:
        append(args[1], args[2], args[3])
    elif args[:1] == ["list"] and len(args) == 2:
        read(args[1])
    elif args[:1] == ["--self-test"] and len(args) == 2:
        self_test(args[1])
    else:
        raise SystemExit(__doc__)
