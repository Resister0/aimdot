//! Painter page: paint a crosshair pixel by pixel (crosshair-core's paint.rs does the pixels). While you paint,
//! the picture goes live to the overlay: it is written as a hidden draft PNG into the image library
//! (`.paint-a.png` / `.paint-b.png`, taking turns so the state the overlay gets really changes, and at most
//! ~30 times a second) and chosen as the current image. "Save to images" writes it under a real name.
//! Also: symmetry modes and shape tools, import from a game screenshot (centre crop + colour key), and export
//! (PNG at 1x / 2x / 4x, copy the picture to the clipboard, "Copy for Discord").

use super::{with_app, App, AppWindow, LayerRow, Paint, S};
use crosshair_core::config::Profile;
use crosshair_core::gfx::Color;
use crosshair_core::image::Image;
use crosshair_core::paint::{self, Effect, Painter, Px, Shape, Sym, CLEAR, MAX_FRAMES, MAX_SIZE};
use std::path::PathBuf;
use slint::{ComponentHandle, ModelRc, SharedPixelBuffer, SharedString, VecModel};
use std::cell::RefCell;
use std::time::Duration;

/// S.page of the Painter page (app.slint's nav).
pub const PAGE: i32 = 5;
/// The live drafts take turns (`.paint-a` / `.paint-b`, + ` (anim 4x8)` when animated).
const DRAFTS: [&str; 2] = [".paint-a", ".paint-b"];

const BRUSH: i32 = 0;
const ERASER: i32 = 1;
const FILL: i32 = 2;
const LINE: i32 = 3;
const PICK: i32 = 4;
const DISK: i32 = 5;
const RING: i32 = 6;
const SQUARE: i32 = 7;
const CROSS: i32 = 8;

fn shape_of(tool: i32) -> Option<Shape> {
    Some(match tool {
        DISK => Shape::Disk,
        RING => Shape::Ring,
        SQUARE => Shape::Square,
        CROSS => Shape::Cross,
        _ => return None,
    })
}

/// A screenshot being turned into a canvas: its centre (MAX_SIZE square), the crosshair colour and how close
/// a pixel must be to it.
struct Shot {
    crop: Vec<Px>,
    key: [u8; 3],
    tol: f32,
    one: bool,
}

impl Shot {
    /// The centre n x n of the stored crop.
    fn crop(&self, n: usize) -> Vec<Px> {
        let o = (MAX_SIZE - n) / 2;
        (0..n * n).map(|i| self.crop[(i / n + o) * MAX_SIZE + i % n + o]).collect()
    }
}

struct St {
    ui: Option<slint::Weak<AppWindow>>,
    p: Painter,
    tool: i32,
    brush: i32,
    color: Px,
    outline: bool,
    outline_color: Px,
    /// The open colour picker: "" / "color" / "outline"; its h/s/v as given (so the hue doesn't jump).
    pick_key: String,
    hsv: (f32, f32, f32),
    /// Mouse: last cell of a stroke, start of a line, erasing (right button).
    last: Option<(i32, i32)>,
    line: Option<((i32, i32), (i32, i32))>,
    erase: bool,
    live: bool,
    /// The crosshair before the painter took over the screen (back when "Show on screen" is switched off).
    before: Option<Profile>,
    draft: usize,
    scale: f32,
    name: String,
    /// The library file this picture was opened from (saving with that name overwrites it).
    from: String,
    push: slint::Timer,
    checker_n: usize,
    gap: i32,
    shot: Option<Shot>,
    export_scale: i32,
    /// Layers ticked for the multi-layer actions (same order as `p.layers()`).
    sel: Vec<bool>,
    /// Animation: the frame shown on the canvas, playing, the frame before as a faint ghost.
    view: usize,
    play: bool,
    play_timer: slint::Timer,
    onion: bool,
    /// The slider being dragged (one undo step per drag, not one per move).
    drag_key: String,
}

impl Default for St {
    fn default() -> Self {
        St {
            ui: None,
            p: Painter::default(),
            tool: BRUSH,
            brush: 1,
            color: [0, 255, 127, 255],
            outline: true,
            outline_color: [0, 0, 0, 255],
            pick_key: String::new(),
            hsv: (150.0, 1.0, 1.0),
            last: None,
            line: None,
            erase: false,
            live: true,
            before: None,
            draft: 0,
            scale: 1.0,
            name: "My crosshair".into(),
            from: String::new(),
            push: slint::Timer::default(),
            checker_n: 0,
            gap: 2,
            shot: None,
            export_scale: 2,
            sel: vec![true],
            view: 0,
            play: false,
            play_timer: slint::Timer::default(),
            onion: false,
            drag_key: String::new(),
        }
    }
}

thread_local! {
    static ST: RefCell<St> = RefCell::new(St::default());
}

/// Never call `with_app` (or anything that ends in App::changed) inside this: App::refresh comes back here.
fn st<R>(f: impl FnOnce(&mut St) -> R) -> R {
    ST.with(|s| f(&mut s.borrow_mut()))
}

fn px_of(c: slint::Color, alpha: f32) -> Px {
    [c.red(), c.green(), c.blue(), (alpha.clamp(0.0, 1.0) * 255.0).round() as u8]
}

fn scol(p: Px) -> slint::Color {
    slint::Color::from_rgb_u8(p[0], p[1], p[2])
}

fn hex(p: Px) -> String {
    Color { r: p[0], g: p[1], b: p[2], a: 255 }.to_hex()
}

fn hsv_of(p: Px) -> (f32, f32, f32) {
    let (h, s, v) = Color { r: p[0], g: p[1], b: p[2], a: 255 }.to_hsv();
    (h * 360.0, s, v)
}

/// Straight RGBA cells -> a Slint image, one pixel per cell (shown big with pixelated scaling).
fn cells(n: usize, px: &[Px]) -> slint::Image {
    let mut buf = SharedPixelBuffer::<slint::Rgba8Pixel>::new(n as u32, n as u32);
    for (d, s) in buf.make_mut_slice().iter_mut().zip(px) {
        *d = slint::Rgba8Pixel { r: s[0], g: s[1], b: s[2], a: s[3] };
    }
    slint::Image::from_rgba8(buf)
}

impl St {
    fn outline_px(&self) -> Option<Px> {
        self.outline.then_some(self.outline_color)
    }

