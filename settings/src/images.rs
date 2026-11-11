//! Images page: import a picture (PNG, JPG, BMP, GIF or SVG - turned into PNG, because the overlay only decodes
//! PNG; big pictures are made smaller), the library with thumbnails, select / delete, and the size (10-400 %),
//! smoothing, opacity and tint of the chosen image. Import: the system file dialog (Windows: the common
//! "Open" dialog; Linux: kdialog or zenity), a pasted path, or a file dropped on the window (Windows / X11 through
//! winit, Wayland through wl_dnd.rs).
//! SVG (crosshairs exported from other crosshair apps): drawn here with resvg (already linked by Slint) at a size
//! the user picks - by default the SVG's own size, at most share::MAX_SIDE (128 px) - with sharp edges (each
//! edge pixel snapped on or off), so a 1 px line on whole pixels stays 1 px. A small panel on the page shows the result before it is saved.

use super::{to_image, with_app, App, AppWindow, ImgItem, Imgs};
use crosshair_core::image::{self as cimg, Image};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::path::{Path, PathBuf};

/// Longest side of an imported picture (the engine draws at most 1024 px anyway).
pub const MAX_SIDE: usize = 1024;
/// Files bigger than this are not read at all.
const MAX_FILE: u64 = 64 << 20;
pub const EXTS: [&str; 6] = ["png", "jpg", "jpeg", "bmp", "gif", "svg"];
const THUMB: i32 = 64;
/// SVG files bigger than this are refused (crosshair SVGs are a few KB).
const MAX_SVG: u64 = 4 << 20;
/// The SVG size choice: smallest / biggest long side in px.
pub const SVG_MIN: u32 = 4;
pub const SVG_MAX: u32 = 256;

#[cfg(target_os = "linux")]
#[path = "wl_dnd.rs"]
mod wl_dnd;

/// Shown in the list: PNGs that aren't the painter's hidden live drafts (".paint-a.png").
pub fn listed(name: &str) -> bool {
    !name.starts_with('.') && name.to_ascii_lowercase().ends_with(".png")
}

#[derive(Default)]
struct St {
    /// What the list shows (names + file stamps + selection); rebuilt only when it changes.
    key: String,
    ui: Option<slint::Weak<AppWindow>>,
    /// An SVG waiting for its size (the panel on the page).
    svg: Option<SvgWait>,
}

struct SvgWait {
    stem: String,
    bytes: Vec<u8>,
    /// its own size (px)
    own: (f32, f32),
    side: u32,
    sharp: bool,
}

thread_local! {
    static ST: RefCell<St> = RefCell::new(St::default());
}

// ------------------------------------------------------------------ decoding / encoding (no UI)

/// Decodes PNG / JPG / BMP / GIF (first frame) into straight RGBA, at most MAX_SIDE px on the long side.
pub fn decode(bytes: &[u8]) -> Result<Image, String> {
    Ok(fit(decode_full(bytes)?, MAX_SIDE))
}

/// Decodes at the full size (screenshots for the painter: only their centre is used).
pub fn decode_full(bytes: &[u8]) -> Result<Image, String> {
    use image::ImageFormat as F;
    let bad = || "That file isn't a PNG, JPG, BMP or GIF picture".to_string();
    let fmt = image::guess_format(bytes).map_err(|_| bad())?;
    if !matches!(fmt, F::Png | F::Jpeg | F::Bmp | F::Gif) {
        return Err(bad());
    }
    let mut r = image::ImageReader::with_format(std::io::Cursor::new(bytes), fmt);
    let mut lim = image::Limits::default();
    lim.max_image_width = Some(16384);
    lim.max_image_height = Some(16384);
    lim.max_alloc = Some(512 << 20);
    r.limits(lim);
    let img = r.decode().map_err(|e| format!("That picture could not be read ({e})"))?.into_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    Ok(Image { w, h, rgba: img.into_raw() })
}

/// Makes a picture smaller (area average, like the engine) so its long side is at most `max`.
pub fn fit(img: Image, max: usize) -> Image {
    let big = img.w.max(img.h);
    if big <= max {
        return img;
    }
    let k = max as f32 / big as f32;
    let (w, h) = (((img.w as f32 * k).round() as usize).max(1), ((img.h as f32 * k).round() as usize).max(1));
    let c = cimg::scale_to(&img, w, h, true);
    let mut rgba = Vec::with_capacity(w * h * 4);
    for &p in &c.px {
        let a = p >> 24;
        let un = |v: u32| if a == 0 { 0 } else { ((v * 255 + a / 2) / a).min(255) as u8 };
        rgba.extend_from_slice(&[un((p >> 16) & 255), un((p >> 8) & 255), un(p & 255), a as u8]);
    }
    Image { w: c.w as usize, h: c.h as usize, rgba }
}

pub fn encode_png(img: &Image) -> Result<Vec<u8>, String> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::ImageEncoder;
    let mut out = Vec::new();
    PngEncoder::new_with_quality(&mut out, CompressionType::Best, FilterType::Adaptive)
        .write_image(&img.rgba, img.w as u32, img.h as u32, image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("Could not make the PNG ({e})"))?;
    Ok(out)
}

/// File-name stem without characters that are a problem on Windows or in config.ini.
pub fn clean_stem(s: &str) -> String {
    let s: String = s.chars().filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ' ' | '(' | ')')).take(40).collect();
    let s = s.trim();
    if s.is_empty() {
        "image".into()
    } else {
        s.into()
    }
}

