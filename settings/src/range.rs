//! The preview's "test range": backgrounds drawn here (no bundled pictures - the app stays light), the crosshair in
//! the middle, an optional slow pan, and colour-blind views (a colour matrix over the whole preview).
//! Backgrounds (config `preview_bg`; the numbers stay, new ones are added at the end): 0 Dark, 1 Light, 2 Grey,
//! 3 Field (the old "Scene"), 4 Sky, 5 Grass, 6 Snow, 7 Night, 8 Neon city, 9 Black, 10 White, 11 Green screen,
//! 12 Cave, 13 Desert noon, 14 Sunset, 15 Leaves, 16 Gravel. All drawn in code: 0 KB of bundled pictures.
//! Your own pictures (config `preview_img`) live in `<config>/backgrounds/` (imported like the Images page does).
//! Everything that moves repeats every `w` pixels, so the pan loops without a seam.
use crosshair_core::gfx::{Canvas, Color};

pub const SIZE: i32 = 264; // the preview square, in pixels (= its size in the window)
pub const NAMES: [&str; 17] = [
    "Dark", "Light", "Grey", "Field", "Sky", "Grass", "Snow", "Night", "Neon", "Black", "White", "Green", "Cave", "Desert", "Sunset", "Leaves", "Gravel",
];
/// The order of the thumbnails: plain ones, dark, bright, colourful, busy
pub const ORDER: [u32; 17] = [9, 0, 2, 1, 10, 11, 7, 12, 6, 13, 4, 3, 5, 14, 8, 15, 16];

fn hash(i: u32) -> f32 {
    let mut x = i.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65535.0
}

fn vgrad(c: &mut Canvas, y: f32, h: f32, top: u32, bottom: u32) {
    let (a, b) = (Color::hex(top), Color::hex(bottom));
    let w = c.w as f32;
    c.fill_fn(0.0, y, w, h, |_, v| a.mix(b, v));
}

/// Draws `f(x_offset)` three times (one period left, here, right) so anything that pans wraps around.
fn tiled(c: &mut Canvas, pan: f32, mut f: impl FnMut(&mut Canvas, f32)) {
    let w = c.w as f32;
    let p = pan.rem_euclid(w);
    for k in -1..=1 {
        f(c, k as f32 * w - p);
    }
}

