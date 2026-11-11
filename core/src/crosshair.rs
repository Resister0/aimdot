//! Crosshair rasterisation.
//!
//! Geometry is rendered once into two 8-bit coverage masks (fill + outline) and cached by a
//! geometry hash. Colouring those masks is a single cheap pass, so colour changes and the
//! RGB rainbow animation never re-rasterise anything.

use crate::config::Profile;
use crate::gfx::{Canvas, Color};
use crate::image::{self, Image};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub struct Masks {
    pub size: i32,
    pub fill: Vec<u8>,
    pub outline: Vec<u8>,
}

struct R {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    op: f32,
}
struct Ring {
    c: f32,
    r0: f32,
    r1: f32,
    op: f32,
}

/// Hash of every field that affects mask geometry.
pub fn geom_key(p: &Profile) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut add = |v: u32| {
        for b in v.to_le_bytes() {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
    };
    let f = |on: bool, v: f32| if on { v.to_bits() } else { 0 };
    add(f(p.outline, p.outline_thick));
    add(p.dot as u32 | (p.dot_round as u32) << 1 | (p.inner as u32) << 2 | (p.outer as u32) << 3);
    add(p.circle as u32 | (p.t_style as u32) << 1);
    add(f(p.dot, p.dot_size));
    add(f(p.dot, p.dot_opacity));
    for (on, v) in [
        (p.inner, [p.inner_len, p.inner_thick, p.inner_gap, p.inner_opacity]),
        (p.outer, [p.outer_len, p.outer_thick, p.outer_gap, p.outer_opacity]),
        (p.circle, [p.circle_radius, p.circle_thick, p.circle_opacity, 0.0]),
    ] {
        for x in v {
            add(f(on, x));
        }
    }
    add(p.rotation.to_bits());
    add(p.scale.to_bits());
    h
}

fn arms(rects: &mut Vec<R>, len: f32, t: f32, gap: f32, op: f32, co: f32, hc: f32, axis: bool, t_style: bool) {
    let b0 = if axis { (co - t / 2.0).round() } else { co - t / 2.0 };
    let b1 = b0 + t;
    let s = co + hc + gap;
    let e = co - hc - gap;
    rects.push(R { x0: s, y0: b0, x1: s + len, y1: b1, op });
    rects.push(R { x0: e - len, y0: b0, x1: e, y1: b1, op });
    rects.push(R { x0: b0, y0: s, x1: b1, y1: s + len, op });
    if !t_style {
        rects.push(R { x0: b0, y0: e - len, x1: b1, y1: e, op });
    }
    if gap <= 0.0 {
        rects.push(R { x0: b0, y0: b0, x1: b1, y1: b1, op });
    }
}