/// `stem.png`, or `stem (2).png` ... when that name is taken.
pub fn free_name(dir: &Path, stem: &str) -> String {
    let stem = clean_stem(stem);
    let mut name = format!("{stem}.png");
    let mut n = 2;
    while dir.join(&name).exists() {
        name = format!("{stem} ({n}).png");
        n += 1;
    }
    name
}

/// Writes through a temp file + rename, so the overlay never reads half a file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("png.tmp");
    std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Could not save the picture ({e})")
    })
}

/// Copies a picture into the library as PNG and returns its file name. A PNG that Aimdot's own decoder reads
/// and that isn't too big is copied byte for byte; everything else is converted.
pub fn import_file(dir: &Path, src: &Path) -> Result<String, String> {
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == "svg" {
        // straight in at the default size (the page asks for the size first: import_now)
        let (stem, bytes) = read_svg(src)?;
        return import_svg(dir, &stem, &bytes, svg_default_side(svg_size(&bytes)?), true);
    }
    if !EXTS.contains(&ext.as_str()) {
        return Err("Unsupported file type - use PNG, SVG, JPG, BMP or GIF".into());
    }
    let meta = std::fs::metadata(src).map_err(|_| "That file wasn't found".to_string())?;
    if !meta.is_file() {
        return Err("That's a folder, not a picture".into());
    }
    if meta.len() > MAX_FILE {
        return Err("That file is too big (over 64 MB)".into());
    }
    let bytes = std::fs::read(src).map_err(|e| format!("Could not read that file ({e})"))?;
    let direct = cimg::is_png(&bytes) && cimg::decode_png(&bytes).map_or(false, |i| i.w.max(i.h) <= MAX_SIDE);
    let png = if direct { bytes } else { encode_png(&decode(&bytes)?)? };
    // The overlay must be able to read what we write.
    cimg::decode_png(&png).map_err(|_| "That picture could not be converted".to_string())?;
    let _ = std::fs::create_dir_all(dir);
    let name = free_name(dir, src.file_stem().and_then(|s| s.to_str()).unwrap_or("image"));
    write_atomic(&dir.join(&name), &png)?;
    Ok(name)
}

// ------------------------------------------------------------------ SVG

fn is_svg_name(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("svg"))
}

/// No fonts are loaded: text in an SVG is left out. `crisp` = resvg's "crispEdges" (no anti-aliasing).
fn svg_tree(bytes: &[u8], crisp: bool) -> Result<resvg::usvg::Tree, String> {
    let mut o = resvg::usvg::Options::default();
    if crisp {
        o.shape_rendering = resvg::usvg::ShapeRendering::CrispEdges;
    }
    resvg::usvg::Tree::from_data(bytes, &o).map_err(|e| format!("That SVG could not be read ({e})"))
}

/// Sharp edges: every edge pixel becomes fully on or off - on when the shape covers at least half of it - with
/// the shape's own opacity. That opacity comes from `full` = the alpha of a drawing without anti-aliasing, the
/// strongest one around the pixel (3 x 3). resvg's no-anti-aliasing drawing alone isn't mirror-even (a ring got a
/// pixel on one side only), so it only tells the opacity, the smooth drawing tells what is covered.
fn snap_edges(img: &mut Image, full: &[u8]) {
    let (w, h) = (img.w, img.h);
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4 + 3;
            let v = img.rgba[i];
            if v == 0 {
                continue;
            }
            let mut top = 0;
            for yy in y.saturating_sub(1)..(y + 2).min(h) {
                for xx in x.saturating_sub(1)..(x + 2).min(w) {
                    top = top.max(full[yy * w + xx]);
                }
            }
            let on = top > 0 && v as u16 * 2 >= top as u16;
            img.rgba[i] = if on { top } else { 0 };
            if !on {
                img.rgba[i - 3..i].fill(0);
            }
        }
    }
}

/// The SVG's own size in px (its width / height, else its viewBox).
pub fn svg_size(bytes: &[u8]) -> Result<(f32, f32), String> {
    let s = svg_tree(bytes, false)?.size();
    Ok((s.width(), s.height()))
}

/// The size picked when an SVG comes in: its own size (long side), at most the share-code limit.
pub fn svg_default_side((w, h): (f32, f32)) -> u32 {
    (w.max(h).round() as u32).clamp(SVG_MIN, crosshair_core::share::MAX_SIDE as u32)
}

