//! Game Bar mode (Windows): hands the crosshair to the Aimdot Game Bar widget (`gamebar-widget/`), which
//! draws it over exclusive-fullscreen DirectX games where a normal window can't show.
//!
//! The channel is two files in the widget package's own data folder,
//! `%LOCALAPPDATA%\Packages\<package family name>\LocalState\` - the simplest thing that works with packaged-app
//! isolation (no AppService, no loopback exemption, no pipe ACLs). The widget watches the folder and reloads.
//!
//! - `crosshair.png`  the crosshair at its exact pixel size (1 image pixel = 1 screen pixel)
//! - `crosshair.ini`  version, seq, visible, offset, rainbow settings - written LAST, so when the widget sees a
//!   new ini, the pictures it points at are already there
//! - rainbow only: `crosshair-base.png` (everything that keeps its colour: outline) + `crosshair-fill.png`
//!   (8-bit coverage of the part that gets the rainbow colour). The widget re-tints from these itself, so there
//!   are no 30 fps file writes.
//!
//! Every file is written to `<name>.tmp` first and then renamed over the old one (same folder), so the widget never
//! reads a half-written file. Only std, no crates; nothing here is Windows-only except where the folder is, so the
//! module type-checks anywhere. `main.rs` includes it with `#[cfg(windows)]`.

use crosshair_core::config::{Profile, Settings};
use crosshair_core::log;
use crosshair_core::crosshair::Renderer;
use crosshair_core::gfx::{Canvas, Color};
use std::io;
use std::path::{Path, PathBuf};

/// The widget package's `Identity Name` (gamebar-widget/Package.appxmanifest). The package family name is this +
/// `_` + a hash of the Publisher (the signing certificate), so it differs between a sideloaded build, a
/// developer build and a Store build - all of them are found by this prefix.
pub const PACKAGE_NAME: &str = "Resister0.AimdotWidget";

/// Bump when the ini format changes in a way an older widget would get wrong.
pub const FORMAT: u32 = 1;

/// Every installed widget package's `LocalState` folder (usually one). Empty = the widget isn't installed, or it
/// was installed but never opened (Windows makes the folder when the package is registered; we never create it).
pub fn folders() -> Vec<PathBuf> {
    let Some(base) = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) else { return Vec::new() };
    let pkgs = PathBuf::from(base).join("Packages");
    let prefix = format!("{}_", PACKAGE_NAME.to_ascii_lowercase());
    let mut v: Vec<PathBuf> = std::fs::read_dir(&pkgs)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_name().to_str().is_some_and(|n| n.to_ascii_lowercase().starts_with(&prefix)))
                .map(|e| e.path().join("LocalState"))
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// For the settings window's `info` message (`gamebar=`): "installed" or "missing".
pub fn status() -> &'static str {
    if folders().is_empty() {
        "missing"
    } else {
        "installed"
    }
}

/// What `GameBar::update` did.
#[derive(Debug, PartialEq)]
pub enum Wrote {
    /// New files are in place.
    Yes,
    /// Same picture and settings as last time: nothing written.
    Same,
    /// The widget isn't installed (no LocalState folder).
    NoWidget,
}

pub struct GameBar {
    /// Starts at the clock time, so a restarted overlay never repeats a number the widget has already seen.
    seq: u64,
    /// Hash of the last files written (pictures + ini without `seq`).
    last: Option<u64>,
    /// Bumped by `recenter()`: the widget asks Game Bar to centre its window when this changes.
    recenter: u32,
    canvas: Canvas,
    black: Canvas,
    white: Canvas,
    /// Write lines at most every 2 s (a slider drag writes per change; the newest one always gets logged).
    log_writes: log::Throttle,
    /// Errors / "no widget" once until they change (the 1 s tick retries).
    log_err: log::Quiet,
    /// Copies the widget's own LocalState\widget.log into aimdot.log as [widget/...].
    widget_log: log::Follow,
}

impl Default for GameBar {
    fn default() -> Self {
        Self::new()
    }
}

/// "Resister0.AimdotWidget_<hash>\LocalState": which copy of the widget (dev / signed / Store), no home path.
fn short(dir: &Path) -> String {
    let pkg = dir.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    format!("{pkg}\\LocalState")
}

