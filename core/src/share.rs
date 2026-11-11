//! Share codes: a crosshair as a short line of text ("AIM1:..."), for Discord and friends. Pasting the code into
//! Aimdot gives exactly the same crosshair. Works for a Designer crosshair and for a painted pixel crosshair.
//!
//! Text: `AIM1:` + base64url (no padding) of a small binary:
//!
//! ```text
//! [head]    1 byte: bit 0 = a pixel picture follows. Other bits: must be 0 (a newer Aimdot's code otherwise)
//! [fields]  (id, value) pairs, only the settings that differ from the AIM1 base values below; id 0 ends them
//! [pixels]  only when head bit 0: canvas w, h; painted box x, y, w, h; flags; palette; runs (see `put_pixels`)
//! [check]   2 bytes Fletcher-16 of everything before it (a typo or a cut-off code is refused)
//! ```
//!
//! The field ids and base values are FROZEN for AIM1: new settings get new ids, old ids never change meaning
//! (base values are fixed here, not taken from `Profile::default()`, so a changed default can't change old codes).
//! Numbers are stored in the UI's own steps: lengths in half pixels, opacity / saturation in 1 %, scale in 1 %,
//! rotation in half degrees, rainbow speed in 0.05. A value between two steps comes back rounded to the nearest one.
//! Not in a code: the image file name (a picture travels as its pixels instead), and anything that isn't the
//! crosshair itself (position, hotkeys, which games use it).

use crate::config::Profile;
use crate::gfx::Color;
use crate::image::Image;

pub const PREFIX: &str = "AIM1:";
/// Longest code accepted (characters after the prefix). A full 63 px painting with many colours fits easily.
pub const MAX_CODE: usize = 8000;
/// Largest picture (each side) that can travel in a code. The painter's canvas is at most 63 (+2 for the outline).
pub const MAX_SIDE: usize = 128;
/// Most colours in a picture (transparent not counted).
pub const MAX_COLOURS: usize = 255;
/// Longest name in a code (bytes of UTF-8).
pub const MAX_NAME: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// No "AIM1:" code in the text.
    NoCode,
    /// The code is damaged: a typo, cut off, or not an Aimdot code.
    Damaged,
    /// Made by a newer Aimdot (unknown parts).
    Newer,
    /// Too long to be a crosshair code.
    TooLong,
    /// Encoding: the picture is bigger than MAX_SIDE or has more than MAX_COLOURS colours.
    PictureTooBig,
    /// Encoding: an image crosshair without its pixels (the caller must load the picture first).
    NoPicture,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Error::NoCode => "No Aimdot code found - it starts with AIM1:",
            Error::Damaged => "That code is damaged or cut off - copy it again",
            Error::Newer => "That code is from a newer Aimdot - update Aimdot to use it",
            Error::TooLong => "That code is too long to be a crosshair",
            Error::PictureTooBig => "This picture is too big for a code (at most 128 x 128 px and 255 colours)",
            Error::NoPicture => "The picture of this crosshair could not be read",
        })
    }
}

/// What a code holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Shared {
    /// The name the sender gave it ("" when none).
    pub name: String,
    /// The crosshair. With a picture: `mode` = 1 and `image` is empty - save `pixels` as a PNG, then set `image`.
    pub profile: Profile,
    /// A painted / pixel picture (straight RGBA). Animated: all frames side by side (a strip `frames` wide).
    pub pixels: Option<Image>,
    /// Animated picture: (frames, frames per second). None = a still picture.
    pub anim: Option<(u32, u32)>,
}

// ------------------------------------------------------------------ fields (FROZEN for AIM1)

#[derive(Clone, Copy)]
enum Q {
    /// No value: the id's presence means "the opposite of the base".
    Flip,
    /// RGB, 3 bytes.
    Rgb,
    /// A small number 0..=255 as it is (modes).
    Byte,
    /// x2, 1 byte (0..=127.5 px).
    Half,
    /// x100, 1 byte (0..=1).
    Pct,
    /// x20, 1 byte (rainbow speed 0.1..=5).
    Twentieth,
    /// x100, 2 bytes (scale 0.05..=4).
    Hundredth16,
    /// x2, 2 bytes (rotation 0..=359).
    HalfDeg16,
}

trait Field: Sized + Clone {
    /// Appends the value if it differs from the base; false = nothing written (the id is dropped again).
    fn put(&self, q: Q, base: &Self, out: &mut Vec<u8>) -> bool;
    fn take(q: Q, base: &Self, r: &mut Reader) -> Result<Self, Error>;
}