/// Draws an SVG with its long side `side` px (the other side in proportion, rounded to whole pixels), transparent
/// background, the whole SVG (its own canvas, so its centre stays the picture's centre).
pub fn render_svg(bytes: &[u8], side: u32, sharp: bool) -> Result<Image, String> {
    use resvg::tiny_skia::{Pixmap, Transform};
    let t = svg_tree(bytes, false)?;
    let (w, h) = (t.size().width(), t.size().height());
    let side = side.clamp(1, SVG_MAX) as f32;
    let k = side / w.max(h);
    let (pw, ph) = (((w * k).round() as u32).max(1), ((h * k).round() as u32).max(1));
    // drawn at 4x, then each 4 x 4 block averaged: much more even edges than tiny-skia's own anti-aliasing
    // (mirrored pixels of a ring differed by 1/16 there, enough to flip a sharp edge on one side only)
    const SS: u32 = 4;
    let mut pm = Pixmap::new(pw * SS, ph * SS).ok_or("That SVG has no size")?;
    // scale each side on its own, so the SVG fills the whole-pixel canvas exactly (no half pixel at an edge)
    resvg::render(&t, Transform::from_scale((pw * SS) as f32 / w, (ph * SS) as f32 / h), &mut pm.as_mut());
    let (big, bw) = (pm.data(), (pw * SS) as usize);
    let mut rgba = Vec::with_capacity((pw * ph * 4) as usize);
    for y in 0..ph as usize {
        for x in 0..pw as usize {
            // premultiplied sums
            let mut sum = [0u32; 4];
            for yy in y * SS as usize..(y + 1) * SS as usize {
                for xx in x * SS as usize..(x + 1) * SS as usize {
                    let i = (yy * bw + xx) * 4;
                    for c in 0..4 {
                        sum[c] += big[i + c] as u32;
                    }
                }
            }
            let n = SS * SS;
            let a = (sum[3] + n / 2) / n;
            let un = |v: u32| if sum[3] == 0 { 0 } else { ((v * 255 + sum[3] / 2) / sum[3]).min(255) as u8 };
            rgba.extend_from_slice(&[un(sum[0]), un(sum[1]), un(sum[2]), a as u8]);
        }
    }
    let mut img = Image { w: pw as usize, h: ph as usize, rgba };
    if sharp {
        let mut c = Pixmap::new(pw, ph).ok_or("That SVG has no size")?;
        resvg::render(&svg_tree(bytes, true)?, Transform::from_scale(pw as f32 / w, ph as f32 / h), &mut c.as_mut());
        let full: Vec<u8> = c.pixels().iter().map(|p| p.alpha()).collect();
        snap_edges(&mut img, &full);
    }
    if img.rgba.chunks(4).all(|p| p[3] == 0) {
        return Err("That SVG drew nothing at this size (text isn't supported)".into());
    }
    Ok(img)
}

/// Reads an SVG file (checked like the other pictures) and gives its stem + bytes.
fn read_svg(src: &Path) -> Result<(String, Vec<u8>), String> {
    let meta = std::fs::metadata(src).map_err(|_| "That file wasn't found".to_string())?;
    if !meta.is_file() {
        return Err("That's a folder, not a picture".into());
    }
    if meta.len() > MAX_SVG {
        return Err("That SVG is too big (over 4 MB)".into());
    }
    let bytes = std::fs::read(src).map_err(|e| format!("Could not read that file ({e})"))?;
    Ok((src.file_stem().and_then(|s| s.to_str()).unwrap_or("image").to_string(), bytes))
}

/// Saves a drawn SVG into the library as `stem.png` and returns the file name.
pub fn import_svg(dir: &Path, stem: &str, bytes: &[u8], side: u32, sharp: bool) -> Result<String, String> {
    let png = encode_png(&render_svg(bytes, side, sharp)?)?;
    cimg::decode_png(&png).map_err(|_| "That picture could not be converted".to_string())?;
    let _ = std::fs::create_dir_all(dir);
    let name = free_name(dir, stem);
    write_atomic(&dir.join(&name), &png)?;
    Ok(name)
}

/// A pasted path: quotes and `file://` taken off.
pub fn clean_path(text: &str) -> Option<PathBuf> {
    let t = text.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    let t = t.strip_prefix("file://").unwrap_or(t);
    if t.is_empty() {
        return None;
    }
    // %20 etc. from a pasted file:// URL
    let mut out = Vec::new();
    let b = t.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&t[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    let t = String::from_utf8(out).ok()?;
    let t = if let Some(rest) = t.strip_prefix("~/") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(rest).to_string_lossy().into_owned()).unwrap_or(t.clone())
    } else {
        t
    };
    Some(PathBuf::from(t))
}

// ------------------------------------------------------------------ UI

fn ui() -> Option<AppWindow> {
    ST.with(|s| s.borrow().ui.as_ref().and_then(|w| w.upgrade()))
}

fn status(text: impl Into<SharedString>, ok: bool) {
    if let Some(ui) = ui() {
        let g = ui.global::<Imgs>();
        g.set_status(text.into());
        g.set_status_ok(ok);
    }
}

fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |s| s.0)
}

/// Imports `src`, selects it and sends the new state. An SVG first opens the size panel.
pub fn import_now(a: &mut App, src: &Path) {
    if is_svg_name(src) {
        match read_svg(src).and_then(|(stem, bytes)| svg_size(&bytes).map(|own| (stem, bytes, own))) {
            Ok((stem, bytes, own)) => {
                let side = svg_default_side(own);
                ST.with(|s| s.borrow_mut().svg = Some(SvgWait { stem, bytes, own, side, sharp: true }));
                status("", true);
                svg_show();
            }
            Err(e) => status(e, false),
        }
        return;
    }
    let dir = a.m.rend.img_dir.clone();
    match import_file(&dir, src) {
        Ok(name) => added(a, &name),
        Err(e) => status(e, false),
    }
}

fn added(a: &mut App, name: &str) {
    a.m.rend.forget(name);
    a.m.scan_images();
    a.m.select_image(name);
    status(format!("Imported {}", stem(name)), true);
    a.changed();
}