pub fn build_masks(p: &Profile) -> Masks {
    let sc = p.scale;
    let rot = p.rotation.rem_euclid(360.0);
    let q = (rot / 90.0).round();
    // Axis-aligned crosshairs are snapped to whole pixels so they stay razor sharp.
    let axis = (rot - q * 90.0).abs() < 0.01;
    let (sn, cs) = if axis {
        match (q as i32) & 3 {
            0 => (0.0, 1.0),
            1 => (1.0, 0.0),
            2 => (0.0, -1.0),
            _ => (-1.0, 0.0),
        }
    } else {
        rot.to_radians().sin_cos()
    };
    let dim = |v: f32| if axis { (v * sc).round().max(1.0) } else { (v * sc).max(0.5) };
    let gapv = |v: f32| if axis { (v * sc).round().max(0.0) } else { (v * sc).max(0.0) };
    let o = if p.outline { dim(p.outline_thick) } else { 0.0 };

    // Odd primary thickness => centre the whole design on a pixel instead of a pixel boundary.
    let primary = if p.inner {
        Some(dim(p.inner_thick))
    } else if p.outer {
        Some(dim(p.outer_thick))
    } else if p.dot {
        Some(dim(p.dot_size))
    } else if p.circle {
        Some(dim(p.circle_thick))
    } else {
        None
    };
    let odd = axis && primary.map_or(false, |t| (t as i32) % 2 == 1);
    let (co, hc) = if odd { (-0.5, 0.5) } else { (0.0, 0.0) };

    let mut rects: Vec<R> = Vec::new();
    let mut rings: Vec<Ring> = Vec::new();
    if p.circle && p.circle_opacity > 0.0 {
        let t = dim(p.circle_thick);
        let r0 = if axis { (p.circle_radius * sc - t / 2.0).round().max(0.0) + hc } else { (p.circle_radius * sc - t / 2.0).max(0.0) };
        rings.push(Ring { c: co, r0, r1: r0 + t, op: p.circle_opacity });
    }
    if p.outer && p.outer_opacity > 0.0 {
        arms(&mut rects, dim(p.outer_len), dim(p.outer_thick), gapv(p.outer_gap), p.outer_opacity, co, hc, axis, p.t_style);
    }
    if p.inner && p.inner_opacity > 0.0 {
        arms(&mut rects, dim(p.inner_len), dim(p.inner_thick), gapv(p.inner_gap), p.inner_opacity, co, hc, axis, p.t_style);
    }
    if p.dot && p.dot_opacity > 0.0 {
        let d = dim(p.dot_size);
        let x0 = if axis { (co - d / 2.0).round() } else { co - d / 2.0 };
        if p.dot_round {
            rings.push(Ring { c: x0 + d / 2.0, r0: -1.0, r1: d / 2.0, op: p.dot_opacity });
        } else {
            rects.push(R { x0, y0: x0, x1: x0 + d, y1: x0 + d, op: p.dot_opacity });
        }
    }

    let mut ext = 1.0f32;
    for r in &rects {
        let mx = (r.x0 - co).abs().max((r.x1 - co).abs());
        let my = (r.y0 - co).abs().max((r.y1 - co).abs());
        ext = ext.max((mx * mx + my * my).sqrt());
    }
    for g in &rings {
        ext = ext.max(g.r1 + (g.c - co).abs() * 1.5);
    }
    let half = ((ext + o + 2.5).ceil() as i32).clamp(4, 512);
    let n = half * 2;
    let mut fill = vec![0u8; (n * n) as usize];
    let mut outline = vec![0u8; (n * n) as usize];
    let inv = |x: f32, y: f32| -> (f32, f32) {
        let (dx, dy) = (x - co, y - co);
        (dx * cs + dy * sn + co, -dx * sn + dy * cs + co)
    };
    let m = o + 1.0;
    const SS: i32 = 4;
    for py in 0..n {
        for px in 0..n {
            let (ccx, ccy) = (px as f32 + 0.5 - half as f32, py as f32 + 0.5 - half as f32);
            let (ux, uy) = inv(ccx, ccy);
            let near = rects.iter().any(|r| ux > r.x0 - m && ux < r.x1 + m && uy > r.y0 - m && uy < r.y1 + m)
                || rings.iter().any(|g| {
                    let d = ((ux - g.c) * (ux - g.c) + (uy - g.c) * (uy - g.c)).sqrt();
                    d < g.r1 + m && d > g.r0 - m
                });
            if !near {
                continue;
            }
            let (mut fs, mut os) = (0.0f32, 0.0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let (ux, uy) = inv(
                        ccx - 0.5 + (sx as f32 + 0.5) / SS as f32,
                        ccy - 0.5 + (sy as f32 + 0.5) / SS as f32,
                    );
                    let (mut f, mut ov) = (0.0f32, 0.0f32);
                    for r in &rects {
                        if ux >= r.x0 - o && ux < r.x1 + o && uy >= r.y0 - o && uy < r.y1 + o {
                            ov = ov.max(r.op);
                            if ux >= r.x0 && ux < r.x1 && uy >= r.y0 && uy < r.y1 {
                                f = f.max(r.op);
                            }
                        }
                    }
                    for g in &rings {
                        let d = ((ux - g.c) * (ux - g.c) + (uy - g.c) * (uy - g.c)).sqrt();
                        if d <= g.r1 + o && d >= g.r0 - o {
                            ov = ov.max(g.op);
                            if d <= g.r1 && d >= g.r0 {
                                f = f.max(g.op);
                            }
                        }
                    }
                    fs += f;
                    os += ov;
                }
            }
            let i = (py * n + px) as usize;
            let k = 255.0 / (SS * SS) as f32;
            fill[i] = (fs * k + 0.5) as u8;
            outline[i] = (os * k + 0.5) as u8;
        }
    }
    Masks { size: n, fill, outline }
}

/// Per-pixel colour source: a solid colour or one of the rainbow modes.
struct Tint {
    mode: u32,
    hue: f32,
    sat: f32,
    solid: (f32, f32, f32),
}