    /// Everything into the page.
    fn show(&mut self) {
        let Some(ui) = self.ui.as_ref().and_then(|w| w.upgrade()) else { return };
        let g = ui.global::<Paint>();
        let n = self.p.n;
        // the picture (+ outline, + the line being dragged)
        let mut q = self.p.sketch();
        if let Some(l) = self.line {
            self.draw_drag(&mut q, l);
        }
        let k = self.p.frame_count();
        self.view %= k;
        let phase = self.view as f32 / k as f32;
        let (_, px) = q.outlined_at(self.outline_px(), 0, phase);
        g.set_layer(cells(n, &px));
        let ghost = self.onion && k > 1 && !self.play;
        g.set_onion_on(ghost);
        if ghost {
            let prev = (self.view + k - 1) % k;
            g.set_onion(cells(n, &self.p.composite(prev as f32 / k as f32)));
        }
        self.show_layers(&g);
        g.set_frames(k as i32);
        g.set_fps(self.p.fps);
        g.set_play(self.play);
        g.set_view(self.view as i32);
        g.set_onion_wanted(self.onion);
        if self.checker_n != n {
            self.checker_n = n;
            let ck: Vec<Px> = (0..n * n).map(|i| if (i % n + i / n) % 2 == 0 { [128, 128, 128, 36] } else { CLEAR }).collect();
            g.set_checker(cells(n, &ck));
        }
        g.set_n(n as i32);
        g.set_tool(self.tool);
        g.set_brush(self.brush);
        g.set_sym(self.p.sym.index());
        g.set_gap(self.gap);
        g.set_export_scale(self.export_scale);
        g.set_shot(self.shot.is_some());
        if let Some(sh) = &self.shot {
            g.set_shot_img(cells(n, &sh.crop(n)));
            g.set_shot_key(slint::Color::from_rgb_u8(sh.key[0], sh.key[1], sh.key[2]));
            g.set_shot_tol(sh.tol);
            g.set_shot_one(sh.one);
        }
        g.set_color(scol(self.color));
        g.set_alpha(self.color[3] as f32 / 255.0);
        g.set_outline(self.outline);
        g.set_outline_color(scol(self.outline_color));
        g.set_can_undo(self.p.can_undo());
        g.set_can_redo(self.p.can_redo());
        g.set_empty(self.p.is_empty());
        g.set_live(self.live);
        g.set_scale(self.scale);
        let out = if self.outline { n + 2 } else { n };
        let on = (out as f32 * self.scale).round() as usize;
        g.set_size_text(format!("{n} px{}  ·  {on} × {on} px on screen", if self.outline { " + outline" } else { "" }).into());
        g.set_name(self.name.as_str().into());
        g.set_from(self.from.as_str().into());
        g.set_pick_key(self.pick_key.as_str().into());
        g.set_ph(self.hsv.0);
        g.set_ps(self.hsv.1);
        g.set_pv(self.hsv.2);
        let pc = if self.pick_key == "outline" { self.outline_color } else { self.color };
        g.set_phex(hex(pc).into());
    }

    fn show_layers(&mut self, g: &Paint) {
        let n = self.p.n;
        self.sel.resize(self.p.layers().len(), false);
        let active = self.p.active();
        let rows: Vec<LayerRow> = (0..self.p.layers().len())
            .rev()
            .map(|i| {
                let l = &self.p.layers()[i];
                let px: Vec<Px> = self.p.layer_px(i).iter().map(|p| if p[3] == 0 { CLEAR } else { let c = l.tint.unwrap_or([p[0], p[1], p[2]]); [c[0], c[1], c[2], p[3]] }).collect();
                LayerRow {
                    index: i as i32,
                    name: l.name.as_str().into(),
                    img: cells(n, &px),
                    visible: l.visible,
                    on: i == active,
                    sel: self.sel[i],
                    opacity: l.opacity,
                    tinted: l.tint.is_some(),
                    tint: l.tint.map_or(slint::Color::from_rgb_u8(255, 255, 255), |t| slint::Color::from_rgb_u8(t[0], t[1], t[2])),
                    effect: l.effect.index(),
                    effect_name: l.effect.name().into(),
                }
            })
            .collect();
        let nsel = self.sel.iter().filter(|&&b| b).count();
        g.set_layer_count(rows.len() as i32);
        g.set_sel_count(nsel as i32);
        // what the "chosen layers" controls show: the active layer's values
        let l = &self.p.layers()[active];
        g.set_sel_opacity(l.opacity);
        g.set_sel_effect(l.effect.index());
        g.set_sel_tinted(l.tint.is_some());
        g.set_layers(ModelRc::new(VecModel::from(rows)));
    }

    /// The ticked layers (at least the active one).
    fn chosen(&self) -> Vec<usize> {
        let v: Vec<usize> = self.sel.iter().enumerate().filter(|(_, &b)| b).map(|(i, _)| i).collect();
        if v.is_empty() {
            vec![self.p.active()]
        } else {
            v
        }
    }

    /// One undo step for a change to the chosen layers (a slider drag counts once).
    fn layer_change(&mut self, key: &str) {
        if key.is_empty() || self.drag_key != key {
            self.p.checkpoint();
        }
        self.drag_key = key.to_string();
    }

    /// The line or shape being dragged from `a` to `b`.
    fn draw_drag(&self, q: &mut Painter, (a, b): ((i32, i32), (i32, i32))) {
        let c = if self.erase { CLEAR } else { self.color };
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        match shape_of(self.tool) {
            Some(sh @ (Shape::Disk | Shape::Ring)) => {
                let r = ((dx * dx + dy * dy) as f32).sqrt().round() as i32;
                q.shape(sh, a.0, a.1, r, c, self.brush, self.gap);
            }
            Some(sh) => q.shape(sh, a.0, a.1, dx.abs().max(dy.abs()), c, self.brush, self.gap),
            None => q.line(a.0, a.1, b.0, b.1, c, self.brush),
        }
    }

    /// Screenshot import: the canvas = the kept pixels of the screenshot's centre.
    fn shot_apply(&mut self) {
        let col = self.color;
        if let Some(sh) = &self.shot {
            let n = self.p.n;
            let px = paint::key_out(&sh.crop(n), sh.key, sh.tol, sh.one.then_some(col));
            self.p.set_pixels(px);
        }
    }