/// The SVG panel: preview at the chosen size + the size text.
fn svg_show() {
    let Some(ui) = ui() else { return };
    let g = ui.global::<Imgs>();
    ST.with(|s| {
        let s = s.borrow();
        let Some(w) = s.svg.as_ref() else {
            g.set_svg_on(false);
            g.set_svg_preview(slint::Image::default());
            return;
        };
        g.set_svg_on(true);
        g.set_svg_name(w.stem.as_str().into());
        g.set_svg_side(w.side as i32);
        g.set_svg_sharp(w.sharp);
        let own = svg_default_side(w.own);
        g.set_svg_own(own as i32);
        g.set_svg_own_text(format!("{} × {} px", w.own.0.round(), w.own.1.round()).into());
        match render_svg(&w.bytes, w.side, w.sharp) {
            Ok(img) => {
                g.set_svg_preview(to_image(&cimg::scale_to(&img, img.w, img.h, false)));
                g.set_svg_preview_w(img.w as i32);
                g.set_svg_preview_h(img.h as i32);
                let big = if img.w.max(img.h) > crosshair_core::share::MAX_SIDE { " - too big for a share code" } else { "" };
                g.set_svg_text(format!("Picture {} × {} px{big}", img.w, img.h).into());
                g.set_svg_ok(true);
            }
            Err(e) => {
                g.set_svg_preview(slint::Image::default());
                g.set_svg_text(e.into());
                g.set_svg_ok(false);
            }
        }
    });
}

/// From any thread (file dialog, drop): import on the UI thread.
fn import_later(p: PathBuf) {
    let _ = slint::invoke_from_event_loop(move || with_app(|a| import_now(a, &p)));
}

/// Pushes the library + the chosen image's settings into the page (called from App::refresh).
pub fn refresh(a: &mut App) {
    let Some(ui) = a.ui() else { return };
    let g = ui.global::<Imgs>();
    let p = a.m.cur.clone();
    let active = p.mode == 1 && !p.image.is_empty();
    let sel = if active { p.image.as_str() } else { "" };
    g.set_active(active && listed(sel));
    g.set_selected(sel.into());
    g.set_scale(p.img_scale);
    g.set_smooth(p.img_smooth);
    g.set_opacity(p.img_opacity);
    g.set_tint(p.img_tint);
    // one frame of an animated picture (frames side by side)
    let orig = if active { a.m.rend.frame(sel, 0).map(|i| (i.w, i.h)) } else { None };
    g.set_missing(active && orig.is_none());
    g.set_can_edit(orig.map_or(false, |(w, h)| w <= crosshair_core::paint::MAX_SIZE && h <= crosshair_core::paint::MAX_SIZE));
    g.set_size_text(match (orig, a.m.rend.image_size(&p)) {
        (Some((ow, oh)), Some((w, h))) => match crosshair_core::paint::anim_of(sel) {
            Some((k, fps)) => format!("On screen {w} × {h} px  (animated: {k} frames, {fps} a second)").into(),
            None => format!("On screen {w} × {h} px  (picture {ow} × {oh} px)").into(),
        },
        _ => SharedString::new(),
    });

    let names: Vec<String> = a.m.images.iter().filter(|n| listed(n)).cloned().collect();
    let mut key = String::from(sel);
    for n in &names {
        let m = std::fs::metadata(a.m.rend.img_dir.join(n)).ok();
        let t = m.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
        key.push_str(&format!("|{n}:{}:{t}", m.map_or(0, |m| m.len())));
    }
    if ST.with(|s| s.borrow().key == key) {
        return;
    }
    let items: Vec<ImgItem> = names
        .iter()
        .map(|n| {
            let (img, ok) = match a.m.rend.thumb(n, THUMB) {
                Some(t) => (to_image(&t), true),
                None => (slint::Image::default(), false),
            };
            ImgItem { file: n.as_str().into(), name: stem(n).into(), img, ok, on: n == sel }
        })
        .collect();
    g.set_rows(items.len().div_ceil(5) as i32);
    g.set_items(ModelRc::new(VecModel::from(items)));
    ST.with(|s| s.borrow_mut().key = key);
}