impl Tint {
    fn new(p: &Profile, t: f64) -> Tint {
        if p.rainbow {
            let hue = (t * p.rainbow_speed as f64 * 0.2).fract() as f32;
            let c = Color::from_hsv(hue, p.rainbow_sat, 1.0);
            Tint { mode: p.rainbow_mode + 1, hue, sat: p.rainbow_sat, solid: (c.r as f32, c.g as f32, c.b as f32) }
        } else {
            Tint { mode: 0, hue: 0.0, sat: 0.0, solid: (p.color.r as f32, p.color.g as f32, p.color.b as f32) }
        }
    }
    #[inline]
    fn at(&self, dx: f32, dy: f32) -> (f32, f32, f32) {
        let h = match self.mode {
            2 => self.hue - (dx + dy) * 0.012,
            3 => self.hue + dy.atan2(dx) / std::f32::consts::TAU,
            _ => return self.solid,
        };
        let c = Color::from_hsv(h, self.sat, 1.0);
        (c.r as f32, c.g as f32, c.b as f32)
    }
}

#[inline]
fn pack_pm(a: f32, r: f32, g: f32, b: f32) -> u32 {
    let a8 = (a * 255.0 + 0.5) as u32;
    let c = |v: f32| ((v + 0.5) as u32).min(a8);
    (a8 << 24) | (c(r) << 16) | (c(g) << 8) | c(b)
}

pub fn compose(m: &Masks, p: &Profile, t: f64, out: &mut Canvas) {
    let n = m.size;
    out.resize(n, n);
    let tint = Tint::new(p, t);
    let oop = if p.outline { p.outline_opacity } else { 0.0 };
    let (or, og, ob) = (p.outline_color.r as f32, p.outline_color.g as f32, p.outline_color.b as f32);
    let half = n as f32 / 2.0;
    for y in 0..n {
        for x in 0..n {
            let i = (y * n + x) as usize;
            let (f8, o8) = (m.fill[i], m.outline[i]);
            if f8 == 0 && (o8 == 0 || oop <= 0.0) {
                out.px[i] = 0;
                continue;
            }
            let fa = f8 as f32 / 255.0;
            let oa = o8 as f32 / 255.0 * oop * (1.0 - fa);
            let (r, g, b) = tint.at(x as f32 + 0.5 - half, y as f32 + 0.5 - half);
            let k = p.opacity;
            out.px[i] = pack_pm((fa + oa) * k, (r * fa + or * oa) * k, (g * fa + og * oa) * k, (b * fa + ob * oa) * k);
        }
    }
}

fn compose_image(src: &Canvas, p: &Profile, t: f64, out: &mut Canvas) {
    let (w, h) = ((src.w + 1) & !1, (src.h + 1) & !1);
    out.resize(w, h);
    out.px.fill(0);
    let tint = Tint::new(p, t);
    let k = p.img_opacity;
    let (hx, hy) = (src.w as f32 / 2.0, src.h as f32 / 2.0);
    for y in 0..src.h {
        for x in 0..src.w {
            let s = src.px[(y * src.w + x) as usize];
            let a = (s >> 24) as f32 / 255.0;
            if a <= 0.0 {
                continue;
            }
            let v = if p.img_tint {
                let (r, g, b) = tint.at(x as f32 + 0.5 - hx, y as f32 + 0.5 - hy);
                pack_pm(a * k, r * a * k, g * a * k, b * a * k)
            } else {
                pack_pm(a * k, ((s >> 16) & 255) as f32 * k, ((s >> 8) & 255) as f32 * k, (s & 255) as f32 * k)
            };
            out.px[(y * w + x) as usize] = v;
        }
    }
}

pub type Loader = fn(&Path) -> Option<Image>;

type Stamp = (u64, std::time::SystemTime);

fn stamp(p: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.len(), m.modified().ok()?))
}

/// Renders profiles to canvases, owning all caches (masks, decoded and scaled images).
pub struct Renderer {
    masks: HashMap<u64, Rc<Masks>>,
    /// Decoded images by file name, with the file's (length, modified time) when it was read: a file that was
    /// changed (the painter's live draft) or replaced (deleted + imported again under the same name) is read again.
    images: HashMap<String, (Option<Stamp>, Option<Rc<Image>>)>,
    /// The scaled picture last drawn: (name, scale bits, smooth, animation frame, canvas).
    scaled: Option<(String, u32, bool, usize, Rc<Canvas>)>,
    thumbs: HashMap<(String, i32), Rc<Canvas>>,
    pub img_dir: PathBuf,
    loader: Loader,
}

