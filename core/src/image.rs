//! Self-contained PNG decoder (inflate + unfiltering + colour conversion) and image scaling.

use crate::gfx::Canvas;

#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub w: usize,
    pub h: usize,
    /// Straight-alpha RGBA, 4 bytes per pixel.
    pub rgba: Vec<u8>,
}

pub const MAX_DIM: usize = 4096;

// ---------------------------------------------------------------- inflate

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    buf: u32,
    cnt: u32,
}

impl<'a> Bits<'a> {
    fn bits(&mut self, n: u32) -> Result<u32, &'static str> {
        while self.cnt < n {
            let b = *self.d.get(self.pos).ok_or("unexpected end of data")?;
            self.pos += 1;
            self.buf |= (b as u32) << self.cnt;
            self.cnt += 8;
        }
        let v = self.buf & ((1u32 << n) - 1);
        self.buf >>= n;
        self.cnt -= n;
        Ok(v)
    }
}

struct Huff {
    count: [u16; 16],
    symbol: Vec<u16>,
}

impl Huff {
    fn new(lens: &[u8]) -> Huff {
        let mut count = [0u16; 16];
        for &l in lens {
            count[l as usize] += 1;
        }
        count[0] = 0;
        let mut offs = [0u16; 16];
        for i in 1..16 {
            offs[i] = offs[i - 1] + count[i - 1];
        }
        let mut symbol = vec![0u16; lens.len()];
        for (s, &l) in lens.iter().enumerate() {
            if l != 0 {
                symbol[offs[l as usize] as usize] = s as u16;
                offs[l as usize] += 1;
            }
        }
        Huff { count, symbol }
    }
    fn decode(&self, b: &mut Bits) -> Result<u16, &'static str> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= b.bits(1)? as i32;
            let count = self.count[len] as i32;
            if code - count < first {
                return self.symbol.get((index + (code - first)) as usize).copied().ok_or("bad huffman code");
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err("bad huffman code")
    }
}

const LBASE: [u16; 29] =
    [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEXT: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DBASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145,
    8193, 12289, 16385, 24577,
];
const DEXT: [u8; 30] =
    [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

/// Decompresses a zlib stream; `limit` caps the output size.
pub fn inflate(data: &[u8], limit: usize) -> Result<Vec<u8>, &'static str> {
    if data.len() < 2 || data[0] & 15 != 8 {
        return Err("not a zlib stream");
    }
    let mut b = Bits { d: data, pos: 2, buf: 0, cnt: 0 };
    let mut out: Vec<u8> = Vec::new();
    loop {
        let last = b.bits(1)?;
        match b.bits(2)? {
            0 => {
                b.buf = 0;
                b.cnt = 0;
                let p = b.pos;
                if p + 4 > data.len() {
                    return Err("unexpected end of data");
                }
                let len = u16::from_le_bytes([data[p], data[p + 1]]) as usize;
                if p + 4 + len > data.len() || out.len() + len > limit {
                    return Err("bad stored block");
                }
                out.extend_from_slice(&data[p + 4..p + 4 + len]);
                b.pos = p + 4 + len;
            }
            t @ (1 | 2) => {
                let (lit, dist) = if t == 1 {
                    let mut l = [8u8; 288];
                    l[144..256].fill(9);
                    l[256..280].fill(7);
                    (Huff::new(&l), Huff::new(&[5u8; 30]))
                } else {
                    let nlen = b.bits(5)? as usize + 257;
                    let ndist = b.bits(5)? as usize + 1;
                    let ncode = b.bits(4)? as usize + 4;
                    if nlen > 286 || ndist > 30 {
                        return Err("bad block header");
                    }
                    const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
                    let mut cl = [0u8; 19];
                    for &o in ORDER.iter().take(ncode) {
                        cl[o] = b.bits(3)? as u8;
                    }
                    let ch = Huff::new(&cl);
                    let mut lens = [0u8; 316];
                    let mut i = 0;
                    while i < nlen + ndist {
                        let sym = ch.decode(&mut b)?;
                        let (val, rep) = match sym {
                            0..=15 => (sym as u8, 1),
                            16 => {
                                if i == 0 {
                                    return Err("bad code lengths");
                                }
                                (lens[i - 1], 3 + b.bits(2)? as usize)
                            }
                            17 => (0, 3 + b.bits(3)? as usize),
                            _ => (0, 11 + b.bits(7)? as usize),
                        };
                        if i + rep > nlen + ndist {
                            return Err("bad code lengths");
                        }
                        lens[i..i + rep].fill(val);
                        i += rep;
                    }
                    (Huff::new(&lens[..nlen]), Huff::new(&lens[nlen..nlen + ndist]))
                };
                loop {
                    let sym = lit.decode(&mut b)? as usize;
                    if sym < 256 {
                        if out.len() >= limit {
                            return Err("image too large");
                        }
                        out.push(sym as u8);
                    } else if sym == 256 {
                        break;
                    } else {
                        let s = sym - 257;
                        if s >= 29 {
                            return Err("bad length code");
                        }
                        let len = LBASE[s] as usize + b.bits(LEXT[s] as u32)? as usize;
                        let ds = dist.decode(&mut b)? as usize;
                        if ds >= 30 {
                            return Err("bad distance code");
                        }
                        let d = DBASE[ds] as usize + b.bits(DEXT[ds] as u32)? as usize;
                        if d > out.len() || out.len() + len > limit {
                            return Err("bad distance");
                        }
                        let start = out.len() - d;
                        for i in 0..len {
                            out.push(out[start + i]);
                        }
                    }
                }
            }
            _ => return Err("bad block type"),
        }
        if last == 1 {
            return Ok(out);
        }
    }
}