    /// The finished picture (frame 0) at the export size (nearest neighbour, so pixels stay sharp).
    fn export_scaled(&self) -> Image {
        upscale(&self.p.export(self.outline_px()), self.export_scale.clamp(1, 8) as usize)
    }

    /// Every animation frame at the export size.
    fn export_scaled_frames(&self) -> Vec<Image> {
        self.p.export_frames(self.outline_px()).iter().map(|f| upscale(f, self.export_scale.clamp(1, 8) as usize)).collect()
    }

    /// What goes into the image library / to the overlay: one picture, or the frames side by side.
    fn library_picture(&self) -> Image {
        if self.p.frame_count() > 1 {
            self.p.export_strip(self.outline_px())
        } else {
            self.p.export(self.outline_px())
        }
    }

    /// `stem.png`, or `stem (anim 4x8).png` when animated.
    fn file_name(&self, stem: &str) -> String {
        let k = self.p.frame_count();
        if k > 1 {
            paint::anim_name(stem, k, self.p.fps)
        } else {
            format!("{stem}.png")
        }
    }

    /// After a change to the picture: show it, and send it to the overlay soon.
    fn edited(&mut self) {
        self.show();
        if self.live && !self.push.running() {
            self.push.start(slint::TimerMode::SingleShot, Duration::from_millis(33), push_live);
        }
    }
}

fn status(text: impl Into<SharedString>, ok: bool) {
    st(|s| {
        if let Some(ui) = s.ui.as_ref().and_then(|w| w.upgrade()) {
            ui.global::<Paint>().set_status(text.into());
            ui.global::<Paint>().set_status_ok(ok);
        }
    });
}

fn is_draft(p: &Profile) -> bool {
    p.mode == 1 && p.image.starts_with(".paint-")
}

/// Each pixel becomes a k x k block.
fn upscale(img: &Image, k: usize) -> Image {
    if k <= 1 {
        return img.clone();
    }
    let (w, h) = (img.w * k, img.h * k);
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let i = ((y / k) * img.w + x / k) * 4;
            rgba.extend_from_slice(&img.rgba[i..i + 4]);
        }
    }
    Image { w, h, rgba }
}

/// A picture as SVG: one rectangle per run of same-coloured pixels in a row, crisp edges, `k` = size factor.
pub fn svg_of(img: &Image, k: usize) -> String {
    let mut s = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\" shape-rendering=\"crispEdges\">\n",
        img.w * k,
        img.h * k,
        img.w,
        img.h
    );
    let px = |x: usize, y: usize| &img.rgba[(y * img.w + x) * 4..(y * img.w + x) * 4 + 4];
    for y in 0..img.h {
        let mut x = 0;
        while x < img.w {
            let p = px(x, y);
            let mut end = x + 1;
            while end < img.w && px(end, y) == p {
                end += 1;
            }
            if p[3] > 0 {
                s.push_str(&format!("<rect x=\"{x}\" y=\"{y}\" width=\"{}\" height=\"1\" fill=\"#{:02x}{:02x}{:02x}\"", end - x, p[0], p[1], p[2]));
                if p[3] < 255 {
                    s.push_str(&format!(" fill-opacity=\"{:.3}\"", p[3] as f32 / 255.0));
                }
                s.push_str("/>\n");
            }
            x = end;
        }
    }
    s.push_str("</svg>\n");
    s
}

/// An animated GIF (loops forever; GIF has only on/off transparency).
pub fn gif_of(frames: &[Image], fps: f32) -> Result<Vec<u8>, String> {
    use image::codecs::gif::{GifEncoder, Repeat};
    let mut out = Vec::new();
    {
        let mut enc = GifEncoder::new_with_speed(&mut out, 10);
        enc.set_repeat(Repeat::Infinite).map_err(|e| e.to_string())?;
        let ms = (1000.0 / fps.max(1.0)).round() as u32;
        for f in frames {
            let buf = image::RgbaImage::from_raw(f.w as u32, f.h as u32, f.rgba.clone()).ok_or("bad frame")?;
            enc.encode_frame(image::Frame::from_parts(buf, 0, 0, image::Delay::from_numer_denom_ms(ms, 1))).map_err(|e| e.to_string())?;
        }
    }
    Ok(out)
}

/// An animated PNG (full transparency; loops forever).
pub fn apng_of(frames: &[Image], fps: f32) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let (w, h) = (frames[0].w as u32, frames[0].h as u32);
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_animated(frames.len() as u32, 0).map_err(|e| e.to_string())?;
        enc.set_frame_delay(1, fps.round().clamp(1.0, 60.0) as u16).map_err(|e| e.to_string())?;
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        for f in frames {
            w.write_image_data(&f.rgba).map_err(|e| e.to_string())?;
        }
        w.finish().map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Writes the draft file and makes it the crosshair on screen.
fn push_live() {
    let (png, scale) = st(|s| {
        let png = if s.live { super::images::encode_png(&s.library_picture()).ok() } else { None };
        (png, s.scale)
    });
    let Some(png) = png else { return };
    with_app(|a| {
        let draft = st(|s| {
            s.draft = 1 - s.draft;
            s.file_name(DRAFTS[s.draft])
        });
        let draft = draft.as_str();
        if super::images::write_atomic(&a.m.rend.img_dir.join(draft), &png).is_err() {
            return;
        }
        if !is_draft(&a.m.cur) {
            let cur = a.m.cur.clone();
            st(|s| s.before = Some(cur));
        }
        a.m.cur.mode = 1;
        a.m.cur.image = draft.to_string();
        a.m.cur.img_scale = scale;
        a.m.cur.img_smooth = false;
        a.changed();
    });
}

/// "Show on screen while painting" switched off: the crosshair from before comes back.
fn restore() {
    let before = st(|s| s.before.take());
    with_app(|a| {
        if is_draft(&a.m.cur) {
            if let Some(b) = before {
                a.m.cur = b;
            } else {
                a.m.cur.mode = 0;
            }
            a.changed();
        }
    });
}