impl Renderer {
    pub fn new(img_dir: PathBuf, loader: Loader) -> Self {
        Renderer { masks: HashMap::new(), images: HashMap::new(), scaled: None, thumbs: HashMap::new(), img_dir, loader }
    }

    pub fn masks(&mut self, p: &Profile) -> Rc<Masks> {
        let key = geom_key(p);
        if let Some(m) = self.masks.get(&key) {
            return m.clone();
        }
        if self.masks.len() >= 96 {
            self.masks.clear();
        }
        let m = Rc::new(build_masks(p));
        self.masks.insert(key, m.clone());
        m
    }

    pub fn image(&mut self, name: &str) -> Option<Rc<Image>> {
        if name.is_empty() {
            return None;
        }
        let path = self.img_dir.join(name);
        let st = stamp(&path);
        if self.images.get(name).map_or(false, |e| e.0 != st) {
            self.forget(name);
        }
        if !self.images.contains_key(name) {
            let img = (self.loader)(&path).filter(|i| i.w > 0 && i.h > 0).map(Rc::new);
            self.images.insert(name.to_string(), (st, img));
        }
        self.images.get(name).and_then(|e| e.1.clone())
    }

    pub fn forget(&mut self, name: &str) {
        self.images.remove(name);
        self.thumbs.retain(|k, _| k.0 != name);
        self.scaled = None;
    }

    /// One frame of a picture: the whole picture, or for an animated one (`paint::anim_of`) frame `i` of the strip.
    pub fn frame(&mut self, name: &str, i: usize) -> Option<Rc<Image>> {
        let img = self.image(name)?;
        match crate::paint::anim_of(name) {
            Some((k, _)) if img.w % k == 0 && img.w >= k => {
                let (fw, i) = (img.w / k, i % k);
                let mut rgba = Vec::with_capacity(fw * img.h * 4);
                for y in 0..img.h {
                    let row = (y * img.w + i * fw) * 4;
                    rgba.extend_from_slice(&img.rgba[row..row + fw * 4]);
                }
                Some(Rc::new(Image { w: fw, h: img.h, rgba }))
            }
            _ => Some(img),
        }
    }

    /// Pixel size the image will have on screen for a profile (one frame of an animated picture).
    pub fn image_size(&mut self, p: &Profile) -> Option<(usize, usize)> {
        let img = self.frame(&p.image, 0)?;
        let f = |v: usize| ((v as f32 * p.img_scale).round() as usize).clamp(1, 1024);
        Some((f(img.w), f(img.h)))
    }

    /// Thumbnail fitted inside `size` x `size`.
    pub fn thumb(&mut self, name: &str, size: i32) -> Option<Rc<Canvas>> {
        let img = self.frame(name, 0)?; // first: drops the cached thumbnail if the file changed
        let key = (name.to_string(), size);
        if let Some(t) = self.thumbs.get(&key) {
            return Some(t.clone());
        }
        let k = (size as f32 / img.w.max(img.h) as f32).min(4.0);
        let c = Rc::new(image::scale_to(
            &img,
            ((img.w as f32 * k).round() as usize).max(1),
            ((img.h as f32 * k).round() as usize).max(1),
            k < 2.0,
        ));
        self.thumbs.insert(key, c.clone());
        Some(c)
    }

    /// Renders `p` at time `t` into `out`, resizing it. The design centre is the canvas centre.
    pub fn render(&mut self, p: &Profile, t: f64, out: &mut Canvas) {
        if p.mode == 1 {
            if let Some((w, h)) = self.image_size(p) {
                let f = match crate::paint::anim_of(&p.image) {
                    Some((k, fps)) => ((t.max(0.0) * fps as f64).floor() as usize) % k,
                    None => 0,
                };
                let hit = matches!(&self.scaled, Some((n, s, sm, fr, _)) if n == &p.image && *s == p.img_scale.to_bits() && *sm == p.img_smooth && *fr == f);
                if !hit {
                    let img = self.frame(&p.image, f).unwrap();
                    let c = Rc::new(image::scale_to(&img, w, h, p.img_smooth));
                    self.scaled = Some((p.image.clone(), p.img_scale.to_bits(), p.img_smooth, f, c));
                }
                let src = self.scaled.as_ref().unwrap().4.clone();
                compose_image(&src, p, t, out);
                return;
            }
            out.resize(2, 2);
            out.px.fill(0);
            return;
        }
        let m = self.masks(p);
        compose(&m, p, t, out);
    }
}