pub fn backdrop(kind: u32, w: i32, h: i32, pan: f32) -> Canvas {
    let mut c = Canvas::new(w, h);
    let (fw, fh) = (w as f32, h as f32);
    let s = fh / SIZE as f32; // shapes scale with the height (small thumbnails)
    match kind {
        0 => c.clear(Color::hex(0x1A1C22)),
        1 => c.clear(Color::hex(0xD8DBE2)),
        2 => c.clear(Color::hex(0x6A7078)),
        3 => {
            vgrad(&mut c, 0.0, fh * 0.55, 0x7FB2E5, 0xC9DCEF);
            vgrad(&mut c, fh * 0.55, fh * 0.45 + 1.0, 0x6D7A4D, 0x48532F);
        }
        4 => {
            vgrad(&mut c, 0.0, fh, 0x4F9DE8, 0xCFE5FA);
            tiled(&mut c, pan, |c, ox| {
                for i in 0..5u32 {
                    let (x, y) = (ox + hash(i) * fw, fh * (0.12 + 0.7 * hash(i + 50)));
                    let cw = (40.0 + 50.0 * hash(i + 100)) * s;
                    let white = Color::hex(0xFFFFFF).alpha(0.92);
                    c.rrect(x, y, cw, cw * 0.32, cw * 0.16, white);
                    c.rrect(x + cw * 0.18, y - cw * 0.16, cw * 0.42, cw * 0.36, cw * 0.18, white);
                    c.rrect(x + cw * 0.45, y - cw * 0.08, cw * 0.36, cw * 0.3, cw * 0.15, white);
                }
            });
        }
        5 => {
            let hz = fh * 0.4;
            vgrad(&mut c, 0.0, hz, 0x8CC4F0, 0xD6EBF8);
            vgrad(&mut c, hz, fh - hz + 1.0, 0x79B34A, 0x3E6B28);
            tiled(&mut c, pan, |c, ox| {
                for i in 0..160u32 {
                    let x = ox + hash(i) * fw;
                    let v = hash(i + 1000);
                    let y = hz + 4.0 * s + v * (fh - hz);
                    let len = (3.0 + 7.0 * v) * s;
                    let col = if i % 3 == 0 { Color::hex(0xA6D46A) } else { Color::hex(0x2F5A1E) };
                    c.rrect(x, y - len, (1.0 + v) * s.max(0.5), len, 0.5, col.alpha(0.8));
                }
            });
        }
        6 => {
            let hz = fh * 0.45;
            vgrad(&mut c, 0.0, hz, 0xB8C7D6, 0xE8EEF4);
            vgrad(&mut c, hz, fh - hz + 1.0, 0xF4F7FA, 0xDDE6EF);
            tiled(&mut c, pan, |c, ox| {
                for i in 0..4u32 {
                    let r = fw * (0.3 + 0.2 * hash(i + 7));
                    c.rrect(ox + hash(i + 3) * fw - r, hz - r * 0.28, r * 2.0, r, r * 0.5, Color::hex(0xEDF2F7));
                }
                for i in 0..90u32 {
                    let d = (1.2 + 1.8 * hash(i + 300)) * s.max(0.6);
                    c.rrect(ox + hash(i + 200) * fw, hash(i + 400) * fh, d, d, d / 2.0, Color::hex(0xFFFFFF).alpha(0.9));
                }
            });
        }
        7 => {
            vgrad(&mut c, 0.0, fh, 0x070B18, 0x1B2440);
            tiled(&mut c, pan, |c, ox| {
                for i in 0..80u32 {
                    let d = (0.8 + 1.4 * hash(i + 500)) * s.max(0.6);
                    let a = 0.35 + 0.65 * hash(i + 600);
                    c.rrect(ox + hash(i + 700) * fw, hash(i + 800) * fh * 0.8, d, d, d / 2.0, Color::hex(0xFFFFFF).alpha(a));
                }
                let m = 26.0 * s;
                c.rrect(ox + fw * 0.72, fh * 0.12, m, m, m / 2.0, Color::hex(0xF2EFE0));
                for i in 0..6u32 {
                    let r = fw * (0.25 + 0.15 * hash(i + 900));
                    c.rrect(ox + i as f32 * fw / 6.0 - r * 0.5, fh * (0.82 + 0.06 * hash(i + 950)), r * 2.0, fh, r * 0.6, Color::hex(0x0D1222));
                }
            });
        }
        8 => {
            // neon city at night: two rows of buildings, the near one with pink / cyan / yellow windows
            vgrad(&mut c, 0.0, fh, 0x0E0A1F, 0x47185E);
            tiled(&mut c, pan, |c, ox| {
                for (layer, col, top, n) in [(0u32, 0x2C2350, 0.35, 9u32), (1, 0x120F1F, 0.5, 7)] {
                    let mut x = 0.0;
                    let mut i = 0u32;
                    while x < fw && i < 40 {
                        let bw = fw / n as f32 * (0.7 + 0.6 * hash(layer * 100 + i));
                        let bh = fh * (1.0 - top) * (0.45 + 0.55 * hash(layer * 100 + i + 50));
                        let (bx, by) = (ox + x, fh - bh);
                        c.rrect(bx, by, bw - 2.0 * s, bh, 0.0, Color::hex(col));
                        if layer == 1 {
                            let (ww, wh, gap) = (3.0 * s.max(0.5), 4.0 * s.max(0.5), 7.0 * s.max(0.5));
                            let mut wy = by + gap;
                            let mut k = 0u32;
                            while wy < fh - gap {
                                let mut wx = bx + gap * 0.7;
                                while wx < bx + bw - gap {
                                    k += 1;
                                    let on = hash(i * 1000 + k);
                                    if on > 0.45 {
                                        let lit = [0xFF3FD0, 0x3FE8FF, 0xFFD27A][(k % 3) as usize];
                                        c.rrect(wx, wy, ww, wh, 0.0, Color::hex(lit).alpha(0.5 + 0.5 * on));
                                    }
                                    wx += gap;
                                }
                                wy += gap * 1.3;
                            }
                        }
                        x += bw;
                        i += 1;
                    }
                }
            });
        }
        9 => c.clear(Color::hex(0x000000)),
        10 => c.clear(Color::hex(0xFFFFFF)),
        11 => c.clear(Color::hex(0x00B140)),
        12 => {
            // cave: dark rock, a faint light from above, rocks and drips
            vgrad(&mut c, 0.0, fh, 0x2A241F, 0x0A0807);
            tiled(&mut c, pan, |c, ox| {
                for i in 0..70u32 {
                    let r = (6.0 + 22.0 * hash(i + 1100)) * s;
                    let sh = [0x3A322B, 0x1B1714, 0x4A4037, 0x14110F][(i % 4) as usize];
                    c.rrect(ox + hash(i + 1200) * fw, hash(i + 1300) * fh, r * 1.6, r, r * 0.5, Color::hex(sh).alpha(0.85));
                }
                for i in 0..14u32 {
                    let l = (12.0 + 50.0 * hash(i + 1400)) * s;
                    c.rrect(ox + hash(i + 1500) * fw, -2.0, (3.0 + 4.0 * hash(i + 1600)) * s, l, 2.0 * s, Color::hex(0x4A4037));
                }
            });
        }
        13 => {
            // desert at noon: pale hot sky, bright sand dunes, a white sun
            let hz = fh * 0.5;
            vgrad(&mut c, 0.0, hz, 0x8EC9F2, 0xF4EBD6);
            vgrad(&mut c, hz, fh - hz + 1.0, 0xF3DDA6, 0xD9B26A);
            tiled(&mut c, pan, |c, ox| {
                let sun = 30.0 * s;
                c.rrect(ox + fw * 0.2, fh * 0.1, sun, sun, sun / 2.0, Color::hex(0xFFFDF2));
                for i in 0..5u32 {
                    let r = fw * (0.35 + 0.25 * hash(i + 1700));
                    let col = if i % 2 == 0 { 0xE9CC8C } else { 0xD6AC64 };
                    c.rrect(ox + hash(i + 1800) * fw - r, hz + fh * 0.12 * hash(i + 1900) - r * 0.1, r * 2.0, r * 0.6, r * 0.3, Color::hex(col));
                }
            });
        }
        14 => {
            // sunset over water: purple to orange sky, a low sun, warm reflections
            let hz = fh * 0.66;
            vgrad(&mut c, 0.0, hz, 0x2B1B4A, 0xFF7A45);
            vgrad(&mut c, hz, fh - hz + 1.0, 0x4A2C55, 0x140F26);
            tiled(&mut c, pan, |c, ox| {
                let sun = 58.0 * s;
                c.rrect(ox + fw * 0.55, hz - sun * 0.55, sun, sun, sun / 2.0, Color::hex(0xFFD07A));
                for i in 0..16u32 {
                    let y = hz + 4.0 * s + i as f32 * 5.0 * s;
                    let w = (30.0 + 40.0 * hash(i + 2000)) * s;
                    c.rrect(ox + fw * 0.55 + sun / 2.0 - w / 2.0 + (hash(i + 2100) - 0.5) * 20.0 * s, y, w, 1.5 * s.max(0.6), 1.0, Color::hex(0xFFB060).alpha(0.7 - i as f32 * 0.035));
                }
            });
        }
        15 => {
            // leaves: busy green noise in many shades
            c.clear(Color::hex(0x1F3D1A));
            tiled(&mut c, pan, |c, ox| {
                for i in 0..320u32 {
                    let (lw, lh) = ((10.0 + 14.0 * hash(i + 2200)) * s, (6.0 + 8.0 * hash(i + 2300)) * s);
                    let col = [0x2E6B26, 0x4C8C2F, 0x7DB342, 0x16300F, 0xA5C94E, 0x3B7A2A][(i % 6) as usize];
                    c.rrect(ox + hash(i + 2400) * fw, hash(i + 2500) * fh - lh / 2.0, lw, lh, lh / 2.0, Color::hex(col).alpha(0.9));
                }
            });
        }
        16 => {
            // gravel: small grey stones, each with a little shadow
            c.clear(Color::hex(0x6F6961));
            tiled(&mut c, pan, |c, ox| {
                for i in 0..620u32 {
                    let d = (3.0 + 7.0 * hash(i + 2600)) * s.max(0.5);
                    let (x, y) = (ox + hash(i + 2700) * fw, hash(i + 2800) * fh);
                    let g = 0x55 + (hash(i + 2900) * 0x60 as f32) as u32;
                    c.rrect(x + 1.0, y + 1.5, d * 1.2, d, d / 2.0, Color::hex(0x2E2A26).alpha(0.6));
                    c.rrect(x, y, d * 1.2, d, d / 2.0, Color::hex(g << 16 | (g - 4) << 8 | (g - 10)));
                }
            });
        }
        _ => c.clear(Color::hex(0x1A1C22)),
    }
    c
}

