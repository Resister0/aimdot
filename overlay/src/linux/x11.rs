//! X11 fallback (X11 desktops, and GNOME Wayland where there is no layer-shell - games there mostly run
//! through XWayland / Proton, where an X11 window can sit on top): one small override-redirect ARGB window,
//! empty XShape input region (click-through), raised on every update. Needs a compositor for transparency
//! (every modern desktop has one). ⚠ Not tested yet.

use crosshair_core::gfx::Canvas;
use std::os::fd::{AsFd, BorrowedFd};
use x11rb::connection::Connection;
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{self, ConnectionExt as _};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use crosshair_core::config::{Hotkey, MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_WIN};

const LOCK: u16 = 2; // Caps Lock
const NUMLOCK: u16 = 16; // Mod2 on practically every layout

/// Aimdot's (Windows) modifier flags -> X modifier mask (Shift, Control, Mod1 = Alt, Mod4 = Super).
fn x_mods(m: u32) -> u16 {
    [(MOD_SHIFT, 1u16), (MOD_CTRL, 4), (MOD_ALT, 8), (MOD_WIN, 64)].iter().filter(|(w, _)| m & w != 0).map(|(_, x)| x).sum()
}

/// Windows virtual-key code -> X keysym.
fn keysym(vk: u32) -> Option<u32> {
    Some(match vk {
        0x41..=0x5A => vk + 0x20, // a..z
        0x30..=0x39 => vk,
        0x70..=0x87 => 0xFFBE + vk - 0x70, // F1..F24
        0x60..=0x69 => 0xFFB0 + vk - 0x60, // KP_0..KP_9
        0x08 => 0xFF08,
        0x09 => 0xFF09,
        0x0D => 0xFF0D,
        0x13 => 0xFF13,
        0x1B => 0xFF1B,
        0x20 => 0x20,
        0x21 => 0xFF55,
        0x22 => 0xFF56,
        0x23 => 0xFF57,
        0x24 => 0xFF50,
        0x25 => 0xFF51,
        0x26 => 0xFF52,
        0x27 => 0xFF53,
        0x28 => 0xFF54,
        0x2C => 0xFF61,
        0x2D => 0xFF63,
        0x2E => 0xFFFF,
        0x6A => 0xFFAA,
        0x6B => 0xFFAB,
        0x6D => 0xFFAD,
        0x6E => 0xFFAE,
        0x6F => 0xFFAF,
        0x91 => 0xFF14,
        0xBA => 0x3B,
        0xBB => 0x3D,
        0xBC => 0x2C,
        0xBD => 0x2D,
        0xBE => 0x2E,
        0xBF => 0x2F,
        0xC0 => 0x60,
        0xDB => 0x5B,
        0xDC => 0x5C,
        0xDD => 0x5D,
        0xDE => 0x27,
        _ => return None,
    })
}

/// (keycode, keysym) for the unshifted keysym of every keycode.
fn keyboard_map(conn: &RustConnection) -> Vec<(u8, u32)> {
    let s = conn.setup();
    let (min, max) = (s.min_keycode, s.max_keycode);
    let Ok(Ok(r)) = conn.get_keyboard_mapping(min, max - min + 1).map(|c| c.reply()) else { return Vec::new() };
    let per = r.keysyms_per_keycode.max(1) as usize;
    r.keysyms.chunks(per).enumerate().filter_map(|(i, ks)| ks.first().filter(|&&k| k != 0).map(|&k| (min + i as u8, k))).collect()
}

