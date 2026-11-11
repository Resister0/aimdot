//! Software rasteriser: a premultiplied-ARGB canvas with anti-aliased primitives.
//! Pixel layout is 0xAARRGGBB, which is exactly what a 32-bit Windows DIB expects.

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const WHITE: Color = Color::hex(0xFFFFFF);
    pub const BLACK: Color = Color::hex(0x000000);

    pub const fn hex(v: u32) -> Self {
        Color { r: (v >> 16) as u8, g: (v >> 8) as u8, b: v as u8, a: 255 }
    }
    pub fn alpha(self, a: f32) -> Self {
        Color { a: (self.a as f32 * a.clamp(0.0, 1.0) + 0.5) as u8, ..self }
    }
    pub fn mix(self, o: Color, t: f32) -> Color {
        let t = t.clamp(0.0, 1.0);
        let l = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t + 0.5) as u8;
        Color { r: l(self.r, o.r), g: l(self.g, o.g), b: l(self.b, o.b), a: l(self.a, o.a) }
    }
    pub fn luma(self) -> f32 {
        (0.299 * self.r as f32 + 0.587 * self.g as f32 + 0.114 * self.b as f32) / 255.0
    }
    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
    /// Accepts `#RGB`, `#RRGGBB` and `#RRGGBBAA` (leading `#` optional).
    pub fn parse(s: &str) -> Option<Color> {
        let s = s.trim().trim_start_matches('#');
        if !s.is_ascii() {
            return None;
        }
        let h = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
        match s.len() {
            3 => {
                let d = |i: usize| u8::from_str_radix(&s[i..i + 1], 16).ok().map(|v| v * 17);
                Some(Color { r: d(0)?, g: d(1)?, b: d(2)?, a: 255 })
            }
            6 => Some(Color { r: h(0)?, g: h(2)?, b: h(4)?, a: 255 }),
            8 => Some(Color { r: h(0)?, g: h(2)?, b: h(4)?, a: h(6)? }),
            _ => None,
        }
    }
    /// h, s, v all in 0..=1.
    pub fn from_hsv(h: f32, s: f32, v: f32) -> Color {
        let h = (h - h.floor()) * 6.0;
        let i = h.floor();
        let f = h - i;
        let p = v * (1.0 - s);
        let q = v * (1.0 - s * f);
        let t = v * (1.0 - s * (1.0 - f));
        let (r, g, b) = match i as i32 {
            0 => (v, t, p),
            1 => (q, v, p),
            2 => (p, v, t),
            3 => (p, q, v),
            4 => (t, p, v),
            _ => (v, p, q),
        };
        let c = |x: f32| (x.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        Color { r: c(r), g: c(g), b: c(b), a: 255 }
    }
    pub fn to_hsv(self) -> (f32, f32, f32) {
        let (r, g, b) = (self.r as f32 / 255.0, self.g as f32 / 255.0, self.b as f32 / 255.0);
        let mx = r.max(g).max(b);
        let mn = r.min(g).min(b);
        let d = mx - mn;
        let h = if d <= 0.0 {
            0.0
        } else if mx == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if mx == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        } / 6.0;
        (h, if mx <= 0.0 { 0.0 } else { d / mx }, mx)
    }
}

#[inline(always)]
fn div255(x: u32) -> u32 {
    let y = x + 128;
    (y + (y >> 8)) >> 8
}

#[inline(always)]
pub fn pack(c: Color) -> u32 {
    let a = c.a as u32;
    (a << 24) | (div255(c.r as u32 * a) << 16) | (div255(c.g as u32 * a) << 8) | div255(c.b as u32 * a)
}

/// Blend straight-alpha colour `c` at coverage `cov` (0..=255) over premultiplied `d`.
#[inline(always)]
fn blend(d: &mut u32, c: Color, cov: u32) {
    let a = div255(c.a as u32 * cov);
    if a == 0 {
        return;
    }
    let ia = 255 - a;
    let dv = *d;
    let r = div255(c.r as u32 * a) + div255(((dv >> 16) & 255) * ia);
    let g = div255(c.g as u32 * a) + div255(((dv >> 8) & 255) * ia);
    let b = div255(c.b as u32 * a) + div255((dv & 255) * ia);
    let oa = a + div255((dv >> 24) * ia);
    *d = (oa << 24) | (r << 16) | (g << 8) | b;
}