fn quant(v: f32, q: Q) -> u32 {
    let k = match q {
        Q::Half | Q::HalfDeg16 => 2.0,
        Q::Pct | Q::Hundredth16 => 100.0,
        Q::Twentieth => 20.0,
        _ => 1.0,
    };
    let max = match q {
        Q::Hundredth16 | Q::HalfDeg16 => 65535.0,
        _ => 255.0,
    };
    let v = if v.is_finite() { v } else { 0.0 };
    (v * k).round().clamp(0.0, max) as u32
}

fn unquant(n: u32, q: Q) -> f32 {
    let k = match q {
        Q::Half | Q::HalfDeg16 => 2.0,
        Q::Pct | Q::Hundredth16 => 100.0,
        Q::Twentieth => 20.0,
        _ => 1.0,
    };
    n as f32 / k
}

impl Field for f32 {
    fn put(&self, q: Q, base: &Self, out: &mut Vec<u8>) -> bool {
        let n = quant(*self, q);
        if n == quant(*base, q) {
            return false;
        }
        match q {
            Q::Hundredth16 | Q::HalfDeg16 => out.extend_from_slice(&(n as u16).to_le_bytes()),
            _ => out.push(n as u8),
        }
        true
    }
    fn take(q: Q, _: &Self, r: &mut Reader) -> Result<Self, Error> {
        let n = match q {
            Q::Hundredth16 | Q::HalfDeg16 => r.u16()? as u32,
            _ => r.u8()? as u32,
        };
        Ok(unquant(n, q))
    }
}

impl Field for bool {
    fn put(&self, _: Q, base: &Self, _: &mut Vec<u8>) -> bool {
        self != base
    }
    fn take(_: Q, base: &Self, _: &mut Reader) -> Result<Self, Error> {
        Ok(!base)
    }
}

impl Field for u32 {
    fn put(&self, _: Q, base: &Self, out: &mut Vec<u8>) -> bool {
        let n = (*self).min(255);
        if n == (*base).min(255) {
            return false;
        }
        out.push(n as u8);
        true
    }
    fn take(_: Q, _: &Self, r: &mut Reader) -> Result<Self, Error> {
        Ok(r.u8()? as u32)
    }
}

impl Field for Color {
    fn put(&self, _: Q, base: &Self, out: &mut Vec<u8>) -> bool {
        if (self.r, self.g, self.b) == (base.r, base.g, base.b) {
            return false;
        }
        out.extend_from_slice(&[self.r, self.g, self.b]);
        true
    }
    fn take(_: Q, _: &Self, r: &mut Reader) -> Result<Self, Error> {
        Ok(Color { r: r.u8()?, g: r.u8()?, b: r.u8()?, a: 255 })
    }
}

/// The id table. NEVER renumber or change a base value; add new fields at new ids (and keep 0 = end, 63 = name).
macro_rules! fields {
    ($($id:literal $f:ident $q:ident $base:expr;)*) => {
        fn put_fields(p: &Profile, out: &mut Vec<u8>) {
            $(
                out.push($id);
                let at = out.len();
                if !Field::put(&p.$f, Q::$q, &$base, out) {
                    out.truncate(at - 1);
                }
            )*
        }
        fn take_field(p: &mut Profile, id: u8, r: &mut Reader) -> Result<(), Error> {
            match id {
                $( $id => p.$f = Field::take(Q::$q, &$base, r)?, )*
                _ => return Err(Error::Newer),
            }
            Ok(())
        }
        fn base_profile() -> Profile {
            let mut p = Profile::default();
            $( p.$f = $base; )*
            p
        }
    };
}