/// Images page: "Edit in painter".
pub fn open_image(a: &mut App, file: &str) {
    // an animated picture opens as its first frame (the layers + effects aren't stored in the PNG)
    let Some(img) = a.m.rend.frame(file, 0) else { return };
    let anim = paint::anim_of(file);
    let ok = st(|s| {
        s.p.checkpoint();
        if s.p.load(&img) {
            s.sel = vec![true];
            s.from = if anim.is_some() { String::new() } else { file.to_string() };
            s.name = file.rsplit_once('.').map_or(file, |x| x.0).to_string();
            if let Some(at) = s.name.rfind(" (anim ") {
                s.name.truncate(at);
            }
            // a picture with its own outline already has it painted in
            s.outline = false;
            s.show();
            true
        } else {
            s.p.drop_checkpoint_if_same();
            false
        }
    });
    if let Some(ui) = a.ui() {
        if ok {
            ui.global::<S>().set_page(PAGE);
        }
    }
    if ok {
        status(format!("Opened {file} - saving with the same name replaces it"), true);
    }
}

fn save(name: &str) {
    let name = super::images::clean_stem(name);
    let (img, from, want) = st(|s| {
        s.name = name.clone();
        (s.library_picture(), s.from.clone(), s.file_name(&name))
    });
    if st(|s| s.p.is_empty()) {
        status("Paint something first", false);
        return;
    }
    let png = match super::images::encode_png(&img) {
        Ok(p) => p,
        Err(e) => return status(e, false),
    };
    with_app(|a| {
        let dir = a.m.rend.img_dir.clone();
        let file = if from.eq_ignore_ascii_case(&want) {
            from.clone()
        } else if want.ends_with(").png") {
            // animated: the marker must stay at the end, so a taken name gets a number before it
            (1..).map(|i| if i == 1 { want.clone() } else { want.replacen(" (anim ", &format!(" {i} (anim "), 1) }).find(|f| !dir.join(f).exists()).unwrap()
        } else {
            super::images::free_name(&dir, &name)
        };
        if let Err(e) = super::images::write_atomic(&dir.join(&file), &png) {
            status(e, false);
            return;
        }
        let scale = st(|s| {
            s.from = file.clone();
            s.name = name.clone();
            s.before = None;
            s.scale
        });
        a.m.rend.forget(&file);
        a.m.scan_images();
        a.m.cur.mode = 1;
        a.m.cur.image = file.clone();
        a.m.cur.img_scale = scale;
        a.m.cur.img_smooth = false;
        status(format!("Saved as {file} - it's in Images now and on screen"), true);
        a.changed();
    });
    st(|s| s.show());
}

/// Opens a game screenshot: its centre becomes the canvas (one undo step; "Cancel" goes back).
fn open_shot(path: &std::path::Path) {
    let img = std::fs::read(path).map_err(|e| format!("Could not read that file ({e})")).and_then(|b| super::images::decode_full(&b));
    let img = match img {
        Ok(i) => i,
        Err(e) => return status(e, false),
    };
    st(|s| {
        let crop = paint::centre_crop(&img, MAX_SIZE);
        let n = s.p.n;
        let key = paint::auto_key(&Shot { crop: crop.clone(), key: [0; 3], tol: 0.0, one: false }.crop(n), n);
        if s.shot.is_none() {
            s.p.checkpoint();
        }
        s.shot = Some(Shot { crop, key, tol: 0.12, one: false });
        s.from.clear();
        s.shot_apply();
        s.edited();
    });
    status("Click the crosshair's color in the small picture if the guess is wrong, then \"Keep\"", true);
}

thread_local! {
    /// Kept while the window is open: on Linux the copied picture lives as long as this does (a clipboard
    /// manager like Klipper keeps it after that).
    static CLIP: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
}

/// Runs `f` with the clipboard; Some(error text) when it fails.
fn clip(f: impl FnOnce(&mut arboard::Clipboard) -> Result<(), String>) -> Option<String> {
    CLIP.with(|c| {
        let mut c = c.borrow_mut();
        if c.is_none() {
            match arboard::Clipboard::new() {
                Ok(cb) => *c = Some(cb),
                Err(e) => return Some(format!("The clipboard isn't available ({e})")),
            }
        }
        f(c.as_mut().unwrap()).err().map(|e| format!("Could not copy ({e})"))
    })
}

/// The picture (at the export size) onto the clipboard; Some(error text) when it fails.
fn copy_image() -> Option<String> {
    let img = st(|s| (!s.p.is_empty()).then(|| s.export_scaled()));
    let Some(img) = img else { return Some("Paint something first".into()) };
    clip(|c| {
        c.set_image(arboard::ImageData { width: img.w, height: img.h, bytes: std::borrow::Cow::Owned(img.rgba) }).map_err(|e| e.to_string())
    })
}

/// Asks where to save, then writes `bytes` there. `ext`: "png" / "svg" / "gif".
fn save_as(file: String, ext: &'static str, bytes: Vec<u8>) {
    let owner = super::images::owner_handle();
    std::thread::spawn(move || {
        let r = super::images::save_file(owner, &file, ext);
        let _ = slint::invoke_from_event_loop(move || match r {
            Ok(Some(p)) => {
                let p: PathBuf = if p.extension().map_or(true, |e| !e.eq_ignore_ascii_case(ext)) { p.with_extension(ext) } else { p };
                match std::fs::write(&p, &bytes) {
                    Ok(()) => status(format!("Saved {}", p.display()), true),
                    Err(e) => status(format!("Could not save ({e})"), false),
                }
            }
            Ok(None) => status("", true),
            Err(e) => status(e, false),
        });
    });
}

/// "Save PNG…" / "Save SVG…" / "Save GIF…" / "Save APNG…".
fn export_as(kind: &str) {
    let (empty, name, k) = st(|s| (s.p.is_empty(), super::images::clean_stem(&s.name), s.export_scale));
    if empty {
        return status("Paint something first", false);
    }
    let r: Result<(String, &'static str, Vec<u8>), String> = match kind {
        "svg" => {
            let img = st(|s| s.p.export(s.outline_px()));
            Ok((format!("{name}.svg"), "svg", svg_of(&img, k as usize).into_bytes()))
        }
        "gif" | "apng" => {
            let (frames, fps) = st(|s| (s.export_scaled_frames(), s.p.fps));
            let bytes = if kind == "gif" { gif_of(&frames, fps) } else { apng_of(&frames, fps) };
            bytes.map(|b| if kind == "gif" { (format!("{name} {k}x.gif"), "gif", b) } else { (format!("{name} {k}x.png"), "png", b) })
        }
        _ => {
            let img = st(|s| s.export_scaled());
            super::images::encode_png(&img).map(|b| (format!("{name} {k}x.png"), "png", b))
        }
    };
    match r {
        Ok((file, ext, bytes)) => save_as(file, ext, bytes),
        Err(e) => status(format!("Could not make the file ({e})"), false),
    }
}

