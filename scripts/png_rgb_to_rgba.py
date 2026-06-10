#!/usr/bin/env python3
import binascii
import struct
import sys
import zlib


def chunk(name, data):
    return (
        struct.pack(">I", len(data))
        + name
        + data
        + struct.pack(">I", binascii.crc32(name + data) & 0xFFFFFFFF)
    )


def paeth(a, b, c):
    p = a + b - c
    pa = abs(p - a)
    pb = abs(p - b)
    pc = abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    if pb <= pc:
        return b
    return c


def unfilter(raw, width, height, channels):
    stride = width * channels
    out = bytearray(height * stride)
    source = memoryview(raw)
    pos = 0
    for y in range(height):
        filter_type = source[pos]
        pos += 1
        row = bytearray(source[pos : pos + stride])
        pos += stride
        prev_offset = (y - 1) * stride

        for x in range(stride):
            left = row[x - channels] if x >= channels else 0
            up = out[prev_offset + x] if y > 0 else 0
            up_left = out[prev_offset + x - channels] if y > 0 and x >= channels else 0

            if filter_type == 1:
                row[x] = (row[x] + left) & 0xFF
            elif filter_type == 2:
                row[x] = (row[x] + up) & 0xFF
            elif filter_type == 3:
                row[x] = (row[x] + ((left + up) // 2)) & 0xFF
            elif filter_type == 4:
                row[x] = (row[x] + paeth(left, up, up_left)) & 0xFF
            elif filter_type != 0:
                raise ValueError(f"unsupported PNG filter {filter_type}")

        out[y * stride : (y + 1) * stride] = row
    return out


def convert(path_in, path_out):
    data = open(path_in, "rb").read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("not a PNG")

    pos = 8
    ihdr = None
    idat = bytearray()
    while pos < len(data):
        length = struct.unpack(">I", data[pos : pos + 4])[0]
        pos += 4
        name = data[pos : pos + 4]
        pos += 4
        payload = data[pos : pos + length]
        pos += length + 4
        if name == b"IHDR":
            ihdr = payload
        elif name == b"IDAT":
            idat.extend(payload)
        elif name == b"IEND":
            break

    if ihdr is None:
        raise ValueError("missing IHDR")

    width, height, bit_depth, color_type, compression, png_filter, interlace = struct.unpack(
        ">IIBBBBB", ihdr
    )
    if bit_depth != 8 or color_type not in (2, 6) or compression != 0 or png_filter != 0 or interlace != 0:
        raise ValueError("only non-interlaced 8-bit RGB/RGBA PNG is supported")

    channels = 3 if color_type == 2 else 4
    pixels = unfilter(zlib.decompress(bytes(idat)), width, height, channels)

    raw = bytearray()
    for y in range(height):
        raw.append(0)
        row = pixels[y * width * channels : (y + 1) * width * channels]
        for x in range(width):
            src = x * channels
            raw.extend(row[src : src + 3])
            raw.append(row[src + 3] if channels == 4 else 255)

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")
    open(path_out, "wb").write(png)


if __name__ == "__main__":
    convert(sys.argv[1], sys.argv[2])