// ---------------------------------------------------------------- PNG

pub fn is_png(d: &[u8]) -> bool {
    d.len() > 8 && d[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
}

pub fn decode_png(d: &[u8]) -> Result<Image, &'static str> {
    if !is_png(d) {
        return Err("not a PNG file");
    }
    let be32 = |o: usize| u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]) as usize;
    let (mut w, mut h, mut depth, mut ctype) = (0usize, 0usize, 0u8, 0u8);
    let mut plte: &[u8] = &[];
    let mut trns: &[u8] = &[];
    let mut idat: Vec<u8> = Vec::new();
    let mut o = 8;
    while o + 8 <= d.len() {
        let len = be32(o);
        let kind = &d[o + 4..o + 8];
        let body = o + 8;
        if len > d.len() || body + len + 4 > d.len() {
            break;
        }
        let data = &d[body..body + len];
        match kind {
            b"IHDR" => {
                if len < 13 {
                    return Err("bad header");
                }
                w = be32(body);
                h = be32(body + 4);
                depth = data[8];
                ctype = data[9];
                if data[12] != 0 {
                    return Err("interlaced PNGs are not supported");
                }
            }
            b"PLTE" => plte = data,
            b"tRNS" => trns = data,
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        o = body + len + 4;
    }
    if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM {
        return Err("unsupported image size");
    }
    let ch = match ctype {
        0 | 3 => 1,
        2 => 3,
        4 => 2,
        6 => 4,
        _ => return Err("unsupported colour type"),
    };
    let ok_depth = match ctype {
        0 => matches!(depth, 1 | 2 | 4 | 8 | 16),
        3 => matches!(depth, 1 | 2 | 4 | 8),
        _ => matches!(depth, 8 | 16),
    };
    if !ok_depth {
        return Err("unsupported bit depth");
    }
    let bpp = ((ch * depth as usize) + 7) / 8;
    let stride = (w * ch * depth as usize + 7) / 8;
    let mut raw = inflate(&idat, (stride + 1) * h)?;
    if raw.len() < (stride + 1) * h {
        return Err("truncated image data");
    }
    // Undo scanline filters in place.
    for y in 0..h {
        let s = y * (stride + 1);
        let ft = raw[s];
        for i in 0..stride {
            let a = if i >= bpp { raw[s + 1 + i - bpp] as i32 } else { 0 };
            let b = if y > 0 { raw[s - stride + i] as i32 } else { 0 };
            let c = if y > 0 && i >= bpp { raw[s - stride + i - bpp] as i32 } else { 0 };
            let x = raw[s + 1 + i] as i32;
            let v = match ft {
                0 => x,
                1 => x + a,
                2 => x + b,
                3 => x + ((a + b) >> 1),
                4 => {
                    let p = a + b - c;
                    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                    x + if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c }
                }
                _ => return Err("bad filter"),
            };
            raw[s + 1 + i] = v as u8;
        }
    }
    let mut rgba = vec![255u8; w * h * 4];
    let t16 = |i: usize| if trns.len() >= i * 2 + 2 { Some(u16::from_be_bytes([trns[i * 2], trns[i * 2 + 1]])) } else { None };
    for y in 0..h {
        let row = &raw[y * (stride + 1) + 1..y * (stride + 1) + 1 + stride];
        // Sample `i` of the row as (8-bit value, raw value).
        let sample = |i: usize| -> (u8, u16) {
            match depth {
                8 => (row[i], row[i] as u16),
                16 => (row[i * 2], u16::from_be_bytes([row[i * 2], row[i * 2 + 1]])),
                _ => {
                    let bit = i * depth as usize;
                    let v = (row[bit / 8] >> (8 - depth as usize - bit % 8)) & ((1u8 << depth) - 1);
                    (v, v as u16)
                }
            }
        };
        for x in 0..w {
            let o = (y * w + x) * 4;
            match ctype {
                0 => {
                    let (v, rawv) = sample(x);
                    let g = if depth < 8 { (v as u32 * 255 / ((1u32 << depth) - 1)) as u8 } else { v };
                    rgba[o..o + 3].fill(g);
                    if t16(0) == Some(rawv) {
                        rgba[o + 3] = 0;
                    }
                }
                2 => {
                    let (r, g, b) = (sample(x * 3), sample(x * 3 + 1), sample(x * 3 + 2));
                    rgba[o] = r.0;
                    rgba[o + 1] = g.0;
                    rgba[o + 2] = b.0;
                    if trns.len() >= 6 && t16(0) == Some(r.1) && t16(1) == Some(g.1) && t16(2) == Some(b.1) {
                        rgba[o + 3] = 0;
                    }
                }
                3 => {
                    let i = sample(x).0 as usize;
                    if i * 3 + 2 < plte.len() {
                        rgba[o..o + 3].copy_from_slice(&plte[i * 3..i * 3 + 3]);
                    }
                    if i < trns.len() {
                        rgba[o + 3] = trns[i];
                    }
                }
                4 => {
                    rgba[o..o + 3].fill(sample(x * 2).0);
                    rgba[o + 3] = sample(x * 2 + 1).0;
                }
                _ => {
                    for k in 0..4 {
                        rgba[o + k] = sample(x * 4 + k).0;
                    }
                }
            }
        }
    }
    Ok(Image { w, h, rgba })
}