impl GameBar {
    /// From the 1 s tick: writes a held-back write line, and in Game Bar mode copies the widget's new log lines
    /// (`LocalState\widget.log`, first time: its last 20) into aimdot.log as `[widget/...]`. Read-only.
    pub fn log_tick(&mut self, game_bar_mode: bool) {
        self.log_writes.tick();
        if game_bar_mode {
            if let Some(dir) = folders().into_iter().next() {
                let lines = self.widget_log.read(&dir.join(log::WIDGET_FILE), 20);
                log::copy_in("widget", &lines);
            }
        }
    }

    pub fn new() -> GameBar {
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(1);
        GameBar {
            seq: ms,
            last: None,
            recenter: 0,
            canvas: Canvas::new(0, 0),
            black: Canvas::new(0, 0),
            white: Canvas::new(0, 0),
            log_writes: log::Throttle::new(std::time::Duration::from_secs(2)),
            log_err: log::Quiet::new(),
            widget_log: log::Follow::new(),
        }
    }

    /// Ask the widget to centre its window on the screen again (e.g. a "Centre the widget" button). Not used yet:
    /// the widget already centres itself when it opens.
    #[allow(dead_code)]
    pub fn recenter(&mut self) {
        self.recenter = self.recenter.wrapping_add(1);
        self.last = None;
    }

    /// Call whenever the crosshair, its offset or whether it's shown changed while Game Bar mode is on, AND from the
    /// overlay's 1 s housekeeping tick (that picks up a widget installed / reinstalled after the last change) - and
    /// once with `visible = false` when Game Bar mode is switched off or the overlay quits, so the widget goes blank.
    /// Cheap when nothing changed (one folder listing, one render, a hash). Errors are file errors (e.g. the widget
    /// was uninstalled a moment ago); ignore them, the next call tries again.
    /// Hide-while-aiming isn't passed on (a file + Game Bar's reaction are too slow for that): `visible` is only the
    /// show / hide toggle.
    pub fn update(&mut self, r: &mut Renderer, p: &Profile, s: &Settings, visible: bool) -> io::Result<Wrote> {
        let dirs = folders();
        if dirs.is_empty() {
            self.last = None;
            self.log_err.line("gamebar", "no widget folder (Packages\\Resister0.AimdotWidget_*\\LocalState) - widget not installed, nothing written");
            return Ok(Wrote::NoWidget);
        }
        let files = self.render(r, p, s, visible);
        let body = ini_body(p, s, visible, self.recenter, &self.canvas, files.len() > 1);
        let mut h = Fnv::new();
        h.add(body.as_bytes());
        for (name, data) in &files {
            h.add(name.as_bytes());
            h.add(data);
        }
        let key = h.0;
        // A reinstalled widget starts with an empty folder: write again even if nothing changed here.
        if self.last == Some(key) && dirs.iter().all(|d| d.join("crosshair.ini").is_file()) {
            return Ok(Wrote::Same);
        }
        self.seq += 1;
        let ini = format!(
            "# Written by Aimdot (Game Bar mode). The Aimdot Game Bar widget reads this file.\nformat={FORMAT}\nseq={}\n{body}",
            self.seq
        );
        let mut res = Ok(Wrote::Yes);
        for dir in &dirs {
            match write_set(dir, &files, &ini) {
                Ok(()) => {
                    self.log_err.reset();
                    let pics: String = files.iter().map(|(n, d)| format!("{n} {} + ", log::size(d.len() as u64))).collect();
                    self.log_writes.line(
                        "gamebar",
                        &format!("wrote {pics}crosshair.ini (seq {}, visible {}, {}x{}) to {}", self.seq, visible as u8, self.canvas.w, self.canvas.h, short(dir)),
                    );
                }
                Err(e) => {
                    self.log_err.line("gamebar", &format!("can't write to {}: {e}", short(dir)));
                    res = Err(e);
                }
            }
        }
        if res.is_ok() {
            self.last = Some(key);
        }
        res
    }