fields! {
    1  color           Rgb         Color::hex(0x00FF7F);
    2  opacity         Pct         1.0f32;
    3  rainbow         Flip        false;
    4  rainbow_mode    Byte        0u32;
    5  rainbow_speed   Twentieth   1.0f32;
    6  rainbow_sat     Pct         1.0f32;
    7  outline         Flip        true;
    8  outline_thick   Half        1.0f32;
    9  outline_color   Rgb         Color::BLACK;
    10 outline_opacity Pct         1.0f32;
    11 dot             Flip        false;
    12 dot_size        Half        2.0f32;
    13 dot_round       Flip        false;
    14 dot_opacity     Pct         1.0f32;
    15 inner           Flip        true;
    16 inner_len       Half        6.0f32;
    17 inner_thick     Half        2.0f32;
    18 inner_gap       Half        3.0f32;
    19 inner_opacity   Pct         1.0f32;
    20 outer           Flip        false;
    21 outer_len       Half        3.0f32;
    22 outer_thick     Half        2.0f32;
    23 outer_gap       Half        12.0f32;
    24 outer_opacity   Pct         0.6f32;
    25 circle          Flip        false;
    26 circle_radius   Half        12.0f32;
    27 circle_thick    Half        2.0f32;
    28 circle_opacity  Pct         1.0f32;
    29 t_style         Flip        false;
    30 rotation        HalfDeg16   0.0f32;
    31 scale           Hundredth16 1.0f32;
    32 img_scale       Hundredth16 1.0f32;
    33 img_opacity     Pct         1.0f32;
    34 img_tint        Flip        false;
    35 img_smooth      Flip        true;
}
const ID_END: u8 = 0;
const ID_NAME: u8 = 63;
/// Animated picture: frames (2..=MAX_FRAMES) + fps (1..=60); the pixel part then holds one block per frame.
const ID_ANIM: u8 = 62;
pub const MAX_FRAMES: u32 = 16;
const HEAD_PIXELS: u8 = 1;

// ------------------------------------------------------------------ encode

/// A Designer crosshair (or any profile) as a code. An image crosshair needs its pixels: use `encode`.
pub fn encode_profile(name: &str, p: &Profile) -> Result<String, Error> {
    encode(name, p, None)
}

/// A painted / pixel picture alone (the image settings stay at their base values: 100 %, crisp, no tint).
pub fn encode_pixels(name: &str, img: &Image) -> Result<String, Error> {
    let p = Profile { mode: 1, img_smooth: false, ..base_profile() };
    encode(name, &p, Some(img))
}

/// The general form. `pixels` = the picture of an image crosshair (`p.mode == 1`); ignored for a Designer one.
pub fn encode(name: &str, p: &Profile, pixels: Option<&Image>) -> Result<String, Error> {
    encode_anim(name, p, pixels, None)
}

/// An animated picture crosshair: `strip` = all frames side by side (width = frames x frame width), `anim` =
/// (frames, fps). Each frame is packed on its own (MAX_SIDE per frame). `anim` None or 1 frame = a still picture.
pub fn encode_anim(name: &str, p: &Profile, pixels: Option<&Image>, anim: Option<(u32, u32)>) -> Result<String, Error> {
    let pixels = if p.mode == 1 { Some(pixels.ok_or(Error::NoPicture)?) } else { None };
    let anim = anim.filter(|&(f, _)| f > 1 && pixels.is_some());
    if let (Some((f, fps)), Some(img)) = (anim, pixels) {
        if f > MAX_FRAMES || !(1..=60).contains(&fps) || img.w % f as usize != 0 {
            return Err(Error::PictureTooBig);
        }
    }
    let mut out = vec![if pixels.is_some() { HEAD_PIXELS } else { 0 }];
    let name = clean_name(name);
    if !name.is_empty() {
        out.push(ID_NAME);
        out.push(name.len() as u8);
        out.extend_from_slice(name.as_bytes());
    }
    let mut q = p.clone();
    q.sanitize();
    if let Some((f, fps)) = anim {
        out.extend_from_slice(&[ID_ANIM, f as u8, fps as u8]);
    }
    put_fields(&q, &mut out);
    out.push(ID_END);
    if let Some(img) = pixels {
        let f = anim.map_or(1, |a| a.0 as usize);
        let fw = img.w / f;
        for i in 0..f {
            put_pixels(&frame(img, i, fw), &mut out)?;
        }
    }
    let sum = fletcher16(&out);
    out.extend_from_slice(&sum.to_le_bytes());
    let mut s = String::with_capacity(PREFIX.len() + out.len() * 4 / 3 + 4);
    s.push_str(PREFIX);
    b64_encode(&out, &mut s);
    Ok(s)
}

/// Frame `i` (width `fw`) of a strip.
fn frame(img: &Image, i: usize, fw: usize) -> Image {
    if fw == img.w {
        return img.clone();
    }
    let mut rgba = Vec::with_capacity(fw * img.h * 4);
    for y in 0..img.h {
        let a = (y * img.w + i * fw) * 4;
        rgba.extend_from_slice(&img.rgba[a..a + fw * 4]);
    }
    Image { w: fw, h: img.h, rgba }
}

/// A name that is safe in a code and in config.ini: no control characters or [ ] | =, at most MAX_NAME bytes.
pub fn clean_name(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().chars().filter(|c| !c.is_control() && !matches!(c, '[' | ']' | '|' | '=')) {
        if out.len() + c.len_utf8() > MAX_NAME {
            break;
        }
        out.push(c);
    }
    out.trim().to_string()
}