struct Mon {
    name: String,
    primary: bool,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

pub struct X11 {
    conn: RustConnection,
    root: u32,
    win: u32,
    gc: u32,
    mons: Vec<Mon>,
    mapped: bool,
    geom: (i32, i32, i32, i32),
    changed: bool,
    last: Option<(Vec<u32>, i32, i32)>,
    /// Hotkey grabs (only without a shortcuts portal): (keycode, X modifier mask) per hotkey 1..8.
    grabs: Vec<Option<(u8, u16)>>,
    pressed: Vec<u32>,
    /// Last key release (keycode, time): X auto-repeat = a release + press with the same time.
    released: Option<(u8, u32)>,
    /// Show / hide "Instant": hidden = a transparent image in the mapped window (KWin fades unmanaged X11 windows
    /// that are mapped / unmapped).
    keep_mapped: bool,
    blank: bool,
}

impl X11 {
    pub fn connect() -> Result<X11, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| format!("no X11 display ({e})"))?;
        let scr = &conn.setup().roots[screen];
        let root = scr.root;
        let visual = scr
            .allowed_depths
            .iter()
            .filter(|d| d.depth == 32)
            .flat_map(|d| d.visuals.iter())
            .find(|v| v.class == xproto::VisualClass::TRUE_COLOR)
            .map(|v| v.visual_id)
            .ok_or("no 32-bit visual (transparency)")?;
        let e = |e: x11rb::errors::ConnectionError| e.to_string();
        let cmap = conn.generate_id().map_err(|e| e.to_string())?;
        conn.create_colormap(xproto::ColormapAlloc::NONE, cmap, root, visual).map_err(e)?;
        let win = conn.generate_id().map_err(|e| e.to_string())?;
        let aux = xproto::CreateWindowAux::new()
            .override_redirect(1)
            .background_pixel(0)
            .border_pixel(0)
            .colormap(cmap)
            .event_mask(xproto::EventMask::EXPOSURE);
        conn.create_window(32, win, root, 0, 0, 8, 8, 0, xproto::WindowClass::INPUT_OUTPUT, visual, &aux).map_err(e)?;
        // Click-through: empty input shape.
        conn.shape_rectangles(shape::SO::SET, shape::SK::INPUT, xproto::ClipOrdering::UNSORTED, win, 0, 0, &[]).map_err(e)?;
        let name = b"Aimdot";
        conn.change_property8(xproto::PropMode::REPLACE, win, xproto::AtomEnum::WM_NAME, xproto::AtomEnum::STRING, name).map_err(e)?;
        let gc = conn.generate_id().map_err(|e| e.to_string())?;
        conn.create_gc(gc, win, &xproto::CreateGCAux::new()).map_err(e)?;
        let _ = conn.randr_select_input(root, randr::NotifyMask::SCREEN_CHANGE);
        let mut x = X11 { conn, root, win, gc, mons: Vec::new(), mapped: false, geom: (0, 0, 0, 0), changed: true, last: None, grabs: Vec::new(), pressed: Vec::new(), released: None, keep_mapped: true, blank: false };
        x.read_monitors();
        x.conn.flush().map_err(e)?;
        Ok(x)
    }

    fn read_monitors(&mut self) {
        let mut v = Vec::new();
        if let Ok(Ok(r)) = self.conn.randr_get_monitors(self.root, true).map(|c| c.reply()) {
            for m in r.monitors {
                let name = self.conn.get_atom_name(m.name).ok().and_then(|c| c.reply().ok()).map(|a| String::from_utf8_lossy(&a.name).to_string()).unwrap_or_default();
                v.push(Mon { name, primary: m.primary, x: m.x as i32, y: m.y as i32, w: m.width as i32, h: m.height as i32 });
            }
        }
        if v.is_empty() {
            let s = &self.conn.setup().roots[0];
            v.push(Mon { name: "Screen".into(), primary: true, x: 0, y: 0, w: s.width_in_pixels as i32, h: s.height_in_pixels as i32 });
        }
        // Primary first, then left to right (like the Windows overlay).
        v.sort_by_key(|m| (!m.primary, m.x, m.y));
        self.mons = v;
        self.changed = true;
    }

    fn draw(&self) {
        if let Some((px, w, h)) = &self.last {
            let mut data = Vec::with_capacity(px.len() * 4);
            for p in px {
                data.extend_from_slice(&p.to_le_bytes()); // B G R A = premultiplied ARGB in a 32-bit ZPixmap
            }
            let _ = self.conn.put_image(xproto::ImageFormat::Z_PIXMAP, self.win, self.gc, *w as u16, *h as u16, 0, 0, 0, 32, &data);
        }
    }
}