    /// The picture files for this state: `crosshair.png`, plus the two rainbow layers when the colour animates.
    fn render(&mut self, r: &mut Renderer, p: &Profile, _s: &Settings, visible: bool) -> Vec<(&'static str, Vec<u8>)> {
        let mut out = Vec::new();
        if !visible {
            // Keep the last picture; the ini says visible=0. (Nothing to render.)
            self.canvas.resize(0, 0);
            return out;
        }
        r.render(p, 0.0, &mut self.canvas);
        out.push(("crosshair.png", png_from_canvas(&self.canvas)));
        if p.rainbow_animated() { // RGB rainbow only: a painted frame animation shows its first frame here
            // Split the picture into "base" (keeps its colour) + "fill" (gets the rainbow colour). Every pixel the
            // engine makes is  base + fill * tint  (premultiplied), so rendering it once with a black and once with a
            // white solid colour gives both: fill = white - black, base = black with fill's alpha taken out.
            let mut q = p.clone();
            q.rainbow = false;
            q.color = Color::BLACK;
            r.render(&q, 0.0, &mut self.black);
            q.color = Color::WHITE;
            r.render(&q, 0.0, &mut self.white);
            if (self.black.w, self.black.h) == (self.white.w, self.white.h) {
                let (base, fill) = split_layers(&self.black, &self.white);
                out.push(("crosshair-base.png", png_rgba(self.black.w as u32, self.black.h as u32, &base)));
                out.push(("crosshair-fill.png", png_gray(self.black.w as u32, self.black.h as u32, &fill)));
            }
        }
        out
    }
}

/// `crosshair.ini` without its first lines (comment, format, seq - `seq` changes every time, so the change check
/// in `update` hashes only this part + the pictures). Plain `key=value` lines like the app's own config; the
/// widget ignores keys it doesn't know.
fn ini_body(p: &Profile, s: &Settings, visible: bool, recenter: u32, c: &Canvas, layers: bool) -> String {
    let b = |v: bool| if v { 1 } else { 0 };
    format!(
        "visible={}\nwidth={}\nheight={}\noffset_x={}\noffset_y={}\nmonitor={}\n\
         recenter={recenter}\nrainbow={}\nrainbow_mode={}\nrainbow_speed={}\nrainbow_sat={}\n",
        b(visible && c.w > 0),
        c.w,
        c.h,
        s.offset_x as i32,
        s.offset_y as i32,
        s.monitor,
        b(layers),
        p.rainbow_mode,
        p.rainbow_speed,
        p.rainbow_sat,
    )
}

/// Pictures first, the ini last (the widget reacts to the ini).
fn write_set(dir: &Path, files: &[(&str, Vec<u8>)], ini: &str) -> io::Result<()> {
    for (name, data) in files {
        write_replace(&dir.join(name), data)?;
    }
    write_replace(&dir.join("crosshair.ini"), ini.as_bytes())
}

/// Write `<path>.tmp`, then rename it over `path` (std's rename replaces an existing file on Windows). The widget
/// reads a file in one go, but if it has the old one open at that very moment the rename fails: try a few times.
fn write_replace(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, data)?;
    let mut tries = 0;
    loop {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(_) if tries < 5 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        }
    }
}

/// From the black- and white-coloured renders: base (straight RGBA) + fill coverage (0..255).
fn split_layers(black: &Canvas, white: &Canvas) -> (Vec<u8>, Vec<u8>) {
    let n = black.px.len();
    let mut base = Vec::with_capacity(n * 4);
    let mut fill = Vec::with_capacity(n);
    for (&k, &w) in black.px.iter().zip(&white.px) {
        let ch = |v: u32, sh: u32| (v >> sh) & 255;
        // white - black on any colour channel = fill alpha (premultiplied white = alpha); use the largest for safety
        let f = ch(w, 16).saturating_sub(ch(k, 16)).max(ch(w, 8).saturating_sub(ch(k, 8))).max(ch(w, 0).saturating_sub(ch(k, 0)));
        let ba = ch(k, 24).saturating_sub(f);
        let pm = (ba << 24) | (ch(k, 16).min(ba) << 16) | (ch(k, 8).min(ba) << 8) | ch(k, 0).min(ba);
        base.extend_from_slice(&unpremultiply(pm));
        fill.push(f as u8);
    }
    (base, fill)
}

/// 0xAARRGGBB premultiplied -> [r, g, b, a] straight (what PNG stores).
fn unpremultiply(v: u32) -> [u8; 4] {
    let a = v >> 24;
    if a == 0 {
        return [0, 0, 0, 0];
    }
    let c = |sh: u32| ((((v >> sh) & 255) * 255 + a / 2) / a).min(255) as u8;
    [c(16), c(8), c(0), a as u8]
}

fn png_from_canvas(c: &Canvas) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(c.px.len() * 4);
    for &v in &c.px {
        rgba.extend_from_slice(&unpremultiply(v));
    }
    png_rgba(c.w as u32, c.h as u32, &rgba)
}

// ---- A tiny PNG writer: stored (uncompressed) deflate blocks. The pictures are small (a crosshair is a few KB to a
// few hundred KB raw), so compression would only cost CPU; the widget's decoder doesn't care.