/// Plays the animation on the canvas (frame by frame at the chosen speed).
fn set_play(on: bool) {
    st(|s| {
        s.play = on && s.p.frame_count() > 1;
        if s.play {
            let ms = (1000.0 / s.p.fps.max(1.0)) as u64;
            s.play_timer.start(slint::TimerMode::Repeated, Duration::from_millis(ms.max(16)), || {
                st(|s| {
                    s.view = (s.view + 1) % s.p.frame_count();
                    s.show();
                })
            });
        } else {
            s.play_timer.stop();
        }
        s.show();
    });
}

/// Test hook (screenshots): CROSSHAIR_PAINT_DEMO=1 starts with a small painted crosshair (=live: also on screen,
/// =clip: also presses "Copy for Discord").
fn demo(p: &mut Painter) {
    let c = p.centre() as i32;
    let g = [0, 255, 127, 255];
    p.sym = Sym::XY;
    for i in 3..8 {
        p.dab(c - i, c, g, 1);
        p.dab(c, c - i, g, 1);
    }
    p.dab(c, c, [255, 40, 80, 255], 1);
    for (x, y) in Painter::line_cells(c - 9, c - 4, c - 4, c - 9) {
        p.dab(x, y, [255, 230, 0, 200], 1);
    }
    p.sym = Sym::Eight;
    p.shape(Shape::Ring, c, c, 12, [0, 229, 255, 160], 1, 0);
    p.sym = Sym::Off;
}