/// Your own picture, scaled once to cover `h` pixels high (at least `w` wide), area-averaged
pub fn cover(img: &crosshair_core::image::Image, w: i32, h: i32) -> Canvas {
    let (sw, sh) = (img.w.max(1) as f32, img.h.max(1) as f32);
    let k = (w as f32 / sw).max(h as f32 / sh);
    let (dw, dh) = (((sw * k).round() as i32).max(w), h);
    let mut c = Canvas::new(dw, dh);
    let y0 = ((sh * k - dh as f32) / 2.0).max(0.0);
    for y in 0..dh {
        let (a, b) = (((y as f32 + y0) / k) as usize, (((y + 1) as f32 + y0) / k).ceil() as usize);
        let (a, b) = (a.min(img.h - 1), b.clamp(a + 1, img.h));
        for x in 0..dw {
            let (l, r) = ((x as f32 / k) as usize, (((x + 1) as f32) / k).ceil() as usize);
            let (l, r) = (l.min(img.w - 1), r.clamp(l + 1, img.w));
            let (mut acc, mut n) = ([0u32; 3], 0u32);
            for yy in a..b {
                for xx in l..r {
                    let p = (yy * img.w + xx) * 4;
                    let al = img.rgba[p + 3] as u32;
                    // see-through parts over the Dark background
                    for ch in 0..3 {
                        acc[ch] += (img.rgba[p + ch] as u32 * al + [0x1A, 0x1C, 0x22][ch] * (255 - al)) / 255;
                    }
                    n += 1;
                }
            }
            let n = n.max(1);
            c.px[(y * dw + x) as usize] = 0xFF00_0000 | (acc[0] / n) << 16 | (acc[1] / n) << 8 | (acc[2] / n);
        }
    }
    c
}

