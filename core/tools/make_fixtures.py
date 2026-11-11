# Generates PNG decoder fixtures: each .png has a matching .rgba (w, h as u32 LE + raw RGBA8).
import struct, random, zlib
from PIL import Image
random.seed(7)
def noise(mode, w, h):
    img = Image.new("RGBA", (w, h))
    px = img.load()
    for y in range(h):
        for x in range(w):
            px[x, y] = ((x * 9 + y * 3) % 256, (x * x + y) % 256, random.randrange(256), (x * 5 + y * 11) % 256 if (x + y) % 3 else 0)
    return img
def save(name, img, **kw):
    img.save(f"tests/fixtures/{name}.png", **kw)
    back = Image.open(f"tests/fixtures/{name}.png").convert("RGBA")
    open(f"tests/fixtures/{name}.rgba", "wb").write(struct.pack("<II", back.width, back.height) + back.tobytes())
base = noise("RGBA", 37, 29)
save("rgba8", base)
save("rgb8", base.convert("RGB"))
save("gray8", base.convert("L"))
save("graya8", base.convert("LA"))
save("pal8", base.convert("RGB").quantize(64))
p = base.convert("RGB").quantize(16); p.info["transparency"] = 3
save("pal_trns", p, transparency=3)
save("gray1", base.convert("1"))
save("stored", base, compress_level=0)
save("big", noise("RGBA", 300, 200), compress_level=9)
# crosshair-like image with a transparent background
x = Image.new("RGBA", (64, 64), (0, 0, 0, 0))
for i in range(64):
    for t in (31, 32):
        if abs(i - 32) > 6:
            x.putpixel((i, t), (255, 40, 40, 255)); x.putpixel((t, i), (255, 40, 40, 255))
save("cross", x)
# 16-bit RGBA written by hand (PIL cannot save RGBA16)
w, h = 5, 4
raw = b""
exp = b""
for yy in range(h):
    raw += b"\0"
    for xx in range(w):
        v = [(xx * 13000 + yy * 700) % 65536, (yy * 16000) % 65536, 65535 - xx * 9000, 65535 if xx else 0]
        raw += struct.pack(">4H", *v); exp += bytes(c >> 8 for c in v)
def chunk(t, d): return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 16, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
open("tests/fixtures/rgba16.png", "wb").write(png)
open("tests/fixtures/rgba16.rgba", "wb").write(struct.pack("<II", w, h) + exp)