/// Premultiplied source-over.
#[inline(always)]
fn over(d: &mut u32, s: u32) {
    let a = s >> 24;
    if a == 255 {
        *d = s;
    } else if a != 0 {
        let ia = 255 - a;
        let dv = *d;
        let ch = |sh: u32| (((s >> sh) & 255) + div255(((dv >> sh) & 255) * ia)).min(255) << sh;
        *d = ch(24) | ch(16) | ch(8) | ch(0);
    }
}

pub struct Canvas {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u32>,
    cx0: i32,
    cy0: i32,
    cx1: i32,
    cy1: i32,
}

impl Canvas {
    pub fn new(w: i32, h: i32) -> Self {
        let (w, h) = (w.max(0), h.max(0));
        Canvas { w, h, px: vec![0; (w * h) as usize], cx0: 0, cy0: 0, cx1: w, cy1: h }
    }
    /// Resizes (contents become undefined-but-valid) and resets the clip.
    pub fn resize(&mut self, w: i32, h: i32) {
        let (w, h) = (w.max(0), h.max(0));
        if w != self.w || h != self.h {
            self.w = w;
            self.h = h;
            self.px.resize((w * h) as usize, 0);
        }
        self.reset_clip();
    }
    pub fn clear(&mut self, c: Color) {
        self.px.fill(pack(c));
    }
    pub fn set_clip(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        self.cx0 = x0.clamp(0, self.w);
        self.cy0 = y0.clamp(0, self.h);
        self.cx1 = x1.clamp(self.cx0, self.w);
        self.cy1 = y1.clamp(self.cy0, self.h);
    }
    pub fn reset_clip(&mut self) {
        self.cx0 = 0;
        self.cy0 = 0;
        self.cx1 = self.w;
        self.cy1 = self.h;
    }
    #[inline]
    fn span(&self, x: f32, y: f32, w: f32, h: f32) -> (i32, i32, i32, i32) {
        (
            (x.floor() as i32).max(self.cx0),
            (y.floor() as i32).max(self.cy0),
            ((x + w).ceil() as i32).min(self.cx1),
            ((y + h).ceil() as i32).min(self.cy1),
        )
    }
    /// Fills pixels x0..x1 of a row with uniform coverage `cov` (0..=255).
    #[inline]
    fn fill_span(&mut self, row: usize, x0: i32, x1: i32, c: Color, cov: u32) {
        if x1 <= x0 || cov == 0 {
            return;
        }
        let s = &mut self.px[row + x0 as usize..row + x1 as usize];
        if c.a == 255 && cov == 255 {
            s.fill(pack(c));
        } else {
            for d in s {
                blend(d, c, cov);
            }
        }
    }