/// A `w` x `h` window onto a covered picture, `pan` pixels to the right (wraps around)
pub fn window(src: &Canvas, w: i32, h: i32, pan: f32) -> Canvas {
    let mut c = Canvas::new(w, h);
    if src.w <= 0 || src.h <= 0 {
        return c;
    }
    let off = ((src.w - w) / 2 + pan as i32).rem_euclid(src.w);
    for y in 0..h.min(src.h) {
        for x in 0..w {
            c.px[(y * w + x) as usize] = src.px[(y * src.w + (off + x) % src.w) as usize];
        }
    }
    c
}

/// 0 = normal, 1 = deuteranopia, 2 = protanopia, 3 = tritanopia (Machado et al. 2009, full severity, in linear RGB)
const CVD: [[f32; 9]; 3] = [
    [0.367322, 0.860646, -0.227968, 0.280085, 0.672501, 0.047413, -0.011820, 0.042940, 0.968881],
    [0.152286, 1.052583, -0.204868, 0.114503, 0.786281, 0.099216, -0.003882, -0.048116, 1.051998],
    [1.255528, -0.076749, -0.178779, -0.078411, 0.930809, 0.147602, 0.004733, 0.691367, 0.303900],
];

fn to_lin(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
fn to_srgb(l: f32) -> u32 {
    let l = l.clamp(0.0, 1.0);
    let c = if l <= 0.003_130_8 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
    (c * 255.0 + 0.5) as u32
}

/// Applies a colour-blind view to an opaque canvas (the composed preview)
pub fn color_blind(c: &mut Canvas, mode: u32) {
    let Some(m) = CVD.get((mode as usize).wrapping_sub(1)) else { return };
    let lin: Vec<f32> = (0..=255u8).map(to_lin).collect();
    // linear -> sRGB through a table (4096 steps is finer than 8-bit output)
    let back: Vec<u32> = (0..4096).map(|i| to_srgb(i as f32 / 4095.0)).collect();
    let q = |l: f32| back[(l.clamp(0.0, 1.0) * 4095.0 + 0.5) as usize];
    for p in c.px.iter_mut() {
        let (r, g, b) = (lin[((*p >> 16) & 255) as usize], lin[((*p >> 8) & 255) as usize], lin[(*p & 255) as usize]);
        let (r2, g2, b2) = (m[0] * r + m[1] * g + m[2] * b, m[3] * r + m[4] * g + m[5] * b, m[6] * r + m[7] * g + m[8] * b);
        *p = (*p & 0xFF00_0000) | (q(r2) << 16) | (q(g2) << 8) | q(b2);
    }
}

/// The whole preview: background (built-in, or your own picture), crosshair in the middle (integer zoom), colour-blind view
pub fn compose(kind: u32, own: Option<&Canvas>, pan: f32, cross: &Canvas, zoom: i32, cvd: u32) -> Canvas {
    let mut c = match own {
        Some(o) => window(o, SIZE, SIZE, pan),
        None => backdrop(kind, SIZE, SIZE, pan),
    };
    let z = zoom.max(1);
    // the crosshair's centre pixel on the preview's centre (the same rounding as the overlay: canvas centre = w / 2)
    c.blit(SIZE / 2 - (cross.w / 2) * z, SIZE / 2 - (cross.h / 2) * z, cross, z);
    color_blind(&mut c, cvd);
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_backdrop_is_opaque_and_the_pan_loops() {
        for k in 0..NAMES.len() as u32 {
            let a = backdrop(k, 64, 64, 10.0);
            assert!(a.px.iter().all(|p| p >> 24 == 255), "backdrop {k} has see-through pixels");
            // one full period of pan more = the same picture
            let b = backdrop(k, 64, 64, 74.0);
            assert_eq!(a.px, b.px, "backdrop {k}: pan doesn't loop");
        }
    }

    #[test]
    fn grey_stays_grey_in_every_view_and_red_changes() {
        for m in 1..=3 {
            let mut c = Canvas::new(2, 1);
            c.px[0] = 0xFF80_8080;
            c.px[1] = 0xFFFF_0000;
            color_blind(&mut c, m);
            let g = c.px[0];
            let (r, gg, b) = ((g >> 16) & 255, (g >> 8) & 255, g & 255);
            assert!(r.abs_diff(128) <= 2 && gg.abs_diff(128) <= 2 && b.abs_diff(128) <= 2, "mode {m}: grey -> {g:08x}");
            assert_ne!(c.px[1], 0xFFFF_0000, "mode {m}: red unchanged");
        }
    }
}