/// Pixel block:
/// `w h` canvas size (1..=MAX_SIDE); `bx by bw bh` the box that holds every painted pixel (bw = 0: empty);
/// `flags` bit 0 left/right mirror, bit 1 up/down mirror (only the first half is stored), bit 2 wide runs;
/// `n` colours + n x RGBA (index 0 = transparent, not stored); then runs over the stored part, row by row.
/// Narrow runs (n <= 15): one byte `index << 4 | k`, length k + 1 for k < 15, else 16 + varint.
/// Wide runs: index byte + varint(length - 1).
fn put_pixels(img: &Image, out: &mut Vec<u8>) -> Result<(), Error> {
    let (w, h) = (img.w, img.h);
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || img.rgba.len() < w * h * 4 {
        return Err(Error::PictureTooBig);
    }
    let px = |x: usize, y: usize| -> [u8; 4] {
        let i = (y * w + x) * 4;
        let p = [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2], img.rgba[i + 3]];
        if p[3] == 0 {
            [0; 4]
        } else {
            p
        }
    };
    // painted box
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if px(x, y)[3] > 0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    out.extend_from_slice(&[w as u8, h as u8]);
    if x0 >= x1 {
        out.extend_from_slice(&[0, 0, 0, 0]);
        return Ok(());
    }
    let (bw, bh) = (x1 - x0, y1 - y0);
    let at = |x: usize, y: usize| px(x0 + x, y0 + y);
    let mx = (0..bh).all(|y| (0..bw / 2).all(|x| at(x, y) == at(bw - 1 - x, y)));
    let my = (0..bh / 2).all(|y| (0..bw).all(|x| at(x, y) == at(x, bh - 1 - y)));
    let (sw, sh) = (if mx { (bw + 1) / 2 } else { bw }, if my { (bh + 1) / 2 } else { bh });
    let mut pal: Vec<[u8; 4]> = Vec::new();
    let mut idx = Vec::with_capacity(sw * sh);
    for y in 0..sh {
        for x in 0..sw {
            let p = at(x, y);
            if p[3] == 0 {
                idx.push(0u8);
                continue;
            }
            let i = match pal.iter().position(|q| *q == p) {
                Some(i) => i,
                None => {
                    if pal.len() == MAX_COLOURS {
                        return Err(Error::PictureTooBig);
                    }
                    pal.push(p);
                    pal.len() - 1
                }
            };
            idx.push(i as u8 + 1);
        }
    }
    let wide = pal.len() > 15;
    out.extend_from_slice(&[x0 as u8, y0 as u8, bw as u8, bh as u8]);
    out.push(mx as u8 | (my as u8) << 1 | (wide as u8) << 2);
    out.push(pal.len() as u8);
    for p in &pal {
        out.extend_from_slice(p);
    }
    let mut i = 0;
    while i < idx.len() {
        let c = idx[i];
        let mut n = 1;
        while i + n < idx.len() && idx[i + n] == c {
            n += 1;
        }
        if wide {
            out.push(c);
            put_varint(out, n - 1);
        } else if n < 16 {
            out.push(c << 4 | (n - 1) as u8);
        } else {
            out.push(c << 4 | 15);
            put_varint(out, n - 16);
        }
        i += n;
    }
    Ok(())
}

fn put_varint(out: &mut Vec<u8>, mut v: usize) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7F) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

// ------------------------------------------------------------------ decode