fn png_rgba(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    png(w, h, 6, 4, rgba)
}

fn png_gray(w: u32, h: u32, gray: &[u8]) -> Vec<u8> {
    png(w, h, 0, 1, gray)
}

/// `color_type` 6 = RGBA, 0 = grayscale; 8 bits per channel; `bpp` bytes per pixel.
fn png(w: u32, h: u32, color_type: u8, bpp: usize, data: &[u8]) -> Vec<u8> {
    let (w, h) = (w.max(1), h.max(1));
    let stride = w as usize * bpp;
    // Each row: filter byte 0 (None) + the row. A 0x0 canvas becomes one transparent pixel.
    let mut raw = Vec::with_capacity((stride + 1) * h as usize);
    for y in 0..h as usize {
        raw.push(0);
        let row = data.get(y * stride..(y + 1) * stride);
        match row {
            Some(r) => raw.extend_from_slice(r),
            None => raw.resize(raw.len() + stride, 0),
        }
    }
    let mut z = Vec::with_capacity(raw.len() + raw.len() / 65535 * 5 + 16);
    z.extend_from_slice(&[0x78, 0x01]); // zlib header: deflate, 32K window, no preset dictionary
    let mut chunks = raw.chunks(65535).peekable();
    if chunks.peek().is_none() {
        z.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(ch) = chunks.next() {
        let last = chunks.peek().is_none();
        let len = ch.len() as u16;
        z.push(last as u8); // BFINAL + BTYPE=00 (stored)
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(ch);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut out = Vec::with_capacity(z.len() + 64);
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, color_type, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut c = Crc::new();
    c.add(kind);
    c.add(data);
    out.extend_from_slice(&c.done().to_be_bytes());
}

struct Crc(u32);
impl Crc {
    fn new() -> Crc {
        Crc(0xFFFF_FFFF)
    }
    fn add(&mut self, d: &[u8]) {
        for &b in d {
            let mut c = (self.0 ^ b as u32) & 0xFF;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            self.0 = c ^ (self.0 >> 8);
        }
    }
    fn done(&self) -> u32 {
        self.0 ^ 0xFFFF_FFFF
    }
}

fn adler32(d: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for ch in d.chunks(5552) {
        for &x in ch {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

struct Fnv(u64);
impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn add(&mut self, d: &[u8]) {
        for &b in d {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crosshair_core::image::decode_png;

    #[test]
    fn png_round_trip() {
        let (w, h) = (300u32, 250u32); // > 65535 raw bytes: several stored blocks
        let rgba: Vec<u8> = (0..w * h * 4).map(|i| (i * 7 % 251) as u8).collect();
        let img = decode_png(&png_rgba(w, h, &rgba)).expect("decodes");
        assert_eq!((img.w, img.h), (w as usize, h as usize));
        let gray: Vec<u8> = (0..w * h).map(|i| (i % 256) as u8).collect();
        assert!(decode_png(&png_gray(w, h, &gray)).is_ok());
        assert!(decode_png(&png_rgba(0, 0, &[])).is_ok());
    }

    #[test]
    fn layers_add_up() {
        // base + fill * tint must give back the render with that solid colour (within rounding).
        let mut r = Renderer::new(PathBuf::from("."), |_| None);
        let p = Profile { rainbow: true, outline: true, dot: true, circle: true, ..Profile::default() };
        let (mut k, mut wh, mut red) = (Canvas::new(0, 0), Canvas::new(0, 0), Canvas::new(0, 0));
        let mut q = p.clone();
        q.rainbow = false;
        q.color = Color::BLACK;
        r.render(&q, 0.0, &mut k);
        q.color = Color::WHITE;
        r.render(&q, 0.0, &mut wh);
        q.color = Color::hex(0xFF0000);
        r.render(&q, 0.0, &mut red);
        let (base, fill) = split_layers(&k, &wh);
        for i in 0..red.px.len() {
            let a = base[i * 4 + 3] as i32;
            let f = fill[i] as i32;
            let want_a = (red.px[i] >> 24) as i32;
            let want_r = ((red.px[i] >> 16) & 255) as i32;
            let got_r = base[i * 4] as i32 * a / 255 + f;
            assert!((a + f - want_a).abs() <= 2, "alpha at {i}: {} vs {}", a + f, want_a);
            assert!((got_r - want_r).abs() <= 3, "red at {i}: {got_r} vs {want_r}");
        }
    }
}