// ---------------------------------------------------------------- scaling

#[inline]
fn premul(p: &[u8]) -> [f32; 4] {
    let a = p[3] as f32 / 255.0;
    [a * 255.0, p[0] as f32 * a, p[1] as f32 * a, p[2] as f32 * a]
}

#[inline]
fn pack_f(v: [f32; 4]) -> u32 {
    let a = v[0].clamp(0.0, 255.0);
    let c = |x: f32| (x.clamp(0.0, a) + 0.5) as u32;
    (((a + 0.5) as u32) << 24) | (c(v[1]) << 16) | (c(v[2]) << 8) | c(v[3])
}

/// Scales `img` to exactly `dw` x `dh` into a premultiplied canvas.
/// Downscaling always area-averages; upscaling is bilinear (`smooth`) or nearest.
pub fn scale_to(img: &Image, dw: usize, dh: usize, smooth: bool) -> Canvas {
    let (dw, dh) = (dw.clamp(1, 2048), dh.clamp(1, 2048));
    let mut out = Canvas::new(dw as i32, dh as i32);
    let (sx, sy) = (img.w as f32 / dw as f32, img.h as f32 / dh as f32);
    let at = |x: usize, y: usize| premul(&img.rgba[(y * img.w + x) * 4..(y * img.w + x) * 4 + 4]);
    for y in 0..dh {
        for x in 0..dw {
            let v = if sx > 1.0 || sy > 1.0 {
                // Box filter over the source footprint.
                let (x0, x1) = (x as f32 * sx, (x as f32 + 1.0) * sx);
                let (y0, y1) = (y as f32 * sy, (y as f32 + 1.0) * sy);
                let mut acc = [0f32; 4];
                let mut wsum = 0.0;
                for yy in (y0.floor() as usize)..(y1.ceil() as usize).min(img.h) {
                    let wy = y1.min(yy as f32 + 1.0) - y0.max(yy as f32);
                    for xx in (x0.floor() as usize)..(x1.ceil() as usize).min(img.w) {
                        let wgt = wy * (x1.min(xx as f32 + 1.0) - x0.max(xx as f32));
                        if wgt > 0.0 {
                            let p = at(xx, yy);
                            for k in 0..4 {
                                acc[k] += p[k] * wgt;
                            }
                            wsum += wgt;
                        }
                    }
                }
                if wsum > 0.0 {
                    for a in &mut acc {
                        *a /= wsum;
                    }
                }
                acc
            } else if smooth {
                let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, img.w as f32 - 1.0);
                let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, img.h as f32 - 1.0);
                let (ix, iy) = (fx.floor() as usize, fy.floor() as usize);
                let (jx, jy) = ((ix + 1).min(img.w - 1), (iy + 1).min(img.h - 1));
                let (tx, ty) = (fx - ix as f32, fy - iy as f32);
                let (a, b, c, d) = (at(ix, iy), at(jx, iy), at(ix, jy), at(jx, jy));
                let mut r = [0f32; 4];
                for k in 0..4 {
                    r[k] = (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty;
                }
                r
            } else {
                at((((x as f32 + 0.5) * sx) as usize).min(img.w - 1), (((y as f32 + 0.5) * sy) as usize).min(img.h - 1))
            };
            out.px[y * dw + x] = pack_f(v);
        }
    }
    out
}