/// Finds the first "AIM1:" code in any text (a whole Discord message is fine) and reads it.
pub fn decode(text: &str) -> Result<Shared, Error> {
    let body = find(text).ok_or(Error::NoCode)?;
    if body.len() > MAX_CODE {
        return Err(Error::TooLong);
    }
    let bytes = b64_decode(body).ok_or(Error::Damaged)?;
    if bytes.len() < 4 {
        return Err(Error::Damaged);
    }
    let (data, sum) = bytes.split_at(bytes.len() - 2);
    if fletcher16(data) != u16::from_le_bytes([sum[0], sum[1]]) {
        return Err(Error::Damaged);
    }
    let mut r = Reader { b: data, at: 0 };
    let head = r.u8()?;
    if head & !HEAD_PIXELS != 0 {
        return Err(Error::Newer);
    }
    let mut p = base_profile();
    let mut name = String::new();
    let mut anim = None;
    loop {
        match r.u8()? {
            ID_END => break,
            ID_NAME => {
                let n = r.u8()? as usize;
                if n > MAX_NAME {
                    return Err(Error::Damaged);
                }
                name = clean_name(std::str::from_utf8(r.take(n)?).map_err(|_| Error::Damaged)?);
            }
            ID_ANIM => {
                let (f, fps) = (r.u8()? as u32, r.u8()? as u32);
                if !(2..=MAX_FRAMES).contains(&f) || !(1..=60).contains(&fps) || head & HEAD_PIXELS == 0 {
                    return Err(Error::Damaged);
                }
                anim = Some((f, fps));
            }
            id => take_field(&mut p, id, &mut r)?,
        }
    }
    let pixels = if head & HEAD_PIXELS != 0 {
        let f = anim.map_or(1, |a: (u32, u32)| a.0 as usize);
        let frames: Vec<Image> = (0..f).map(|_| take_pixels(&mut r)).collect::<Result<_, _>>()?;
        if frames.iter().any(|x| (x.w, x.h) != (frames[0].w, frames[0].h)) {
            return Err(Error::Damaged);
        }
        Some(join(&frames))
    } else {
        None
    };
    if r.at != data.len() {
        return Err(Error::Damaged);
    }
    p.mode = pixels.is_some() as u32;
    p.image.clear();
    p.sanitize();
    Ok(Shared { name, profile: p, pixels, anim })
}

/// Frames side by side.
fn join(frames: &[Image]) -> Image {
    if frames.len() == 1 {
        return frames[0].clone();
    }
    let (fw, h) = (frames[0].w, frames[0].h);
    let w = fw * frames.len();
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for f in frames {
            rgba.extend_from_slice(&f.rgba[y * fw * 4..(y + 1) * fw * 4]);
        }
    }
    Image { w, h, rgba }
}

/// The base64 part of the first code in `text` ("AIM1:" in any letter case, then base64url letters; spaces and
/// line breaks inside the code are skipped only when they are followed by more code, e.g. a wrapped line).
pub fn find(text: &str) -> Option<&str> {
    let low = text.to_ascii_lowercase();
    let start = low.find("aim1:")? + PREFIX.len();
    let rest = &text[start..];
    let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')).unwrap_or(rest.len());
    let s = &rest[..end];
    (!s.is_empty()).then_some(s)
}

/// True when the text holds something that looks like an Aimdot code (cheap; for "paste" buttons).
pub fn looks_like_code(text: &str) -> bool {
    find(text).is_some()
}

fn take_pixels(r: &mut Reader) -> Result<Image, Error> {
    let (w, h) = (r.u8()? as usize, r.u8()? as usize);
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return Err(Error::Damaged);
    }
    let (x0, y0, bw, bh) = (r.u8()? as usize, r.u8()? as usize, r.u8()? as usize, r.u8()? as usize);
    let mut rgba = vec![0u8; w * h * 4];
    if bw == 0 || bh == 0 {
        return if bw == 0 && bh == 0 && x0 == 0 && y0 == 0 { Ok(Image { w, h, rgba }) } else { Err(Error::Damaged) };
    }
    if x0 + bw > w || y0 + bh > h {
        return Err(Error::Damaged);
    }
    let flags = r.u8()?;
    if flags & !7 != 0 {
        return Err(Error::Newer);
    }
    let (mx, my, wide) = (flags & 1 != 0, flags & 2 != 0, flags & 4 != 0);
    let n = r.u8()? as usize;
    if n == 0 || (!wide && n > 15) {
        return Err(Error::Damaged);
    }
    let mut pal = vec![[0u8; 4]];
    for _ in 0..n {
        let c = r.take(4)?;
        if c[3] == 0 {
            return Err(Error::Damaged);
        }
        pal.push([c[0], c[1], c[2], c[3]]);
    }
    let (sw, sh) = (if mx { (bw + 1) / 2 } else { bw }, if my { (bh + 1) / 2 } else { bh });
    let total = sw * sh;
    let mut idx = Vec::with_capacity(total);
    while idx.len() < total {
        let (c, len) = if wide {
            let c = r.u8()?;
            (c, r.varint()? + 1)
        } else {
            let b = r.u8()?;
            let k = (b & 15) as usize;
            (b >> 4, if k < 15 { k + 1 } else { 16 + r.varint()? })
        };
        if c as usize > n || idx.len() + len > total {
            return Err(Error::Damaged);
        }
        idx.extend(std::iter::repeat(c).take(len));
    }
    for y in 0..bh {
        let sy = if my && y >= sh { bh - 1 - y } else { y };
        for x in 0..bw {
            let sx = if mx && x >= sw { bw - 1 - x } else { x };
            let p = pal[idx[sy * sw + sx] as usize];
            let i = ((y0 + y) * w + x0 + x) * 4;
            rgba[i..i + 4].copy_from_slice(&p);
        }
    }
    Ok(Image { w, h, rgba })
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let s = self.b.get(self.at..self.at + n).ok_or(Error::Damaged)?;
        self.at += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, Error> {
        let s = self.take(2)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }
    /// At most 3 bytes (< 2 million): more than any picture needs.
    fn varint(&mut self) -> Result<usize, Error> {
        let mut v = 0usize;
        for i in 0..3 {
            let b = self.u8()?;
            v |= ((b & 0x7F) as usize) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(Error::Damaged)
    }
}