impl super::Display for X11 {
    fn pump(&mut self, wait: &mut dyn FnMut(Option<BorrowedFd<'_>>) -> bool) {
        let _ = self.conn.flush();
        let mut any = false;
        loop {
            match self.conn.poll_for_event() {
                Ok(Some(ev)) => {
                    any = true;
                    match ev {
                        Event::Expose(_) if !self.blank => self.draw(),
                        Event::RandrScreenChangeNotify(_) => self.read_monitors(),
                        Event::KeyPress(k) => {
                            // Lock / NumLock don't matter (grabbed with every combination of them).
                            let m = u16::from(k.state) & !(LOCK | NUMLOCK);
                            let repeat = self.released == Some((k.detail, k.time));
                            if let Some(i) = self.grabs.iter().position(|g| *g == Some((k.detail, m))) {
                                // Only the nudge keys (5..8) repeat while held.
                                if !repeat || i >= 4 {
                                    self.pressed.push(i as u32 + 1);
                                }
                            }
                        }
                        Event::KeyRelease(k) => self.released = Some((k.detail, k.time)),
                        _ => {}
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    eprintln!("aimdot: X11 connection lost: {e}");
                    std::process::exit(1);
                }
            }
        }
        if !any {
            let fd = self.conn.stream().as_fd();
            wait(Some(fd));
        } else {
            wait(None);
        }
        let _ = self.conn.flush();
    }
    fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }
    fn monitor_height(&self, monitor: u32) -> Option<i32> {
        let monitor = if monitor == super::FOLLOW { 0 } else { monitor }; // following the game: not on X11 (yet)
        self.mons.get((monitor as usize).min(self.mons.len().max(1) - 1)).map(|m| m.h)
    }
    fn monitors(&self) -> Vec<String> {
        self.mons.iter().map(|m| format!("{} ({}x{})", m.name, m.w, m.h)).collect()
    }
    fn present(&mut self, pic: &Canvas, visible: bool, monitor: u32, off: (i32, i32), new_pic: bool) {
        let monitor = if monitor == super::FOLLOW { 0 } else { monitor }; // following the game: not on X11 (yet)
        if !visible && self.keep_mapped && self.mapped {
            if !self.blank {
                let (w, h) = (self.geom.2.max(1), self.geom.3.max(1));
                let zero = vec![0u8; (w * h * 4) as usize];
                let _ = self.conn.put_image(xproto::ImageFormat::Z_PIXMAP, self.win, self.gc, w as u16, h as u16, 0, 0, 0, 32, &zero);
                self.blank = true;
            }
            let _ = self.conn.flush();
            return;
        }
        if !visible {
            if self.mapped {
                let _ = self.conn.unmap_window(self.win);
                self.mapped = false;
            }
            let _ = self.conn.flush();
            return;
        }
        let m = &self.mons[(monitor as usize).min(self.mons.len() - 1)];
        let (w, h) = (pic.w.max(1), pic.h.max(1));
        let g = (m.x + m.w / 2 + off.0 - w / 2, m.y + m.h / 2 + off.1 - h / 2, w, h);
        if g != self.geom {
            let aux = xproto::ConfigureWindowAux::new().x(g.0).y(g.1).width(w as u32).height(h as u32);
            let _ = self.conn.configure_window(self.win, &aux);
            self.geom = g;
        }
        if new_pic || self.last.is_none() {
            self.last = Some((pic.px.clone(), pic.w, pic.h));
        }
        if !self.mapped {
            let _ = self.conn.map_window(self.win);
            self.mapped = true;
        }
        let _ = self.conn.configure_window(self.win, &xproto::ConfigureWindowAux::new().stack_mode(xproto::StackMode::ABOVE));
        self.blank = false;
        self.draw();
        let _ = self.conn.flush();
    }
    fn name(&self) -> &'static str {
        "x11"
    }
    fn set_keep_mapped(&mut self, on: bool) {
        self.keep_mapped = on;
    }
    fn grab_keys(&mut self, keys: &[Hotkey]) -> Option<Vec<bool>> {
        let root = self.root;
        for (code, m) in self.grabs.drain(..).flatten() {
            for extra in [0, LOCK, NUMLOCK, LOCK | NUMLOCK] {
                let _ = self.conn.ungrab_key(code, root, xproto::ModMask::from(m | extra));
            }
        }
        let map = keyboard_map(&self.conn);
        let mut ok = Vec::new();
        for h in keys {
            let g = keysym(h.vk).and_then(|ks| map.iter().find(|&&(_, s)| s == ks).map(|&(c, _)| (c, x_mods(h.mods))));
            let mut fine = h.vk == 0 || g.is_some();
            if let Some((code, m)) = g {
                for extra in [0, LOCK, NUMLOCK, LOCK | NUMLOCK] {
                    let r = self.conn.grab_key(true, root, xproto::ModMask::from(m | extra), code, xproto::GrabMode::ASYNC, xproto::GrabMode::ASYNC);
                    // BadAccess = another app holds this key already.
                    fine &= r.map_or(false, |c| c.check().is_ok());
                }
            }
            self.grabs.push(g.filter(|_| fine));
            ok.push(fine);
        }
        let _ = self.conn.flush();
        Some(ok)
    }
    fn take_hotkeys(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.pressed)
    }
}