pub fn init(ui: &AppWindow) {
    let demo_mode = std::env::var("CROSSHAIR_PAINT_DEMO").ok();
    st(|s| {
        s.ui = Some(ui.as_weak());
        if demo_mode.is_some() {
            demo(&mut s.p);
        }
        if demo_mode.as_deref() == Some("anim") {
            // three layers that move: the lines stay, a pulsing ring, a blinking dot; 4 frames
            let c = s.p.centre() as i32;
            s.p.layer_mut(0).unwrap().name = "Lines".into();
            s.p.add_layer();
            s.p.layer_mut(1).unwrap().name = "Ring".into();
            s.p.layer_mut(1).unwrap().effect = Effect::Pulse;
            s.p.layer_mut(1).unwrap().tint = Some([255, 230, 0]);
            s.p.shape(Shape::Ring, c, c, 10, [255, 255, 255, 255], 1, 0);
            s.p.add_layer();
            s.p.layer_mut(2).unwrap().name = "Dot".into();
            s.p.layer_mut(2).unwrap().effect = Effect::Blink;
            s.p.shape(Shape::Disk, c, c, 1, [255, 45, 85, 255], 1, 0);
            s.p.frames = 4;
            s.p.fps = 4.0;
            s.sel = vec![false, true, true];
            s.view = 2;
            s.onion = true;
        }
        if let Some(t) = std::env::var("CROSSHAIR_PAINT_TAB").ok().and_then(|v| v.parse().ok()) {
            if let Some(ui) = s.ui.as_ref().and_then(|w| w.upgrade()) {
                ui.global::<Paint>().set_tab(t);
            }
        }
        s.show();
    });
    // CROSSHAIR_SHOT=<file>: start a screenshot import (test hook)
    if let Some(p) = std::env::var_os("CROSSHAIR_SHOT") {
        std::env::remove_var("CROSSHAIR_SHOT");
        let p = PathBuf::from(p);
        slint::Timer::single_shot(Duration::from_millis(300), move || open_shot(&p));
    }
    // CROSSHAIR_PAINT_DEMO=live: also send it to the overlay, like a brush stroke would
    if matches!(demo_mode.as_deref(), Some("live") | Some("anim")) {
        slint::Timer::single_shot(Duration::from_millis(1500), push_live);
    }
    // CROSSHAIR_PAINT_DEMO=clip: press "Copy for Discord" (clipboard test)
    if demo_mode.as_deref() == Some("clip") {
        let w = ui.as_weak();
        slint::Timer::single_shot(Duration::from_millis(1500), move || {
            if let Some(ui) = w.upgrade() {
                ui.global::<Paint>().invoke_copy_discord();
            }
        });
    }
    let g = ui.global::<Paint>();
    g.on_down(|x, y, right| {
        st(|s| {
            s.drag_key.clear();
            s.erase = right || s.tool == ERASER;
            let c = if s.erase { CLEAR } else { s.color };
            match s.tool {
                PICK if !right => {
                    if let Some(p) = s.p.get(x, y).filter(|p| p[3] > 0) {
                        s.color = p;
                        s.hsv = hsv_of(p);
                        s.tool = BRUSH;
                    }
                    s.show();
                }
                FILL if !right => {
                    s.p.checkpoint();
                    s.p.fill(x, y, c);
                    s.p.drop_checkpoint_if_same();
                    s.edited();
                }
                LINE | DISK | RING | SQUARE | CROSS => {
                    s.line = Some(((x, y), (x, y)));
                    s.show();
                }
                _ => {
                    s.p.checkpoint();
                    s.p.dab(x, y, c, s.brush);
                    s.last = Some((x, y));
                    s.edited();
                }
            }
        })
    });
    g.on_drag(|x, y| {
        st(|s| {
            if let Some((a, _)) = s.line {
                if s.line.map(|l| l.1) != Some((x, y)) {
                    s.line = Some((a, (x, y)));
                    s.show();
                }
            } else if let Some((lx, ly)) = s.last {
                if (lx, ly) != (x, y) {
                    let c = if s.erase { CLEAR } else { s.color };
                    s.p.line(lx, ly, x, y, c, s.brush);
                    s.last = Some((x, y));
                    s.edited();
                }
            }
        })
    });
    g.on_up(|| {
        st(|s| {
            if let Some(l) = s.line.take() {
                s.p.checkpoint();
                let mut q = s.p.sketch();
                s.draw_drag(&mut q, l);
                s.p.px = q.px;
                s.p.drop_checkpoint_if_same();
                s.edited();
            } else if s.last.take().is_some() {
                s.p.drop_checkpoint_if_same();
                s.show();
            }
        })
    });
    g.on_set_tool(|t| st(|s| {
        s.tool = t.clamp(BRUSH, CROSS);
        s.show();
    }));
    g.on_set_brush(|b| st(|s| {
        s.brush = b.clamp(1, 7) | 1;
        s.show();
    }));
    g.on_set_sym(|i| st(|s| {
        s.p.sym = Sym::from_index(i);
        s.show();
    }));
    g.on_set_gap(|v| st(|s| {
        s.gap = v.clamp(0, 20);
        s.show();
    }));
    g.on_set_size(|n| st(|s| {
        if n as usize != s.p.n {
            if s.shot.is_some() {
                // importing: the bigger / smaller centre of the same screenshot (still one undo step)
                s.p.resize(n.max(1) as usize);
                s.shot_apply();
            } else {
                s.p.checkpoint();
                s.p.resize(n.max(1) as usize);
                s.p.drop_checkpoint_if_same();
            }
            s.edited();
        }
    }));
    g.on_set_color(|c| st(|s| {
        s.color = px_of(c, s.color[3] as f32 / 255.0);
        s.hsv = hsv_of(s.color);
        if s.shot.as_ref().map_or(false, |sh| sh.one) {
            s.shot_apply();
            s.edited();
        } else {
            s.show();
        }
    }));
    g.on_set_alpha(|a| st(|s| {
        s.color[3] = (a.clamp(0.1, 1.0) * 255.0).round() as u8;
        s.show();
    }));
    g.on_set_outline(|on| st(|s| {
        s.outline = on;
        s.edited();
    }));
    g.on_set_outline_color(|c| st(|s| {
        s.outline_color = px_of(c, 1.0);
        s.edited();
    }));
    g.on_open_picker(|key| st(|s| {
        s.pick_key = key.to_string();
        let c = if s.pick_key == "outline" { s.outline_color } else { s.color };
        s.hsv = hsv_of(c);
        s.show();
    }));
    g.on_pick(|h, sat, v| st(|s| {
        s.hsv = (h, sat, v);
        let c = Color::from_hsv(h / 360.0, sat, v);
        if s.pick_key == "outline" {
            s.outline_color = [c.r, c.g, c.b, 255];
            s.edited();
        } else {
            s.color = [c.r, c.g, c.b, s.color[3]];
            s.show();
        }
    }));
    g.on_hex(|text| st(|s| {
        let Some(c) = Color::parse(text.as_str()) else { return };
        s.hsv = hsv_of([c.r, c.g, c.b, 255]);
        if s.pick_key == "outline" {
            s.outline_color = [c.r, c.g, c.b, 255];
            s.edited();
        } else {
            s.color = [c.r, c.g, c.b, s.color[3]];
            s.show();
        }
    }));
    g.on_undo(|| st(|s| {
        if s.p.undo() {
            s.edited();
        }
    }));
    g.on_redo(|| st(|s| {
        if s.p.redo() {
            s.edited();
        }
    }));
    g.on_clear(|| st(|s| {
        if !s.p.is_empty() {
            s.p.checkpoint();
            s.p.clear();
            s.edited();
        }
    }));
    g.on_new_picture(|| {
        st(|s| {
            s.p.checkpoint();
            s.p.clear();
            s.from.clear();
            s.name = "My crosshair".into();
            s.edited();
        });
        status("", true);
    });
    g.on_set_live(|on| {
        st(|s| {
            s.live = on;
            s.show();
        });
        if on {
            if !st(|s| s.p.is_empty()) {
                push_live();
            }
        } else {
            restore();
        }
    });
    g.on_set_scale(|k| {
        let push = st(|s| {
            s.scale = k.clamp(1.0, 4.0).round();
            s.show();
            s.live && !s.p.is_empty()
        });
        if push {
            push_live();
        }
    });
    g.on_save(|name| save(name.as_str()));

    // screenshot import
    g.on_shot_browse(|| {
        status("Choose a screenshot…", true);
        let owner = super::images::owner_handle();
        std::thread::spawn(move || {
            let r = super::images::pick_file(owner);
            let _ = slint::invoke_from_event_loop(move || match r {
                Ok(Some(p)) => open_shot(&p),
                Ok(None) => status("", true),
                Err(e) => status(e, false),
            });
        });
    });
    g.on_shot_path(|text| match super::images::clean_path(text.as_str()) {
        Some(p) => open_shot(&p),
        None => status("Paste the full path of a screenshot first", false),
    });
    g.on_shot_pick(|x, y| st(|s| {
        let n = s.p.n;
        if let Some(sh) = &mut s.shot {
            if x >= 0 && y >= 0 && (x as usize) < n && (y as usize) < n {
                let p = sh.crop(n)[y as usize * n + x as usize];
                if p[3] > 0 {
                    sh.key = [p[0], p[1], p[2]];
                }
            }
        }
        s.shot_apply();
        s.edited();
    }));
    g.on_set_shot_tol(|v| st(|s| {
        if let Some(sh) = &mut s.shot {
            sh.tol = v.clamp(0.0, 1.0);
        }
        s.shot_apply();
        s.edited();
    }));
    g.on_set_shot_one(|on| st(|s| {
        if let Some(sh) = &mut s.shot {
            sh.one = on;
        }
        s.shot_apply();
        s.edited();
    }));
    g.on_shot_keep(|| {
        st(|s| {
            s.shot = None;
            s.show();
        });
        status("Taken from the screenshot - paint on it as you like", true);
    });
    g.on_shot_cancel(|| {
        st(|s| {
            if s.shot.take().is_some() {
                s.p.undo();
                s.p.drop_redo();
            }
            s.edited();
        });
        status("", true);
    });

    // export
    g.on_set_export_scale(|k| st(|s| {
        s.export_scale = k.clamp(1, 4);
        s.show();
    }));
    g.on_export_png(|| export_as("png"));
    g.on_export_svg(|| export_as("svg"));
    g.on_export_anim(|kind| export_as(if kind == "gif" { "gif" } else { "apng" }));

    // layers (the list is top first; indexes are layer numbers, 0 = bottom)
    g.on_layer_pick(|i| st(|s| {
        s.p.select_layer(i.max(0) as usize);
        s.sel = vec![false; s.p.layers().len()];
        if let Some(b) = s.sel.get_mut(i.max(0) as usize) {
            *b = true;
        }
        s.show();
    }));
    g.on_layer_tick(|i, on| st(|s| {
        if let Some(b) = s.sel.get_mut(i.max(0) as usize) {
            *b = on;
        }
        s.show();
    }));
    g.on_layer_all(|| st(|s| {
        let all = s.sel.iter().all(|&b| b);
        s.sel = vec![!all; s.p.layers().len()];
        s.show();
    }));
    g.on_layer_visible(|i, on| st(|s| {
        s.layer_change("");
        if let Some(l) = s.p.layer_mut(i.max(0) as usize) {
            l.visible = on;
        }
        s.edited();
    }));
    g.on_layer_rename(|i, name| st(|s| {
        let name: String = name.trim().chars().take(24).collect();
        if !name.is_empty() {
            s.layer_change("");
            if let Some(l) = s.p.layer_mut(i.max(0) as usize) {
                l.name = name;
            }
        }
        s.show();
    }));
    g.on_layer_add(|| st(|s| {
        s.layer_change("");
        if s.p.add_layer() {
            let a = s.p.active();
            s.sel = (0..s.p.layers().len()).map(|i| i == a).collect();
        } else {
            s.p.drop_checkpoint_if_same();
        }
        s.show();
    }));
    g.on_layer_duplicate(|| st(|s| {
        s.layer_change("");
        let c = s.chosen();
        if !s.p.duplicate_layers(&c) {
            s.p.drop_checkpoint_if_same();
        }
        s.sel = vec![false; s.p.layers().len()];
        s.edited();
    }));
    g.on_layer_delete(|| st(|s| {
        s.layer_change("");
        let c = s.chosen();
        s.p.delete_layers(&c);
        let a = s.p.active();
        s.sel = (0..s.p.layers().len()).map(|i| i == a).collect();
        s.edited();
    }));
    g.on_layer_merge(|| st(|s| {
        s.layer_change("");
        let c = s.chosen();
        if s.p.merge_layers(&c) {
            let a = s.p.active();
            s.sel = (0..s.p.layers().len()).map(|i| i == a).collect();
        } else {
            s.p.drop_checkpoint_if_same();
        }
        s.edited();
    }));
    g.on_layer_move(|up| st(|s| {
        s.layer_change("");
        let a = s.p.active();
        if s.p.move_layer(a, up) {
            let a2 = s.p.active();
            s.sel.swap(a, a2);
        } else {
            s.p.drop_checkpoint_if_same();
        }
        s.edited();
    }));
    g.on_layer_nudge(|dx, dy| st(|s| {
        s.layer_change("");
        let c = s.chosen();
        s.p.nudge(&c, dx, dy);
        s.p.drop_checkpoint_if_same();
        s.edited();
    }));
    g.on_set_layer_opacity(|v| st(|s| {
        s.layer_change("opacity");
        for i in s.chosen() {
            if let Some(l) = s.p.layer_mut(i) {
                l.opacity = v.clamp(0.05, 1.0);
            }
        }
        s.edited();
    }));
    g.on_set_layer_tint(|c| st(|s| {
        s.layer_change("");
        for i in s.chosen() {
            if let Some(l) = s.p.layer_mut(i) {
                l.tint = Some([c.red(), c.green(), c.blue()]);
            }
        }
        s.edited();
    }));
    g.on_clear_layer_tint(|| st(|s| {
        s.layer_change("");
        for i in s.chosen() {
            if let Some(l) = s.p.layer_mut(i) {
                l.tint = None;
            }
        }
        s.edited();
    }));
    g.on_set_layer_effect(|e| st(|s| {
        s.layer_change("");
        for i in s.chosen() {
            if let Some(l) = s.p.layer_mut(i) {
                l.effect = Effect::from_index(e);
            }
        }
        // an effect needs frames to show
        if s.p.frames < 2 && Effect::from_index(e) != Effect::None {
            s.p.frames = 4;
        }
        s.edited();
    }));

    // animation
    g.on_set_frames(|k| {
        let playing = st(|s| {
            s.p.frames = (k.max(1) as usize).min(MAX_FRAMES);
            s.edited();
            s.play
        });
        set_play(playing);
    });
    g.on_set_fps(|v| {
        let playing = st(|s| {
            s.p.fps = v.clamp(1.0, 30.0).round();
            s.edited();
            s.play
        });
        set_play(playing);
    });
    g.on_set_play(set_play);
    g.on_set_view(|v| st(|s| {
        s.view = v.max(0) as usize % s.p.frame_count();
        s.show();
    }));
    g.on_set_onion(|on| st(|s| {
        s.onion = on;
        s.show();
    }));
    g.on_copy_image(|| {
        if let Some(msg) = copy_image() {
            status(msg, false);
        } else {
            let k = st(|s| s.export_scale);
            status(format!("Picture copied ({k}×) - paste it anywhere"), true);
        }
    });
    g.on_copy_discord(|| {
        if let Some(msg) = copy_image() {
            status(msg, false);
            return;
        }
        // animated: the whole strip + (frames, fps), so a friend gets the animation too
        let code = st(|s| {
            let k = s.p.frame_count();
            if k > 1 {
                let prof = Profile { mode: 1, img_scale: s.scale, img_smooth: false, ..Profile::default() };
                crosshair_core::share::encode_anim(&s.name, &prof, Some(&s.p.export_strip(s.outline_px())), Some((k as u32, s.p.fps.round() as u32)))
            } else {
                crosshair_core::share::encode_pixels(&s.name, &s.p.export(s.outline_px()))
            }
        });
        match code {
            Ok(code) => {
                st(|s| {
                    if let Some(ui) = s.ui.as_ref().and_then(|w| w.upgrade()) {
                        ui.global::<Paint>().set_share_code(code.into());
                    }
                });
                status("Picture copied - paste it in Discord, then \"Copy code\" and paste the code after it (friends paste it into Aimdot)", true);
            }
            Err(e) => status(format!("Picture copied. No share code: {e}"), false),
        }
    });
    g.on_paste_code(|text| {
        let r = crosshair_core::share::decode(text.as_str());
        match r {
            Ok(sh) => match sh.pixels {
                Some(strip) => {
                    // an animation opens as its first frame (its layers and effects aren't in the code)
                    let img = match sh.anim {
                        Some((k, _)) if k > 1 && strip.w % k as usize == 0 => {
                            let fw = strip.w / k as usize;
                            let mut rgba = Vec::with_capacity(fw * strip.h * 4);
                            for y in 0..strip.h {
                                rgba.extend_from_slice(&strip.rgba[y * strip.w * 4..(y * strip.w + fw) * 4]);
                            }
                            Image { w: fw, h: strip.h, rgba }
                        }
                        _ => strip,
                    };
                    let ok = st(|s| {
                        s.p.checkpoint();
                        if s.p.load(&img) {
                            s.from.clear();
                            s.outline = false; // a shared picture has its outline painted in already
                            if !sh.name.is_empty() {
                                s.name = sh.name.clone();
                            }
                            s.edited();
                            true
                        } else {
                            s.p.drop_checkpoint_if_same();
                            false
                        }
                    });
                    if ok {
                        status("Opened the shared picture", true);
                    } else {
                        status(format!("That picture is {} × {} px - the painter takes up to {MAX_SIZE} × {MAX_SIZE}", img.w, img.h), false);
                    }
                }
                None => status("That code is a Designer crosshair, not a painted one", false),
            },
            Err(e) => status(e.to_string(), false),
        }
    });
    g.on_copy_text(|t| {
        if let Some(msg) = clip(|c| c.set_text(t.as_str()).map_err(|e| e.to_string())) {
            status(msg, false);
        } else {
            status("Share code copied", true);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_sizes_keep_pixels_sharp() {
        let mut s = St::default();
        s.p = Painter::new(9);
        s.outline = false;
        s.p.dab(4, 4, [255, 0, 0, 255], 1);
        for k in [1, 2, 4] {
            s.export_scale = k;
            let img = s.export_scaled();
            let k = k as usize;
            assert_eq!((img.w, img.h), (9 * k, 9 * k));
            let at = |x: usize, y: usize| &img.rgba[(y * img.w + x) * 4..(y * img.w + x) * 4 + 4];
            assert_eq!(at(4 * k, 4 * k), &[255, 0, 0, 255]);
            assert_eq!(at(5 * k - 1, 5 * k - 1), &[255, 0, 0, 255]);
            assert_eq!(at(5 * k, 4 * k)[3], 0);
        }
    }

    #[test]
    fn a_painting_travels_in_a_share_code() {
        let mut p = Painter::new(21);
        p.sym = Sym::Eight;
        p.shape(Shape::Cross, 10, 10, 6, [0, 255, 127, 255], 1, 2);
        p.dab(10, 10, [255, 40, 80, 200], 1);
        let img = p.export(Some([0, 0, 0, 255]));
        let code = crosshair_core::share::encode_pixels("Test", &img).unwrap();
        assert!(code.starts_with("AIM1:"));
        let back = crosshair_core::share::decode(&code).unwrap();
        let px = back.pixels.unwrap();
        assert_eq!(px, img);
        let mut q = Painter::new(31);
        assert!(q.load(&px));
        assert_eq!(q.n, 23);
    }

    #[test]
    fn svg_gif_apng() {
        let mut p = Painter::new(9);
        p.dab(4, 4, [255, 0, 0, 255], 1);
        p.dab(5, 4, [255, 0, 0, 255], 1);
        p.dab(4, 5, [0, 0, 255, 128], 1);
        let img = p.export(None);
        let svg = svg_of(&img, 4);
        assert!(svg.starts_with("<svg") && svg.contains("width=\"36\"") && svg.contains("viewBox=\"0 0 9 9\""));
        assert!(svg.contains("<rect x=\"4\" y=\"4\" width=\"2\" height=\"1\" fill=\"#ff0000\"/>"), "{svg}");
        assert!(svg.contains("fill-opacity=\"0.502\""));
        assert_eq!(svg.matches("<rect").count(), 2);
        // animated: 4 frames
        p.layer_mut(0).unwrap().effect = Effect::Spin;
        p.frames = 4;
        let frames = p.export_frames(None);
        let gif = gif_of(&frames, 8.0).unwrap();
        let dec = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(&gif)).unwrap();
        use image::AnimationDecoder;
        assert_eq!(dec.into_frames().count(), 4);
        let apng = apng_of(&frames, 8.0).unwrap();
        let r = png::Decoder::new(std::io::Cursor::new(&apng)).read_info().unwrap();
        assert_eq!(r.info().animation_control().map(|a| a.num_frames), Some(4));
    }

    #[test]
    fn animated_files_are_named_for_the_overlay() {
        let mut s = St::default();
        assert_eq!(s.file_name("Ring"), "Ring.png");
        s.p.frames = 4;
        s.p.fps = 12.0;
        assert_eq!(s.file_name("Ring"), "Ring (anim 4x12).png");
        s.outline = false;
        s.p = Painter::new(9);
        s.p.frames = 4;
        s.p.dab(4, 4, [255; 4], 1);
        let strip = s.library_picture();
        assert_eq!((strip.w, strip.h), (36, 9));
        assert!(is_draft(&Profile { mode: 1, image: ".paint-b (anim 4x8).png".into(), ..Profile::default() }));
    }

    #[test]
    fn an_animation_travels_in_a_share_code() {
        let mut p = Painter::new(15);
        p.dab(7, 4, [0, 255, 127, 255], 1);
        p.layer_mut(0).unwrap().effect = Effect::Spin;
        p.frames = 4;
        let strip = p.export_strip(None);
        let prof = Profile { mode: 1, ..Profile::default() };
        let code = crosshair_core::share::encode_anim("Spin", &prof, Some(&strip), Some((4, 8))).unwrap();
        let back = crosshair_core::share::decode(&code).unwrap();
        assert_eq!(back.anim, Some((4, 8)));
        assert_eq!(back.pixels.unwrap(), strip);
    }

    #[test]
    fn screenshot_centre_follows_the_canvas_size() {
        let crop: Vec<Px> = (0..MAX_SIZE * MAX_SIZE).map(|i| [(i % MAX_SIZE) as u8, (i / MAX_SIZE) as u8, 0, 255]).collect();
        let sh = Shot { crop, key: [0; 3], tol: 0.1, one: false };
        let c = sh.crop(15);
        assert_eq!(c.len(), 225);
        // the middle pixel of the small crop is the middle of the big one
        assert_eq!(c[7 * 15 + 7], [31, 31, 0, 255]);
    }
}