/// Wires the page's callbacks and drag-and-drop. Call once, after the App is in place.
pub fn init(ui: &AppWindow) {
    ST.with(|s| s.borrow_mut().ui = Some(ui.as_weak()));
    let g = ui.global::<Imgs>();
    g.set_exts("PNG, SVG, JPG, BMP or GIF".into());
    // Windows / X11: winit's own drag and drop; Wayland: wl_dnd.rs
    g.set_can_drop(true);
    g.on_svg_size(|v| {
        ST.with(|s| {
            if let Some(w) = s.borrow_mut().svg.as_mut() {
                w.side = (v.max(0) as u32).clamp(SVG_MIN, SVG_MAX);
            }
        });
        svg_show();
    });
    g.on_svg_set_sharp(|on| {
        ST.with(|s| {
            if let Some(w) = s.borrow_mut().svg.as_mut() {
                w.sharp = on;
            }
        });
        svg_show();
    });
    g.on_svg_cancel(|| {
        ST.with(|s| s.borrow_mut().svg = None);
        svg_show();
    });
    g.on_svg_import(|| {
        let Some(w) = ST.with(|s| s.borrow_mut().svg.take()) else { return };
        with_app(|a| match import_svg(&a.m.rend.img_dir.clone(), &w.stem, &w.bytes, w.side, w.sharp) {
            Ok(name) => added(a, &name),
            Err(e) => {
                status(e, false);
                ST.with(|s| s.borrow_mut().svg = Some(w));
            }
        });
        svg_show();
    });
    g.on_browse(|| {
        status("Choose a picture…", true);
        let owner = owner_handle();
        std::thread::spawn(move || match pick_file(owner) {
            Ok(Some(p)) => import_later(p),
            Ok(None) => {
                let _ = slint::invoke_from_event_loop(|| status("", true));
            }
            Err(e) => {
                let _ = slint::invoke_from_event_loop(move || status(e, false));
            }
        });
    });
    g.on_import_path(|text| match clean_path(text.as_str()) {
        Some(p) => with_app(|a| import_now(a, &p)),
        None => status("Paste the full path of a picture first", false),
    });
    g.on_select(|file| {
        with_app(|a| {
            a.m.select_image(file.as_str());
            a.changed();
        })
    });
    g.on_delete(|file| {
        with_app(|a| {
            a.m.delete_image(file.as_str());
            status(format!("Deleted {}", stem(file.as_str())), true);
            a.changed();
        })
    });
    g.on_num(|key, v| {
        with_app(|a| {
            let v = if key == "img_scale" { (v * 100.0).round() / 100.0 } else { v };
            a.m.cur.set(key.as_str(), &format!("{v}"));
            a.changed();
        })
    });
    g.on_flag(|key, on| {
        with_app(|a| {
            a.m.cur.set(key.as_str(), if on { "1" } else { "0" });
            a.changed();
        })
    });
    g.on_use_design(|| {
        with_app(|a| {
            a.m.cur.mode = 0;
            a.changed();
        })
    });
    g.on_edit(|file| with_app(|a| super::painter::open_image(a, file.as_str())));

    // Test hook (screenshots): CROSSHAIR_IMPORT=<file> imports that file at start.
    if let Some(p) = std::env::var_os("CROSSHAIR_IMPORT") {
        // not passed on to the overlay this window starts (it could open another settings window)
        std::env::remove_var("CROSSHAIR_IMPORT");
        import_later(PathBuf::from(p));
    }

    // Drag and drop: winit has it on Windows and X11 (one DroppedFile per file); Wayland: wl_dnd.rs, started on the
    // first window event (the window exists then). Deferred: the filter runs inside winit's event handling, where
    // the App may be borrowed.
    use slint::winit_030::{winit::event::WindowEvent, EventResult, WinitWindowAccessor};
    ui.window().on_winit_window_event(|_win, ev| {
        #[cfg(target_os = "linux")]
        wl_dnd::start(_win);
        match ev {
            WindowEvent::HoveredFile(_) => drag_over(true),
            WindowEvent::HoveredFileCancelled => drag_over(false),
            WindowEvent::DroppedFile(p) => {
                drag_over(false);
                dropped(vec![p.clone()]);
            }
            _ => {}
        }
        EventResult::Propagate
    });
}

/// A drag with files is over the window (or not any more): the drop zone lights up.
pub fn drag_over(on: bool) {
    if let Some(ui) = ui() {
        ui.global::<Imgs>().set_drag_over(on);
    }
}

/// Files dropped on the window (any page): imported one by one, then the Images page shows. Only one SVG at a
/// time waits for its size - the last one dropped.
pub fn dropped(paths: Vec<PathBuf>) {
    let _ = slint::invoke_from_event_loop(move || {
        drag_over(false);
        if paths.is_empty() {
            status("Only files from this PC can be dropped here", false);
        }
        with_app(|a| {
            for p in &paths {
                import_now(a, p);
            }
        });
        if let Some(ui) = ui() {
            ui.global::<super::S>().set_page(PAGE);
        }
    });
}

/// This page's number in the rail (app.slint).
pub const PAGE: i32 = 4;

/// The settings window's own handle, so the Windows dialog sits on top of it.
pub fn owner_handle() -> isize {
    #[cfg(windows)]
    {
        use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use slint::winit_030::WinitWindowAccessor;
        if let Some(ui) = ui() {
            return ui
                .window()
                .with_winit_window(|w| match w.window_handle().map(|h| h.as_raw()) {
                    Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
                    _ => 0,
                })
                .unwrap_or(0);
        }
    }
    0
}

/// The system "open file" dialog. Ok(None) = cancelled. Runs on its own thread (it blocks).
#[cfg(not(windows))]
pub fn pick_file(_owner: isize) -> Result<Option<PathBuf>, String> {
    use std::process::Command;
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    let tries: [(&str, Vec<String>); 2] = [
        ("kdialog", vec!["--title".into(), "Import a crosshair image".into(), "--getopenfilename".into(), home.clone(), "Pictures (*.png *.svg *.jpg *.jpeg *.bmp *.gif)".into()]),
        (
            "zenity",
            vec!["--file-selection".into(), "--title=Import a crosshair image".into(), "--file-filter=Pictures | *.png *.PNG *.svg *.SVG *.jpg *.JPG *.jpeg *.bmp *.gif".into(), format!("--filename={home}/")],
        ),
    ];
    for (prog, args) in tries {
        match Command::new(prog).args(&args).output() {
            Ok(out) => {
                let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
                return Ok((out.status.success() && !path.is_empty()).then(|| PathBuf::from(path)));
            }
            Err(_) => continue, // not installed
        }
    }
    Err("No file dialog found (kdialog or zenity). Paste the picture's path below instead.".into())
}

#[cfg(windows)]
pub fn pick_file(owner: isize) -> Result<Option<PathBuf>, String> {
    win_dialog(owner, None, "")
}