// ------------------------------------------------------------------ small helpers

fn fletcher16(d: &[u8]) -> u16 {
    let (mut a, mut b) = (0u16, 0u16);
    for &x in d {
        a = (a + x as u16) % 255;
        b = (b + a) % 255;
    }
    b << 8 | a
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn b64_encode(d: &[u8], out: &mut String) {
    for ch in d.chunks(3) {
        let v = (ch[0] as u32) << 16 | (*ch.get(1).unwrap_or(&0) as u32) << 8 | *ch.get(2).unwrap_or(&0) as u32;
        for i in 0..ch.len() + 1 {
            out.push(B64[(v >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| B64.iter().position(|&b| b == c).map(|i| i as u32);
    let s = s.as_bytes();
    if s.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for ch in s.chunks(4) {
        let mut v = 0u32;
        for (i, &c) in ch.iter().enumerate() {
            v |= val(c)? << (18 - 6 * i);
        }
        let n = ch.len() - 1;
        // Leftover bits must be zero (one exact text per code).
        if v & ((1 << (8 * (3 - n))) - 1) != 0 && n < 3 {
            return None;
        }
        for i in 0..n {
            out.push((v >> (16 - 8 * i)) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crosshair::presets;

    fn painted(n: usize, f: impl Fn(usize, usize) -> [u8; 4]) -> Image {
        let mut rgba = Vec::with_capacity(n * n * 4);
        for y in 0..n {
            for x in 0..n {
                rgba.extend_from_slice(&f(x, y));
            }
        }
        Image { w: n, h: n, rgba }
    }

    #[test]
    fn every_preset_round_trips_and_is_short() {
        for (name, p) in presets() {
            let code = encode_profile(name, &p).unwrap();
            assert!(code.starts_with(PREFIX));
            assert!(code.len() < 60, "{name}: {code} ({} chars)", code.len());
            let s = decode(&code).unwrap();
            assert_eq!(s.name, name);
            assert_eq!(s.profile, p, "{name}");
            assert!(s.pixels.is_none());
        }
        // the default crosshair is just a few characters
        let c = encode_profile("", &Profile::default()).unwrap();
        assert!(c.len() <= 12, "{c}");
    }

    #[test]
    fn every_setting_round_trips() {
        let p = Profile {
            color: Color::hex(0x123456),
            opacity: 0.37,
            rainbow: true,
            rainbow_mode: 2,
            rainbow_speed: 3.4,
            rainbow_sat: 0.55,
            outline: false,
            outline_thick: 4.0,
            outline_color: Color::hex(0xFEDCBA),
            outline_opacity: 0.2,
            dot: true,
            dot_size: 40.0,
            dot_round: true,
            dot_opacity: 0.0,
            inner: false,
            inner_len: 60.0,
            inner_thick: 20.0,
            inner_gap: 0.0,
            inner_opacity: 0.99,
            outer: true,
            outer_len: 1.0,
            outer_thick: 7.0,
            outer_gap: 100.0,
            outer_opacity: 1.0,
            circle: true,
            circle_radius: 100.0,
            circle_thick: 1.0,
            circle_opacity: 0.5,
            t_style: true,
            rotation: 359.0,
            scale: 3.55,
            ..Profile::default()
        };
        let s = decode(&encode_profile("Mine", &p).unwrap()).unwrap();
        assert_eq!(s.profile, p);
        // half steps survive, finer ones are rounded to the UI's step
        let q = Profile { inner_len: 6.5, opacity: 0.333, ..Profile::default() };
        let s = decode(&encode_profile("", &q).unwrap()).unwrap();
        assert_eq!(s.profile.inner_len, 6.5);
        assert_eq!(s.profile.opacity, 0.33);
    }

    #[test]
    fn names_are_cleaned_and_limited() {
        let code = encode_profile("  [Pro]|x=1\nline  ", &Profile::default()).unwrap();
        assert_eq!(decode(&code).unwrap().name, "Prox1line");
        let long = "é".repeat(100);
        let n = decode(&encode_profile(&long, &Profile::default()).unwrap()).unwrap().name;
        assert!(n.len() <= MAX_NAME && n.chars().all(|c| c == 'é'));
    }

    #[test]
    fn painted_crosshair_round_trips_small() {
        // a 31 px plus with a centre dot in two colours + a half-transparent ring pixel set: 4-way symmetric
        let img = painted(31, |x, y| {
            let (dx, dy) = ((x as i32 - 15).abs(), (y as i32 - 15).abs());
            if dx == 0 && dy == 0 {
                [255, 0, 0, 255]
            } else if (dx == 0 && (3..9).contains(&dy)) || (dy == 0 && (3..9).contains(&dx)) {
                [0, 255, 0, 255]
            } else if dx == 12 && dy == 12 {
                [255, 255, 255, 128]
            } else {
                [9, 9, 9, 0] // transparent with junk colour: counts as transparent
            }
        });
        let code = encode_pixels("Painted", &img).unwrap();
        assert!(code.len() < 120, "{} chars: {code}", code.len());
        let s = decode(&code).unwrap();
        assert_eq!(s.name, "Painted");
        assert_eq!(s.profile.mode, 1);
        assert!(s.profile.image.is_empty());
        let back = s.pixels.unwrap();
        assert_eq!((back.w, back.h), (31, 31));
        for (i, (a, b)) in img.rgba.chunks(4).zip(back.rgba.chunks(4)).enumerate() {
            if a[3] == 0 {
                assert_eq!(b, [0, 0, 0, 0], "pixel {i}");
            } else {
                assert_eq!(a, b, "pixel {i}");
            }
        }
    }

    #[test]
    fn unsymmetric_many_colour_and_empty_pictures_round_trip() {
        // 63 px, no symmetry, 200 colours: the wide run form
        let img = painted(63, |x, y| if (x * 7 + y * 3) % 5 == 0 { [(x % 10 * 20) as u8, (y % 20 * 10) as u8, 7, 255] } else { [0; 4] });
        let s = decode(&encode_pixels("", &img).unwrap()).unwrap();
        let back = s.pixels.unwrap();
        for (a, b) in img.rgba.chunks(4).zip(back.rgba.chunks(4)) {
            assert_eq!(if a[3] == 0 { [0u8; 4].as_slice() } else { a }, b);
        }
        // only mirror left/right, odd and even widths
        for n in [9usize, 10] {
            let img = painted(n, |x, y| if y < 3 && (x == 1 || x == n - 2) { [1, 2, 3, 255] } else { [0; 4] });
            assert_eq!(decode(&encode_pixels("", &img).unwrap()).unwrap().pixels.unwrap(), img);
        }
        let empty = painted(9, |_, _| [0; 4]);
        assert_eq!(decode(&encode_pixels("", &empty).unwrap()).unwrap().pixels.unwrap(), empty);
        // too big / too many colours
        let big = Image { w: MAX_SIDE + 1, h: 1, rgba: vec![255; (MAX_SIDE + 1) * 4] };
        assert_eq!(encode_pixels("", &big), Err(Error::PictureTooBig));
        let rainbow = painted(20, |x, y| [x as u8, y as u8, 0, 255]); // 400 colours
        assert_eq!(encode_pixels("", &rainbow), Err(Error::PictureTooBig));
        // an image crosshair needs its pixels
        assert_eq!(encode_profile("", &Profile { mode: 1, ..Profile::default() }), Err(Error::NoPicture));
    }

    #[test]
    fn image_settings_travel_with_the_picture() {
        let img = painted(9, |x, y| if x == 4 || y == 4 { [255, 255, 0, 255] } else { [0; 4] });
        let p = Profile { mode: 1, image: "Mine.png".into(), img_scale: 2.5, img_opacity: 0.8, img_tint: true, rainbow: true, img_smooth: false, ..Profile::default() };
        let s = decode(&encode("x", &p, Some(&img)).unwrap()).unwrap();
        let mut want = p.clone();
        want.image.clear();
        assert_eq!(s.profile, want);
    }

    #[test]
    fn animated_strip_round_trips() {
        // 8 frames of 63 px (504 px wide - more than one block could hold): a dot that moves out
        let (fw, f) = (63usize, 8usize);
        let strip = Image {
            w: fw * f,
            h: fw,
            rgba: (0..fw)
                .flat_map(|y| (0..fw * f).map(move |x| (x, y)))
                .flat_map(|(x, y)| {
                    let (i, lx) = (x / fw, x % fw);
                    if y == 31 && (lx == 31 + i || lx == 31 - i) { [255u8, 200, 0, 255] } else { [0u8; 4] }
                })
                .collect(),
        };
        let p = Profile { mode: 1, img_smooth: false, ..Profile::default() };
        let code = encode_anim("Pulse", &p, Some(&strip), Some((8, 12))).unwrap();
        assert!(code.len() < 200, "{} chars", code.len());
        let s = decode(&code).unwrap();
        assert_eq!(s.anim, Some((8, 12)));
        assert_eq!(s.pixels.unwrap(), strip);
        // a still picture has no anim; a strip that doesn't divide is refused; bad fps refused
        assert_eq!(decode(&encode_pixels("", &frame(&strip, 0, fw)).unwrap()).unwrap().anim, None);
        assert_eq!(encode_anim("", &p, Some(&strip), Some((5, 12))), Err(Error::PictureTooBig));
        assert_eq!(encode_anim("", &p, Some(&strip), Some((8, 0))), Err(Error::PictureTooBig));
        // 1 frame = a still picture, the same code as encode()
        let one = frame(&strip, 0, fw);
        assert_eq!(encode_anim("x", &p, Some(&one), Some((1, 8))).unwrap(), encode("x", &p, Some(&one)).unwrap());
    }

    #[test]
    fn found_inside_a_message_any_case() {
        let code = encode_profile("Dot", &presets()[7].1).unwrap();
        let msg = format!("try my crosshair: {code}  (made in Aimdot)");
        assert_eq!(decode(&msg).unwrap().name, "Dot");
        let low = format!("aim1:{}", &code[5..]);
        assert_eq!(decode(&low).unwrap().name, "Dot");
        assert!(looks_like_code(&msg));
        assert!(!looks_like_code("hello"));
    }

    #[test]
    fn junk_is_refused_safely() {
        assert_eq!(decode("hello"), Err(Error::NoCode));
        assert_eq!(decode("AIM1:"), Err(Error::NoCode));
        assert_eq!(decode("AIM1:A"), Err(Error::Damaged));
        assert_eq!(decode("AIM1:AAAA"), Err(Error::Damaged));
        assert_eq!(decode(&format!("AIM1:{}", "A".repeat(MAX_CODE + 1))), Err(Error::TooLong));
        let code = encode_pixels("x", &painted(31, |x, y| if x == 15 || y == 15 { [255, 0, 0, 255] } else { [0; 4] })).unwrap();
        // every single-letter typo and every cut is refused (or, very rarely, still a valid crosshair), never a panic
        let body = &code[5..];
        let mut refused = 0;
        for i in 0..body.len() {
            let mut b = body.as_bytes().to_vec();
            b[i] = if b[i] == b'A' { b'B' } else { b'A' };
            refused += decode(&format!("AIM1:{}", String::from_utf8(b).unwrap())).is_err() as usize;
            let _ = decode(&format!("AIM1:{}", &body[..i]));
        }
        assert!(refused >= body.len() - 1, "{refused} of {}", body.len());
        // a valid checksum around junk bytes: still no panic, and never a picture bigger than the limits
        let mut seed = 0x1234_5678u32;
        for len in 1..300 {
            let mut d: Vec<u8> = (0..len)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    seed as u8
                })
                .collect();
            d[0] &= 1;
            let sum = fletcher16(&d);
            d.extend_from_slice(&sum.to_le_bytes());
            let mut s = String::from(PREFIX);
            b64_encode(&d, &mut s);
            if let Ok(sh) = decode(&s) {
                if let Some(img) = sh.pixels {
                    assert!(img.w <= MAX_SIDE && img.h <= MAX_SIDE);
                }
            }
        }
        // a code with unknown head bits = newer Aimdot
        let mut d = vec![0x80u8, 0];
        let sum = fletcher16(&d);
        d.extend_from_slice(&sum.to_le_bytes());
        let mut s = String::from(PREFIX);
        b64_encode(&d, &mut s);
        assert_eq!(decode(&s), Err(Error::Newer));
    }

    #[test]
    fn base64_round_trip() {
        for len in 0..20 {
            let d: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            let mut s = String::new();
            b64_encode(&d, &mut s);
            assert_eq!(b64_decode(&s).unwrap(), d);
        }
    }
}