    /// Anti-aliased filled rounded rectangle.
    pub fn rrect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, c: Color) {
        if w <= 0.0 || h <= 0.0 || c.a == 0 {
            return;
        }
        let r = r.max(0.0).min(w * 0.5).min(h * 0.5);
        let (x0, y0, x1, y1) = self.span(x, y, w, h);
        let (hw, hh) = (w * 0.5, h * 0.5);
        let (cx, cy) = (x + hw, y + hh);
        let ix0 = ((x + r).ceil() as i32).max(x0);
        let ix1 = ((x + w - r).floor() as i32).min(x1);
        for py in y0..y1 {
            let qy = (py as f32 + 0.5 - cy).abs() - (hh - r);
            let row = (py * self.w) as usize;
            // Between the corners the distance depends only on y: one coverage value per row
            // (fully covered for interior rows, fractional on anti-aliased top/bottom edges).
            let mid = ((0.5 - (qy - r)).clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
            let mut p = x0;
            while p < x1 {
                if p >= ix0 && p < ix1 {
                    self.fill_span(row, p, ix1, c, mid);
                    p = ix1;
                    continue;
                }
                let qx = (p as f32 + 0.5 - cx).abs() - (hw - r);
                let d = if qx > 0.0 && qy > 0.0 { (qx * qx + qy * qy).sqrt() - r } else { qx.max(qy) - r };
                let cov = (0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    blend(&mut self.px[row + p as usize], c, (cov * 255.0 + 0.5) as u32);
                }
                p += 1;
            }
        }
    }

    /// Anti-aliased rounded-rectangle outline of thickness `t`, drawn inside the rect.
    pub fn rrect_stroke(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, t: f32, c: Color) {
        if w <= 0.0 || h <= 0.0 || c.a == 0 || t <= 0.0 {
            return;
        }
        let r = r.max(0.0).min(w * 0.5).min(h * 0.5);
        let (x0, y0, x1, y1) = self.span(x, y, w, h);
        let (hw, hh) = (w * 0.5, h * 0.5);
        let (cx, cy) = (x + hw, y + hh);
        let m = r.max(t + 0.5);
        let sx0 = (cx - (hw - m)).ceil() as i32;
        let sx1 = (cx + (hw - m)).floor() as i32;
        for py in y0..y1 {
            let fy = (py as f32 + 0.5 - cy).abs();
            let qy = fy - (hh - r);
            let skip = fy <= hh - m && sx1 > sx0;
            let row = (py * self.w) as usize;
            // Top/bottom edges: uniform coverage across the straight part of the row.
            let lim = hw - r + qy.min(0.0);
            let ix0 = ((cx - lim - 0.5).ceil() as i32).max(x0);
            let ix1 = ((cx + lim - 0.5).floor() as i32 + 1).min(x1);
            let dm = qy - r;
            let mid = (((0.5 - dm).clamp(0.0, 1.0) - (0.5 - (dm + t)).clamp(0.0, 1.0)) * 255.0 + 0.5) as u32;
            let mut p = x0;
            while p < x1 {
                if skip && p >= sx0 && p < sx1 {
                    p = sx1;
                    continue;
                }
                if !skip && p >= ix0 && p < ix1 {
                    self.fill_span(row, p, ix1, c, mid);
                    p = ix1;
                    continue;
                }
                let qx = (p as f32 + 0.5 - cx).abs() - (hw - r);
                let d = if qx > 0.0 && qy > 0.0 { (qx * qx + qy * qy).sqrt() - r } else { qx.max(qy) - r };
                let cov = (0.5 - d).clamp(0.0, 1.0) - (0.5 - (d + t)).clamp(0.0, 1.0);
                if cov > 0.0 {
                    blend(&mut self.px[row + p as usize], c, (cov * 255.0 + 0.5) as u32);
                }
                p += 1;
            }
        }
    }

    /// Anti-aliased line with round caps.
    pub fn line(&mut self, ax: f32, ay: f32, bx: f32, by: f32, wid: f32, c: Color) {
        let hw = wid * 0.5;
        let (x0, y0, x1, y1) = self.span(
            ax.min(bx) - hw - 1.0,
            ay.min(by) - hw - 1.0,
            (ax - bx).abs() + wid + 2.0,
            (ay - by).abs() + wid + 2.0,
        );
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = (dx * dx + dy * dy).max(1e-6);
        for py in y0..y1 {
            let row = (py * self.w) as usize;
            for p in x0..x1 {
                let (fx, fy) = (p as f32 + 0.5 - ax, py as f32 + 0.5 - ay);
                let t = ((fx * dx + fy * dy) / len2).clamp(0.0, 1.0);
                let (ex, ey) = (fx - dx * t, fy - dy * t);
                let cov = (hw + 0.5 - (ex * ex + ey * ey).sqrt()).clamp(0.0, 1.0);
                if cov > 0.0 {
                    blend(&mut self.px[row + p as usize], c, (cov * 255.0 + 0.5) as u32);
                }
            }
        }
    }

    /// Tinted 8-bit alpha mask (glyphs).
    pub fn mask(&mut self, x: i32, y: i32, w: i32, h: i32, a: &[u8], c: Color) {
        let (x0, y0) = (x.max(self.cx0), y.max(self.cy0));
        let (x1, y1) = ((x + w).min(self.cx1), (y + h).min(self.cy1));
        for py in y0..y1 {
            let row = (py * self.w) as usize;
            let srow = ((py - y) * w - x) as isize;
            for p in x0..x1 {
                let cov = a[(srow + p as isize) as usize] as u32;
                if cov > 0 {
                    blend(&mut self.px[row + p as usize], c, cov);
                }
            }
        }
    }

    /// Source-over blit of a premultiplied canvas with integer nearest-neighbour zoom.
    pub fn blit(&mut self, x: i32, y: i32, src: &Canvas, zoom: i32) {
        let z = zoom.max(1);
        let (x0, y0) = (x.max(self.cx0), y.max(self.cy0));
        let (x1, y1) = ((x + src.w * z).min(self.cx1), (y + src.h * z).min(self.cy1));
        for py in y0..y1 {
            let row = (py * self.w) as usize;
            let srow = (((py - y) / z) * src.w) as usize;
            for p in x0..x1 {
                over(&mut self.px[row + p as usize], src.px[srow + ((p - x) / z) as usize]);
            }
        }
    }

    /// Per-pixel fill; `f` receives normalised (u, v) in 0..1 across the rect.
    pub fn fill_fn(&mut self, x: f32, y: f32, w: f32, h: f32, mut f: impl FnMut(f32, f32) -> Color) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let (x0, y0, x1, y1) = self.span(x.round(), y.round(), w.round(), h.round());
        for py in y0..y1 {
            let row = (py * self.w) as usize;
            let v = (py as f32 + 0.5 - y) / h;
            for p in x0..x1 {
                let u = (p as f32 + 0.5 - x) / w;
                blend(&mut self.px[row + p as usize], f(u, v), 255);
            }
        }
    }

    pub fn checker(&mut self, x: f32, y: f32, w: f32, h: f32, cell: i32, a: Color, b: Color) {
        let (x0, y0, x1, y1) = self.span(x.round(), y.round(), w.round(), h.round());
        let (ox, oy, cell) = (x.round() as i32, y.round() as i32, cell.max(1));
        let (pa, pb) = (pack(a), pack(b));
        for py in y0..y1 {
            let row = (py * self.w) as usize;
            for p in x0..x1 {
                let odd = (((p - ox) / cell) + ((py - oy) / cell)) & 1 == 1;
                self.px[row + p as usize] = if odd { pb } else { pa };
            }
        }
    }

    /// 24-bit BMP of the canvas composited over `bg` (used by the headless QC harness).
    #[allow(dead_code)]
    pub fn to_bmp(&self, bg: Color) -> Vec<u8> {
        let (w, h) = (self.w as usize, self.h as usize);
        let stride = (w * 3 + 3) & !3;
        let size = 54 + stride * h;
        let mut o = Vec::with_capacity(size);
        o.extend_from_slice(b"BM");
        o.extend_from_slice(&(size as u32).to_le_bytes());
        o.extend_from_slice(&[0; 4]);
        o.extend_from_slice(&54u32.to_le_bytes());
        o.extend_from_slice(&40u32.to_le_bytes());
        o.extend_from_slice(&(w as i32).to_le_bytes());
        o.extend_from_slice(&(h as i32).to_le_bytes());
        o.extend_from_slice(&1u16.to_le_bytes());
        o.extend_from_slice(&24u16.to_le_bytes());
        o.extend_from_slice(&[0; 24]);
        for y in (0..h).rev() {
            for x in 0..w {
                let mut d = pack(bg);
                over(&mut d, self.px[y * w + x]);
                o.extend_from_slice(&[d as u8, (d >> 8) as u8, (d >> 16) as u8]);
            }
            o.extend(std::iter::repeat(0).take(stride - w * 3));
        }
        o
    }
}