/// The system "save file" dialog for a `ext` file ("png" / "svg" / "gif"), starting with `name` in the Pictures
/// folder. Ok(None) = cancelled.
#[cfg(windows)]
pub fn save_file(owner: isize, name: &str, ext: &str) -> Result<Option<PathBuf>, String> {
    win_dialog(owner, Some(ext), name)
}

#[cfg(not(windows))]
pub fn save_file(_owner: isize, name: &str, ext: &str) -> Result<Option<PathBuf>, String> {
    use std::process::Command;
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    let pics = PathBuf::from(&home).join("Pictures");
    let dir = if pics.is_dir() { pics } else { PathBuf::from(&home) };
    let start = dir.join(name).to_string_lossy().into_owned();
    let tries: [(&str, Vec<String>); 2] = [
        ("kdialog", vec!["--title".into(), "Export the crosshair".into(), "--getsavefilename".into(), start.clone(), format!("{} file (*.{ext})", ext.to_uppercase())]),
        ("zenity", vec!["--file-selection".into(), "--save".into(), "--confirm-overwrite".into(), "--title=Export the crosshair".into(), format!("--filename={start}")]),
    ];
    for (prog, args) in tries {
        match Command::new(prog).args(&args).output() {
            Ok(out) => {
                let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
                return Ok((out.status.success() && !path.is_empty()).then(|| PathBuf::from(path)));
            }
            Err(_) => continue,
        }
    }
    Err("No file dialog found (kdialog or zenity) - use \"Save to images\" or \"Copy picture\" instead.".into())
}