/// Built-in presets ("standard options"). Only geometry matters; colour is kept from the user.
pub fn presets() -> Vec<(&'static str, Profile)> {
    let base = || Profile { inner: false, dot: false, outer: false, circle: false, ..Profile::default() };
    let mk = |f: &dyn Fn(&mut Profile)| {
        let mut p = base();
        f(&mut p);
        p
    };
    let cross = |p: &mut Profile, len: f32, thick: f32, gap: f32| {
        p.inner = true;
        p.inner_len = len;
        p.inner_thick = thick;
        p.inner_gap = gap;
    };
    let dot = |p: &mut Profile, size: f32, round: bool| {
        p.dot = true;
        p.dot_size = size;
        p.dot_round = round;
    };
    let ring = |p: &mut Profile, r: f32, t: f32| {
        p.circle = true;
        p.circle_radius = r;
        p.circle_thick = t;
    };
    vec![
        ("Classic", mk(&|p| cross(p, 6.0, 2.0, 3.0))),
        ("Classic Dot", mk(&|p| { cross(p, 6.0, 2.0, 4.0); dot(p, 2.0, false); })),
        ("Compact", mk(&|p| cross(p, 4.0, 2.0, 2.0))),
        ("Plus", mk(&|p| cross(p, 6.0, 2.0, 0.0))),
        ("Bold Plus", mk(&|p| cross(p, 7.0, 4.0, 0.0))),
        ("Wide Gap", mk(&|p| cross(p, 7.0, 2.0, 8.0))),
        ("Needle", mk(&|p| { cross(p, 14.0, 1.0, 3.0); p.outline = false; })),
        ("Dot", mk(&|p| dot(p, 4.0, false))),
        ("Micro Dot", mk(&|p| dot(p, 2.0, false))),
        ("Round Dot", mk(&|p| dot(p, 6.0, true))),
        ("Box", mk(&|p| { dot(p, 8.0, false); p.outline_thick = 2.0; })),
        ("Circle", mk(&|p| ring(p, 9.0, 2.0))),
        ("Circle Dot", mk(&|p| { ring(p, 11.0, 2.0); dot(p, 4.0, true); })),
        ("Halo", mk(&|p| { ring(p, 18.0, 1.0); dot(p, 2.0, false); })),
        ("Ring Cross", mk(&|p| { ring(p, 13.0, 2.0); cross(p, 5.0, 2.0, 3.0); })),
        ("Scope", mk(&|p| { ring(p, 16.0, 2.0); cross(p, 9.0, 2.0, 20.0); dot(p, 2.0, false); })),
        ("T-Shape", mk(&|p| { cross(p, 7.0, 2.0, 3.0); dot(p, 2.0, false); p.t_style = true; })),
        ("X", mk(&|p| { cross(p, 7.0, 2.0, 3.0); p.rotation = 45.0; })),
        ("X Dot", mk(&|p| { cross(p, 6.0, 2.0, 5.0); dot(p, 4.0, true); p.rotation = 45.0; })),
        ("Dual", mk(&|p| {
            cross(p, 5.0, 2.0, 3.0);
            p.outer = true;
            p.outer_len = 3.0;
            p.outer_thick = 2.0;
            p.outer_gap = 12.0;
            p.outer_opacity = 0.6;
        })),
    ]
}

/// Applies a preset's shape to `cur`, keeping the user's colour, rainbow and image settings.
pub fn apply_shape(cur: &mut Profile, shape: &Profile) {
    let keep = cur.clone();
    *cur = shape.clone();
    cur.mode = 0;
    cur.color = keep.color;
    cur.opacity = keep.opacity;
    cur.rainbow = keep.rainbow;
    cur.rainbow_mode = keep.rainbow_mode;
    cur.rainbow_speed = keep.rainbow_speed;
    cur.rainbow_sat = keep.rainbow_sat;
    cur.outline_color = keep.outline_color;
    cur.image = keep.image;
    cur.img_scale = keep.img_scale;
    cur.img_opacity = keep.img_opacity;
    cur.img_tint = keep.img_tint;
    cur.img_smooth = keep.img_smooth;
}