/// GetOpenFileNameW / GetSaveFileNameW.
#[cfg(windows)]
fn win_dialog(owner: isize, save_ext: Option<&str>, name: &str) -> Result<Option<PathBuf>, String> {
    let save = save_ext.is_some();
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStringExt;
    #[repr(C)]
    #[allow(non_snake_case)]
    struct OpenFileNameW {
        lStructSize: u32,
        hwndOwner: isize,
        hInstance: isize,
        lpstrFilter: *const u16,
        lpstrCustomFilter: *mut u16,
        nMaxCustFilter: u32,
        nFilterIndex: u32,
        lpstrFile: *mut u16,
        nMaxFile: u32,
        lpstrFileTitle: *mut u16,
        nMaxFileTitle: u32,
        lpstrInitialDir: *const u16,
        lpstrTitle: *const u16,
        Flags: u32,
        nFileOffset: u16,
        nFileExtension: u16,
        lpstrDefExt: *const u16,
        lCustData: isize,
        lpfnHook: *const c_void,
        lpTemplateName: *const u16,
        pvReserved: *const c_void,
        dwReserved: u32,
        FlagsEx: u32,
    }
    #[link(name = "comdlg32")]
    extern "system" {
        fn GetOpenFileNameW(ofn: *mut OpenFileNameW) -> i32;
        fn GetSaveFileNameW(ofn: *mut OpenFileNameW) -> i32;
        fn CommDlgExtendedError() -> u32;
    }
    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(reserved: *const c_void, coinit: u32) -> i32;
    }
    const OFN_NOCHANGEDIR: u32 = 0x8;
    const OFN_PATHMUSTEXIST: u32 = 0x800;
    const OFN_FILEMUSTEXIST: u32 = 0x1000;
    const OFN_EXPLORER: u32 = 0x80000;
    const OFN_OVERWRITEPROMPT: u32 = 0x2;
    let wide = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    // "name\0pattern\0name\0pattern\0\0"
    let filter = if let Some(e) = save_ext { wide(&format!("{} file\0*.{e}\0\0", e.to_uppercase())) } else { wide("Pictures (PNG, SVG, JPG, BMP, GIF)\0*.png;*.svg;*.jpg;*.jpeg;*.bmp;*.gif\0All files\0*.*\0\0") };
    let title = wide(if save { "Export the crosshair\0" } else { "Choose a picture\0" });
    let defext = wide(&format!("{}\0", save_ext.unwrap_or("png")));
    let mut file = vec![0u16; 4096];
    for (i, c) in name.encode_utf16().take(4000).enumerate() {
        file[i] = c;
    }
    let mut ofn = OpenFileNameW {
        lStructSize: std::mem::size_of::<OpenFileNameW>() as u32,
        hwndOwner: owner,
        hInstance: 0,
        lpstrFilter: filter.as_ptr(),
        lpstrCustomFilter: std::ptr::null_mut(),
        nMaxCustFilter: 0,
        nFilterIndex: 1,
        lpstrFile: file.as_mut_ptr(),
        nMaxFile: file.len() as u32,
        lpstrFileTitle: std::ptr::null_mut(),
        nMaxFileTitle: 0,
        lpstrInitialDir: std::ptr::null(),
        lpstrTitle: title.as_ptr(),
        Flags: OFN_EXPLORER | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | if save { OFN_OVERWRITEPROMPT } else { OFN_FILEMUSTEXIST },
        nFileOffset: 0,
        nFileExtension: 0,
        lpstrDefExt: if save { defext.as_ptr() } else { std::ptr::null() },
        lCustData: 0,
        lpfnHook: std::ptr::null(),
        lpTemplateName: std::ptr::null(),
        pvReserved: std::ptr::null(),
        dwReserved: 0,
        FlagsEx: 0,
    };
    // SAFETY: plain Win32 calls; every pointer in `ofn` points at buffers that live until the call returns.
    unsafe {
        CoInitializeEx(std::ptr::null(), 0x2); // COINIT_APARTMENTTHREADED (the Explorer-style dialog uses COM)
        let ok = if save { GetSaveFileNameW(&mut ofn) } else { GetOpenFileNameW(&mut ofn) };
        if ok == 0 {
            let e = CommDlgExtendedError();
            return if e == 0 { Ok(None) } else { Err(format!("The file dialog didn't open (error {e:#x})")) };
        }
    }
    let len = file.iter().position(|&c| c == 0).unwrap_or(file.len());
    Ok(Some(PathBuf::from(std::ffi::OsString::from_wide(&file[..len]))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aimdot-img-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample(w: u32, h: u32) -> image::RgbaImage {
        image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x * 8) as u8, (y * 8) as u8, 200, if (x + y) % 3 == 0 { 0 } else { 255 }]))
    }

    #[test]
    fn every_format_becomes_a_png_the_overlay_reads() {
        let d = scratch("fmt");
        let lib = d.join("images");
        let img = sample(24, 16);
        for (ext, fmt) in [("png", image::ImageFormat::Png), ("jpg", image::ImageFormat::Jpeg), ("bmp", image::ImageFormat::Bmp), ("gif", image::ImageFormat::Gif)] {
            let src = d.join(format!("My pic!.{ext}"));
            if fmt == image::ImageFormat::Jpeg {
                image::DynamicImage::ImageRgba8(img.clone()).to_rgb8().save_with_format(&src, fmt).unwrap();
            } else {
                img.save_with_format(&src, fmt).unwrap();
            }
            let name = import_file(&lib, &src).unwrap();
            assert!(name.ends_with(".png") && name.starts_with("My pic"), "{name}");
            let back = crosshair_core::load_png(&lib.join(&name)).expect(ext);
            assert_eq!((back.w, back.h), (24, 16), "{ext}");
            if ext != "jpg" {
                assert_eq!(back.rgba[3], 0, "{ext}: transparency kept");
                if ext == "gif" {
                    // 256 colours at most: close, not exact
                    assert!(back.rgba[4..7].iter().zip([8u8, 0, 200]).all(|(&a, b)| a.abs_diff(b) <= 24), "gif {:?}", &back.rgba[4..8]);
                } else {
                    assert_eq!(&back.rgba[4..8], &[8, 0, 200, 255], "{ext}");
                }
            }
        }
        // names don't clash
        let names: Vec<String> = std::fs::read_dir(&lib).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).collect();
        assert_eq!(names.len(), 4);
        assert!(names.contains(&"My pic.png".to_string()) && names.contains(&"My pic (4).png".to_string()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bad_files_are_refused() {
        let d = scratch("bad");
        let txt = d.join("notes.txt");
        std::fs::write(&txt, "hello").unwrap();
        assert!(import_file(&d, &txt).is_err());
        let fake = d.join("fake.png");
        std::fs::write(&fake, "not a png at all").unwrap();
        assert!(import_file(&d.join("lib"), &fake).is_err());
        assert!(import_file(&d, &d.join("missing.png")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn big_pictures_are_made_smaller() {
        let d = scratch("big");
        let src = d.join("wide.png");
        image::RgbaImage::from_pixel(3000, 600, image::Rgba([255, 0, 0, 255])).save(&src).unwrap();
        let name = import_file(&d.join("lib"), &src).unwrap();
        let back = crosshair_core::load_png(&d.join("lib").join(name)).unwrap();
        assert_eq!((back.w, back.h), (1024, 205));
        assert_eq!(&back.rgba[..4], &[255, 0, 0, 255]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn painted_picture_round_trip() {
        let mut p = crosshair_core::paint::Painter::new(15);
        p.dab(7, 7, [10, 20, 30, 128], 3);
        let img = p.export(Some([0, 0, 0, 255]));
        let back = cimg::decode_png(&encode_png(&img).unwrap()).unwrap();
        assert_eq!((back.w, back.h), (17, 17));
        assert_eq!(back.rgba, img.rgba);
    }

    // hand-made SVGs like the ones other crosshair apps export
    const CROSS: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="21" height="21" viewBox="0 0 21 21">
        <rect x="10" y="2" width="1" height="7" fill="#00ff00"/><rect x="10" y="12" width="1" height="7" fill="#00ff00"/>
        <rect x="2" y="10" width="7" height="1" fill="#00ff00"/><rect x="12" y="10" width="7" height="1" fill="#00ff00"/></svg>"##;
    const RING: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">
        <circle cx="16" cy="16" r="10" fill="none" stroke="#ffffff" stroke-width="2"/></svg>"##;
    const DOT: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="17px" height="17px">
        <circle cx="8.5" cy="8.5" r="2" fill="red" stroke="black" stroke-width="1"/></svg>"##;

    fn px(img: &Image, x: usize, y: usize) -> [u8; 4] {
        let i = (y * img.w + x) * 4;
        img.rgba[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn a_thin_svg_cross_stays_1_px() {
        let b = CROSS.as_bytes();
        assert_eq!(svg_size(b).unwrap(), (21.0, 21.0));
        assert_eq!(svg_default_side((21.0, 21.0)), 21);
        for sharp in [true, false] {
            let img = render_svg(b, 21, sharp).unwrap();
            assert_eq!((img.w, img.h), (21, 21));
            assert_eq!(px(&img, 10, 3), [0, 255, 0, 255], "sharp {sharp}");
            assert_eq!(px(&img, 9, 3)[3], 0);
            assert_eq!(px(&img, 11, 3)[3], 0);
            assert_eq!(px(&img, 10, 10)[3], 0, "gap in the middle");
            assert_eq!(px(&img, 0, 0)[3], 0, "transparent background");
        }
        // 2x: the line is exactly 2 px, still on whole pixels
        let img = render_svg(b, 42, true).unwrap();
        assert_eq!((img.w, img.h), (42, 42));
        assert_eq!((px(&img, 19, 6)[3], px(&img, 20, 6)[3], px(&img, 21, 6)[3], px(&img, 22, 6)[3]), (0, 255, 255, 0));
        // 1.5x with sharp edges: every pixel fully on or off (no blur)
        let img = render_svg(b, 32, true).unwrap();
        let odd: Vec<(usize, u8)> = img.rgba.chunks(4).enumerate().filter(|(_, p)| p[3] != 0 && p[3] != 255).map(|(i, p)| (i, p[3])).collect();
        assert!(odd.is_empty(), "{odd:?}");
    }

    #[test]
    fn an_svg_ring_is_round_and_centred() {
        let b = RING.as_bytes();
        assert_eq!(svg_size(b).unwrap(), (32.0, 32.0));
        for sharp in [true, false] {
            let img = render_svg(b, 32, sharp).unwrap();
            assert_eq!(px(&img, 16, 16)[3], 0, "hole in the middle");
            assert!(px(&img, 16, 6)[3] > 200 && px(&img, 6, 16)[3] > 200, "the ring");
            // mirror-symmetric around the middle (32 px: between pixels 15 and 16)
            for y in 0..32 {
                for x in 0..32 {
                    let (a, m) = (px(&img, x, y)[3], px(&img, 31 - x, y)[3]);
                    assert!(a.abs_diff(m) <= 2, "{x},{y}: {a} vs {m}");
                }
            }
        }
    }

    #[test]
    fn an_svg_dot_with_outline_keeps_its_centre_pixel() {
        let b = DOT.as_bytes();
        let img = render_svg(b, 17, true).unwrap();
        assert_eq!((img.w, img.h), (17, 17));
        assert_eq!(px(&img, 8, 8), [255, 0, 0, 255], "the dot on the centre pixel");
        assert_eq!(px(&img, 8, 6), [0, 0, 0, 255], "the outline");
        assert_eq!(px(&img, 8, 10), [0, 0, 0, 255]);
        assert_eq!(px(&img, 0, 0)[3], 0);
        // 3x: odd size again, the same centre
        let img = render_svg(b, 51, true).unwrap();
        assert_eq!((img.w, img.h), (51, 51));
        assert_eq!(px(&img, 25, 25), [255, 0, 0, 255]);
    }

    #[test]
    fn svg_sizes_and_import() {
        // big or odd-shaped SVGs: the long side picks the size, at most 128 by default
        assert_eq!(svg_default_side((1000.0, 1000.0)), 128);
        assert_eq!(svg_default_side((2.0, 1.0)), SVG_MIN);
        let wide = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 300 150"><rect width="300" height="150" fill="#08f"/></svg>"##;
        let img = render_svg(wide, svg_default_side(svg_size(wide).unwrap()), true).unwrap();
        assert_eq!((img.w, img.h), (128, 64));
        assert_eq!(px(&img, 127, 63), [0, 136, 255, 255]);
        // a half-transparent line keeps its opacity with sharp edges (and is on / off, no blur)
        let half = br##"<svg xmlns="http://www.w3.org/2000/svg" width="21" height="21"><rect x="10" y="2" width="1" height="17" fill="#fff" fill-opacity="0.5"/></svg>"##;
        let img = render_svg(half, 32, true).unwrap();
        let alphas: std::collections::BTreeSet<u8> = img.rgba.chunks(4).map(|p| p[3]).collect();
        assert_eq!(alphas.into_iter().collect::<Vec<_>>(), vec![0, 128]);
        // a file: into the library as a PNG the overlay reads
        let d = scratch("svg");
        let src = d.join("Thin cross.svg");
        std::fs::write(&src, CROSS).unwrap();
        let name = import_file(&d.join("images"), &src).unwrap();
        assert_eq!(name, "Thin cross.png");
        let back = crosshair_core::load_png(&d.join("images").join(&name)).unwrap();
        assert_eq!((back.w, back.h), (21, 21));
        // broken / empty SVGs are refused
        assert!(svg_size(b"<svg").is_err());
        assert!(render_svg(br#"<svg xmlns="http://www.w3.org/2000/svg" width="9" height="9"></svg>"#, 9, true).is_err());
        std::fs::write(d.join("bad.svg"), "not svg").unwrap();
        assert!(import_file(&d.join("images"), &d.join("bad.svg")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn pasted_paths() {
        assert_eq!(clean_path("  \"/tmp/a b.png\" "), Some(PathBuf::from("/tmp/a b.png")));
        assert_eq!(clean_path("file:///tmp/a%20b.png"), Some(PathBuf::from("/tmp/a b.png")));
        assert_eq!(clean_path("   "), None);
        assert!(listed("x.png") && !listed(".paint-a.png") && !listed("x.jpg"));
        assert_eq!(clean_stem("a/b:c*?"), "abc");
        assert_eq!(clean_stem("///"), "image");
    }
}
