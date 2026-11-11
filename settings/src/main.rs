//! Aimdot settings window (Slint, Echo's look). A separate program: it starts when you open the settings and
//! is gone - memory and all - when you close it. It talks to the running overlay over a local socket
//! (crosshair-core's ipc.rs): every change is sent as the whole state, so the crosshair on screen follows the
//! sliders live. The overlay owns config.ini; only when no overlay runs does this window save the file itself.
#![cfg_attr(windows, windows_subsystem = "windows")]

slint::include_modules!();

mod autostart;
mod images;
mod painter;
mod profiles;
mod range;
mod theme;
mod titlebar;
mod update;

use crosshair_core::config::Profile;
use crosshair_core::gfx::{Canvas, Color};
use crosshair_core::{crosshair, ipc, model::Model, paths};
use slint::{ComponentHandle, Model as _, ModelRc, SharedPixelBuffer, SharedString, VecModel};
use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;
use std::time::{Duration, Instant};

#[cfg(not(windows))]
type Stream = std::os::unix::net::UnixStream;
#[cfg(windows)]
type Stream = std::net::TcpStream;

fn connect() -> Option<Stream> {
    #[cfg(not(windows))]
    {
        Stream::connect(paths::socket_path()).ok()
    }
    #[cfg(windows)]
    {
        let text = std::fs::read_to_string(paths::port_file()).ok()?;
        let (port, token) = text.trim().split_once(' ')?;
        let mut s = Stream::connect(("127.0.0.1", port.parse::<u16>().ok()?)).ok()?;
        let _ = s.set_nodelay(true);
        ipc::write_msg(&mut s, "hello", token).ok()?;
        Some(s)
    }
}

struct App {
    m: Model,
    ui: slint::Weak<AppWindow>,
    conn: Option<Stream>,
    /// Picker state (kept as given, so the hue doesn't jump when saturation is 0).
    hsv: (f32, f32, f32),
    presets: Rc<VecModel<Preset>>,
    preset_key: u64,
    save_timer: slint::Timer,
    anim: slint::Timer,
    /// RGB theme colours: the 33 ms step timer (only while the custom theme has an RGB colour) and the hue turned so far
    rgb: slint::Timer,
    hue: f32,
    /// the theme last put into the window (theme id + custom colours): only re-applied when it changes
    look_key: String,
    /// "Start with my PC" as last applied (Linux: the autostart entry follows the config key)
    autostart: Option<bool>,
    /// the preview's slow pan (on / where it is, px) and colour-blind view (0 off, 1 deutan, 2 protan, 3 tritan) - not saved
    pan_on: bool,
    pan: f32,
    cvd: u32,
    /// the built-in backgrounds' thumbnails (made once) + your own picture, scaled once (file name, picture)
    bg_thumbs: Vec<Backdrop>,
    /// Linux + KDE shortcuts portal: the keys KDE really uses (toggle, next, prev, settings; "" = none)
    hk_kde: Option<Vec<String>>,
    /// the "More presets" gallery: every preset (made once), the search, what the tiles show now (to skip re-drawing)
    more: Vec<crosshair_core::presets_more::More>,
    gal_q: String,
    gal_key: u64,
    own_bg: Option<(String, Canvas)>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app(f: impl FnOnce(&mut App)) {
    APP.with(|a| {
        if let Some(app) = a.borrow_mut().as_mut() {
            f(app)
        }
    });
}

fn to_image(c: &Canvas) -> slint::Image {
    let (w, h) = (c.w.max(1) as u32, c.h.max(1) as u32);
    let mut buf = SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
    if c.w > 0 && c.h > 0 {
        for (d, &s) in buf.make_mut_slice().iter_mut().zip(&c.px) {
            *d = slint::Rgba8Pixel { r: (s >> 16) as u8, g: (s >> 8) as u8, b: s as u8, a: (s >> 24) as u8 };
        }
    }
    slint::Image::from_rgba8_premultiplied(buf)
}

fn col(c: Color) -> slint::Color {
    slint::Color::from_rgb_u8(c.r, c.g, c.b)
}

fn prof(p: &Profile) -> Prof {
    Prof {
        color: col(p.color),
        opacity: p.opacity,
        rainbow: p.rainbow,
        rainbow_mode: p.rainbow_mode as i32,
        rainbow_speed: p.rainbow_speed,
        rainbow_sat: p.rainbow_sat,
        outline: p.outline,
        outline_thick: p.outline_thick,
        outline_color: col(p.outline_color),
        outline_opacity: p.outline_opacity,
        dot: p.dot,
        dot_size: p.dot_size,
        dot_round: p.dot_round,
        dot_opacity: p.dot_opacity,
        inner: p.inner,
        inner_len: p.inner_len,
        inner_thick: p.inner_thick,
        inner_gap: p.inner_gap,
        inner_opacity: p.inner_opacity,
        outer: p.outer,
        outer_len: p.outer_len,
        outer_thick: p.outer_thick,
        outer_gap: p.outer_gap,
        outer_opacity: p.outer_opacity,
        circle: p.circle,
        circle_radius: p.circle_radius,
        circle_thick: p.circle_thick,
        circle_opacity: p.circle_opacity,
        t_style: p.t_style,
        rotation: p.rotation,
        scale: p.scale,
        is_image: p.mode == 1,
    }
}

/// Number as INI text: whole numbers without ".0" (u32 settings parse only those).
fn num_text(v: f32) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

impl App {
    fn ui(&self) -> Option<AppWindow> {
        self.ui.upgrade()
    }

    /// Pushes everything into the window.
    fn refresh(&mut self) {
        let Some(ui) = self.ui() else { return };
        let s = ui.global::<S>();
        s.set_p(prof(&self.m.cur));
        let st = &self.m.set;
        s.set_overlay_on(st.overlay_on);
        s.set_monitor(st.monitor as i32);
        s.set_offset_x(st.offset_x);
        s.set_offset_y(st.offset_y);
        s.set_aim_mode(st.aim_mode as i32);
        let names: Vec<SharedString> = [st.hk_toggle, st.hk_next, st.hk_prev, st.hk_editor, st.hk_nudge_up, st.hk_nudge_down, st.hk_nudge_left, st.hk_nudge_right, st.hk_zoom]
            .iter()
            .map(|h| h.name().into())
            .collect();
        s.set_hk_names(ModelRc::new(VecModel::from(names)));
        s.set_display_mode(st.display_mode as i32);
        s.set_eco(st.eco);
        s.set_size_mode(st.size_mode as i32);
        s.set_monitor_follow(st.monitor_follow);
        s.set_hide_cursor(st.hide_cursor);
        s.set_show_fade(st.show_fade);
        s.set_smart_outline(st.smart_outline);
        s.set_zoom_level((st.zoom_level.round() as i32).clamp(2, 4));
        s.set_zoom_scroll(st.zoom_scroll);
        s.set_zoom_ring(col(st.zoom_ring));
        s.set_preview_bg(st.preview_bg as i32);
        s.set_preview_zoom([1, 2, 4][st.preview_zoom.min(2) as usize]);
        s.set_preview_img(st.preview_img.as_str().into());
        // the top bar's hotkey chip: "Ctrl + Alt + X" -> [Ctrl, Alt, X]
        // (KDE owns the keys: the key KDE really uses, from the overlay's hk_kde)
        let keys: Vec<SharedString> = match &self.hk_kde {
            Some(k) => k[0].split('+').map(|p| p.trim()).filter(|p| !p.is_empty()).map(SharedString::from).collect(),
            None if st.hk_toggle.vk == 0 => Vec::new(),
            None => st.hk_toggle.name().split(" + ").map(SharedString::from).collect(),
        };
        s.set_hk_keys(ModelRc::new(VecModel::from(keys)));
        self.look(&ui);
        let on = self.m.set.autostart;
        s.set_autostart(on);
        s.set_start_minimized(self.m.set.start_minimized);
        if self.autostart != Some(on) {
            self.autostart = Some(on);
            s.set_autostart_note(autostart::apply(on).into());
        }
        self.preview();
        self.refresh_presets();
        self.refresh_gallery();
        self.animate();
        images::refresh(self);
        profiles::refresh(self);
    }

    /// A hotkey row got a new key (vk 0 = none): store it, stop capturing, tell the overlay
    fn set_hotkey(&mut self, n: i32, hk: crosshair_core::config::Hotkey) {
        let set = &mut self.m.set;
        match n {
            0 => set.hk_toggle = hk,
            1 => set.hk_next = hk,
            2 => set.hk_prev = hk,
            4 => set.hk_nudge_up = hk,
            5 => set.hk_nudge_down = hk,
            6 => set.hk_nudge_left = hk,
            7 => set.hk_nudge_right = hk,
            8 => set.hk_zoom = hk,
            _ => set.hk_editor = hk,
        }
        if let Some(ui) = self.ui() {
            ui.global::<S>().set_hk_capture(-1);
        }
        if let Some(c) = &mut self.conn {
            let _ = ipc::write_msg(c, "capture", "0");
        }
        self.changed();
    }

    /// Echo's themes: palette into T + the theme cards (only when the theme or its colours changed)
    fn look(&mut self, ui: &AppWindow) {
        let st = &mut self.m.set;
        if !theme::known(&st.ui_theme) {
            st.ui_theme.clear();
        }
        let (id, acc, bg) = (st.ui_theme.clone(), st.ui_accent.to_hex(), st.ui_bg.to_hex());
        let rgb = (st.ui_accent_rgb, st.ui_bg_rgb);
        let key = format!("{id}|{acc}|{bg}|{}{}", rgb.0 as u8, rgb.1 as u8);
        if key == self.look_key {
            return;
        }
        self.look_key = key;
        // (RGB switched off / another theme: the saved colours come back here)
        theme::apply(ui, &theme::palette(&id, &acc, &bg));
        let s = ui.global::<S>();
        s.set_ui_theme(id.as_str().into());
        s.set_themes(theme::lines(&id, &acc, &bg));
        s.set_ui_accent_rgb(rgb.0);
        s.set_ui_bg_rgb(rgb.1);
        // RGB: a 33 ms timer only while the custom theme has an RGB colour (it draws only while this window is in front)
        if id == "custom" && (rgb.0 || rgb.1) {
            if !self.rgb.running() {
                let last = std::cell::Cell::new(Instant::now());
                self.rgb.start(slint::TimerMode::Repeated, Duration::from_millis(33), move || {
                    let dt = last.replace(Instant::now()).elapsed().as_secs_f32().min(0.2);
                    with_app(|a| a.rgb_tick(dt));
                });
            }
        } else {
            self.rgb.stop();
            ui.global::<T>().set_rgb_live(false);
        }
    }

    /// One RGB step (Echo's lib_color.rs, with its 11 Nov 2026 idle-CPU fix): the hue turns once every 6 s (~30
    /// steps a second, 2 deg each), ONLY while this window is in front - another window in front (= while you
    /// play), minimised or hidden: nothing moves, no redraw at all. The hue only advances while running, so it
    /// continues where it stopped instead of jumping. The saved colours keep their saturation + brightness; nothing
    /// is written to config.ini per step.
    fn rgb_tick(&mut self, dt: f32) {
        let Some(ui) = self.ui() else { return };
        let live = front(&ui);
        let t = ui.global::<T>();
        if t.get_rgb_live() != live {
            t.set_rgb_live(live); // colour fades off while it turns: 1 step = 1 frame
        }
        if !live {
            return;
        }
        self.hue = (self.hue + dt * 60.0) % 360.0;
        let (st, hue) = (&self.m.set, self.hue);
        let turn = |c: Color| {
            let (h, s, v) = hsv_of(c);
            let s = if s < 0.25 { 0.85 } else { s }; // white / grey would stay grey
            Color::from_hsv(((h + hue) % 360.0) / 360.0, s, v.max(0.35)).to_hex()
        };
        let acc = if st.ui_accent_rgb { turn(st.ui_accent) } else { st.ui_accent.to_hex() };
        let bg = if st.ui_bg_rgb { turn(st.ui_bg) } else { st.ui_bg.to_hex() };
        theme::apply(&ui, &theme::palette("custom", &acc, &bg));
    }

    /// A colour picked for `key`: crosshair colours ("color", "outline_color") or the custom theme ("app.ui_accent", "app.ui_bg")
    fn put_color(&mut self, key: &str, c: Color) {
        match key.strip_prefix("app.") {
            Some(k) => self.m.set.set(k, &c.to_hex()),
            None => {
                self.m.cur.set(key, &c.to_hex());
                self.m.cur.mode = 0;
            }
        }
    }

    fn color_of(&self, key: &str) -> Color {
        match key {
            "outline_color" => self.m.cur.outline_color,
            "app.ui_accent" => self.m.set.ui_accent,
            "app.ui_bg" => self.m.set.ui_bg,
            "app.zoom_ring" => self.m.set.zoom_ring,
            _ => self.m.cur.color,
        }
    }

    /// The preview: the test-range background + the crosshair at real pixels (x zoom) + the colour-blind view
    fn preview(&mut self) {
        let Some(ui) = self.ui() else { return };
        let mut c = Canvas::new(0, 0);
        self.m.render(&mut c);
        let zoom = [1, 2, 4][self.m.set.preview_zoom.min(2) as usize];
        // your own background picture: loaded + scaled once (a missing file falls back to the built-in one)
        let want = self.m.set.preview_img.clone();
        if want.is_empty() {
            self.own_bg = None;
        } else if self.own_bg.as_ref().map(|o| o.0.as_str()) != Some(want.as_str()) {
            self.own_bg = crosshair_core::load_png(&bg_dir().join(&want)).map(|i| (want.clone(), range::cover(&i, range::SIZE, range::SIZE)));
        }
        let img = range::compose(self.m.set.preview_bg, self.own_bg.as_ref().map(|o| &o.1), self.pan, &c, zoom, self.cvd);
        ui.global::<S>().set_preview(to_image(&img));
    }

    /// The background thumbnails: the built-in ones + your own pictures + room for "Add your own…"
    fn refresh_backdrops(&mut self) {
        let Some(ui) = self.ui() else { return };
        let mut all = self.bg_thumbs.clone();
        let mut own: Vec<String> = std::fs::read_dir(bg_dir())
            .map(|d| d.filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| n.to_lowercase().ends_with(".png") && !n.starts_with('.')).collect())
            .unwrap_or_default();
        own.sort_by_key(|n| n.to_lowercase());
        for n in own.iter().take(30) {
            if let Some(img) = crosshair_core::load_png(&bg_dir().join(n)) {
                let c = range::window(&range::cover(&img, 96, 64), 96, 64, 0.0);
                let name = n.trim_end_matches(".png").trim_end_matches(".PNG");
                all.push(Backdrop { name: name.into(), img: to_image(&c), value: -1, file: n.as_str().into() });
            }
        }
        let s = ui.global::<S>();
        s.set_bg_rows((all.len() as i32 + 1 + 4) / 5);
        s.set_backdrops(ModelRc::new(VecModel::from(all)));
    }

    /// The gallery's tiles, drawn the way picking them would look (your colour, the Colour sets in theirs); only
    /// while it's open and when something they show changed
    fn refresh_gallery(&mut self) {
        let Some(ui) = self.ui() else { return };
        let s = ui.global::<S>();
        s.set_gal_count(self.more.len() as i32);
        if !s.get_gallery() {
            self.gal_key = u64::MAX;
            return;
        }
        let p = &self.m.cur;
        let key = crosshair::geom_key(p)
            ^ ((p.color.r as u64) << 8 | (p.color.g as u64) << 16 | (p.color.b as u64) << 24)
            ^ ((p.outline_color.r as u64) << 32 | (p.outline_color.g as u64) << 40 | (p.outline_color.b as u64) << 48)
            ^ (p.mode as u64) << 60
            ^ self.gal_q.bytes().fold(1469598103934665603u64, |h, b| (h ^ b as u64).wrapping_mul(1099511628211));
        if key == self.gal_key {
            return;
        }
        self.gal_key = key;
        let cur_key = crosshair::geom_key(&self.m.cur);
        let mut secs: Vec<GalSection> = Vec::new();
        for sec in crosshair_core::presets_more::SECTIONS {
            let mut items = Vec::new();
            for (i, m) in self.more.iter().enumerate().filter(|(_, m)| m.section == sec && crosshair_core::presets_more::matches(m, &self.gal_q)) {
                let mut q = self.m.cur.clone();
                crosshair_core::presets_more::apply(&mut q, m);
                let on = self.m.cur.mode == 0 && crosshair::geom_key(&q) == cur_key && q.color == self.m.cur.color;
                q.opacity = 1.0;
                q.rainbow = false;
                let mut c = Canvas::new(0, 0);
                self.m.rend.render(&q, 0.0, &mut c);
                items.push(GalItem { name: m.name.into(), img: to_image(&c), on, idx: i as i32 });
            }
            if !items.is_empty() {
                secs.push(GalSection { title: sec.to_uppercase().into(), items: ModelRc::new(VecModel::from(items)) });
            }
        }
        s.set_gal(ModelRc::new(VecModel::from(secs)));
    }

    /// Preset thumbnails in the current colour (only rebuilt when something they show changed).
    fn refresh_presets(&mut self) {
        let p = &self.m.cur;
        let key = crosshair::geom_key(p) ^ (p.color.to_hex().len() as u64) ^ ((p.color.r as u64) << 8 | (p.color.g as u64) << 16 | (p.color.b as u64) << 24)
            ^ ((p.outline_color.r as u64) << 32 | (p.outline_color.g as u64) << 40 | (p.outline_color.b as u64) << 48)
            ^ (p.mode as u64) << 60;
        if key == self.preset_key && self.presets.row_count() > 0 {
            return;
        }
        self.preset_key = key;
        let cur_key = crosshair::geom_key(&self.m.cur);
        let mut rows = Vec::new();
        for (name, shape) in self.m.presets.clone() {
            let mut q = self.m.cur.clone();
            crosshair_core::presets_more::apply_builtin(&mut q, rows.len(), &shape);
            q.opacity = 1.0;
            q.rainbow = false;
            let on = self.m.cur.mode == 0 && crosshair::geom_key(&q) == cur_key;
            let mut c = Canvas::new(0, 0);
            self.m.rend.render(&q, 0.0, &mut c);
            rows.push(Preset { name: name.into(), img: to_image(&c), on });
        }
        self.presets.set_vec(rows);
    }

    /// RGB rainbow / animated picture and the slow pan: the preview redraws at 30 fps while either is on (only
    /// then), and only while this window is in front (Echo's idle-CPU lesson, 11 Nov 2026: behind a game it drew
    /// 30 frames a second for nobody). The clock only runs while it draws, so it continues instead of jumping.
    fn animate(&mut self) {
        if self.m.cur.animated() || self.pan_on {
            if !self.anim.running() {
                let (last, clock) = (std::cell::Cell::new(Instant::now()), std::cell::Cell::new(self.m.time));
                let pan0 = self.pan - self.m.time as f32 * 12.0;
                self.anim.start(slint::TimerMode::Repeated, Duration::from_millis(33), move || {
                    let dt = last.replace(Instant::now()).elapsed().as_secs_f64().min(0.2);
                    with_app(|a| {
                        if !a.ui().is_some_and(|ui| front(&ui)) {
                            return;
                        }
                        clock.set(clock.get() + dt);
                        let t = clock.get();
                        a.m.time = t;
                        if a.pan_on {
                            a.pan = (pan0 + t as f32 * 12.0).rem_euclid(range::SIZE as f32); // 12 px a second
                        }
                        a.preview();
                    })
                });
            }
        } else {
            self.anim.stop();
        }
    }

    /// The state changed here: show it, and send it to the overlay (or save it when none runs).
    fn changed(&mut self) {
        self.m.cur.sanitize();
        self.m.set.sanitize();
        self.refresh();
        let text = self.m.to_text();
        let sent = match &mut self.conn {
            Some(c) => ipc::write_msg(c, "state", &text).is_ok(),
            None => false,
        };
        if !sent {
            self.offline();
            self.save_timer.start(slint::TimerMode::SingleShot, Duration::from_millis(400), || with_app(|a| a.m.save()));
        }
    }

    fn offline(&mut self) {
        if self.conn.take().is_some() || self.ui().map_or(false, |u| u.global::<S>().get_conn() != "offline") {
            if let Some(ui) = self.ui() {
                ui.global::<S>().set_conn("offline".into());
            }
        }
    }

    fn on_msg(&mut self, kind: &str, body: &str) {
        let Some(ui) = self.ui() else { return };
        let s = ui.global::<S>();
        match kind {
            "state" => {
                self.m.apply_text(body);
                self.refresh();
            }
            "info" => {
                let (mut got_kde, mut got_hk_ok) = (false, false);
                for (k, v) in ipc::kv(body) {
                    match k {
                        "platform" => s.set_platform(
                            match v {
                                "wayland-layer-shell" => "Wayland layer-shell",
                                "x11" => "X11 window",
                                "win32" => "Windows overlay window",
                                o => o,
                            }
                            .into(),
                        ),
                        "aim" => s.set_aim_status(v.into()),
                        "hk_ok" => {
                            got_hk_ok = true;
                            let ok: Vec<bool> = v.split(',').map(|b| b.trim() != "0").chain(std::iter::repeat(true)).take(9).collect();
                            s.set_hk_ok(ModelRc::new(VecModel::from(ok)));
                        }
                        "gamebar" => s.set_gamebar(v.into()),
                        // Linux: which keys KDE really uses ("unavailable" = no shortcuts portal, no keyboard to read)
                        "hk_kde" => {
                            if v == "unavailable" {
                                self.hk_kde = None;
                                s.set_hk_kde_state("unavailable".into());
                            } else {
                                let k: Vec<String> = v.split('|').map(|x| x.trim().to_string()).chain(std::iter::repeat(String::new())).take(8).collect();
                                let shown: Vec<SharedString> = k.iter().map(|x| x.split('+').map(|p| p.trim()).collect::<Vec<_>>().join(" + ").into()).collect();
                                s.set_hk_kde(ModelRc::new(VecModel::from(shown)));
                                s.set_hk_kde_state("kde".into());
                                self.hk_kde = Some(k);
                            }
                            got_kde = true;
                        }
                        "monitors" => {
                            let opts: Vec<Opt> = v
                                .split('|')
                                .filter(|n| !n.is_empty())
                                .enumerate()
                                .map(|(i, n)| Opt { label: format!("{}: {}", i + 1, n).into(), value: i as i32 })
                                .collect();
                            s.set_monitors(ModelRc::new(VecModel::from(opts)));
                        }
                        _ => {}
                    }
                }
                // hk_ok without hk_kde = the overlay owns the keys itself (Windows, Linux keyboards / X11)
                if got_hk_ok && !got_kde {
                    self.hk_kde = None;
                    s.set_hk_kde_state("".into());
                }
                if got_kde || got_hk_ok {
                    self.refresh(); // the top bar's hotkey chip
                }
            }
            "raise" => {
                let _ = ui.show();
                use slint::winit_030::WinitWindowAccessor;
                ui.window().with_winit_window(|w| w.focus_window());
            }
            "bye" => {
                // The overlay was quit (tray / another window): nothing left to set.
                let _ = slint::quit_event_loop();
            }
            _ => {}
        }
    }
}

/// Reads the overlay's messages on a thread and hands them to the UI thread.
fn reader(stream: Stream) {
    std::thread::spawn(move || {
        let mut r = std::io::BufReader::new(stream);
        while let Ok((k, b)) = ipc::read_msg(&mut r) {
            let _ = slint::invoke_from_event_loop(move || with_app(|a| a.on_msg(&k, &b)));
        }
        let _ = slint::invoke_from_event_loop(|| with_app(|a| a.offline()));
    });
}

/// Connects; if no overlay runs, starts one (`aimdot --tray`) and waits up to 3 s for it.
fn start_connect(spawn: bool) {
    std::thread::spawn(move || {
        let mut s = connect();
        if s.is_none() && spawn {
            let _ = std::process::Command::new(paths::sibling_exe(crosshair_core::OVERLAY_EXE)).arg("--tray").spawn();
            for _ in 0..30 {
                std::thread::sleep(Duration::from_millis(100));
                s = connect();
                if s.is_some() {
                    break;
                }
            }
        }
        let rs = s.as_ref().and_then(|s| s.try_clone().ok());
        let _ = slint::invoke_from_event_loop(move || {
            with_app(|a| {
                let Some(ui) = a.ui() else { return };
                match (s, rs) {
                    (Some(s), Some(r)) => {
                        a.conn = Some(s);
                        ui.global::<S>().set_conn("ok".into());
                        reader(r);
                    }
                    _ => ui.global::<S>().set_conn("offline".into()),
                }
            })
        });
    });
}

/// Windows virtual-key code for the key Slint reports (letters, digits, F-keys, arrows, a few more).
/// Modifier keys alone give None (keep listening).
pub fn vk_of(text: &str) -> Option<u32> {
    use slint::platform::Key as K;
    let mut it = text.chars();
    let c = it.next()?;
    if it.next().is_some() {
        return None;
    }
    if c.is_ascii_alphabetic() {
        return Some(c.to_ascii_uppercase() as u32);
    }
    if c.is_ascii_digit() {
        return Some(c as u32);
    }
    // Shift + digit on a US layout
    if let Some(i) = ")!@#$%^&*(".find(c) {
        return Some(0x30 + i as u32);
    }
    let fkeys = [K::F1, K::F2, K::F3, K::F4, K::F5, K::F6, K::F7, K::F8, K::F9, K::F10, K::F11, K::F12];
    if let Some(i) = fkeys.iter().position(|k| char::from(k.clone()) == c) {
        return Some(0x70 + i as u32);
    }
    let named = [
        (K::LeftArrow, 0x25),
        (K::UpArrow, 0x26),
        (K::RightArrow, 0x27),
        (K::DownArrow, 0x28),
        (K::Home, 0x24),
        (K::End, 0x23),
        (K::PageUp, 0x21),
        (K::PageDown, 0x22),
        (K::Insert, 0x2D),
        (K::Space, 0x20),
        (K::Tab, 0x09),
        (K::Return, 0x0D),
        (K::Pause, 0x13),
    ];
    if let Some((_, v)) = named.iter().find(|(k, _)| char::from(k.clone()) == c) {
        return Some(*v);
    }
    Some(match c {
        ' ' => 0x20,
        ';' | ':' => 0xBA,
        '=' | '+' => 0xBB,
        ',' | '<' => 0xBC,
        '-' | '_' => 0xBD,
        '.' | '>' => 0xBE,
        '/' | '?' => 0xBF,
        '`' | '~' => 0xC0,
        '[' | '{' => 0xDB,
        '\\' | '|' => 0xDC,
        ']' | '}' => 0xDD,
        '\'' | '"' => 0xDE,
        _ => return None,
    })
}

thread_local! {
    /// kept while the window is open: on Linux the copied text is gone once its clipboard object is dropped
    static CLIP: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
}

/// Text onto the clipboard; the note to show
fn copy_text(t: &str) -> String {
    CLIP.with(|c| {
        let mut c = c.borrow_mut();
        if c.is_none() {
            *c = arboard::Clipboard::new().ok();
        }
        match c.as_mut().map(|c| c.set_text(t.to_string())) {
            Some(Ok(())) => "Copied the last part of the log. Paste it into your message.".into(),
            Some(Err(e)) => format!("Couldn't copy ({e})."),
            None => "Couldn't reach the clipboard.".into(),
        }
    })
}

/// The preview's own background pictures
fn bg_dir() -> std::path::PathBuf {
    paths::config_dir().join("backgrounds")
}

/// This window is the one in front (visible, not minimised, has the focus). Timers that redraw something nonstop
/// (RGB theme, the animated preview) only draw while it is - Echo's idle-CPU lesson (11 Nov 2026): a window that
/// redraws 30-165 times a second behind a game costs ~1 core for nothing.
fn front(ui: &AppWindow) -> bool {
    let w = ui.window();
    w.is_visible() && !w.is_minimized() && {
        use slint::winit_030::WinitWindowAccessor;
        w.with_winit_window(|w| w.has_focus()).unwrap_or(true)
    }
}

fn hsv_of(c: Color) -> (f32, f32, f32) {
    let (h, s, v) = c.to_hsv();
    (h * 360.0, s, v)
}

/// Linux: Wayland app_id / X11 WM_CLASS "link.sonela.Aimdot" = the desktop file's name (scripts/install-dev-linux.sh).
/// KWin's taskbar and Alt+Tab find the icon only through a desktop file named exactly like the app_id (Echo, 10 Nov 2026).
fn select_backend() {
    let r = slint::BackendSelector::new()
        .with_winit_window_attributes_hook(|attrs| {
            #[cfg(target_os = "linux")]
            let attrs = {
                use slint::winit_030::winit::platform::{wayland::WindowAttributesExtWayland, x11::WindowAttributesExtX11};
                let a = WindowAttributesExtWayland::with_name(attrs, APP_ID, "aimdot-settings");
                WindowAttributesExtX11::with_name(a, APP_ID, "aimdot-settings")
            };
            attrs
        })
        .select();
    if let Err(e) = r {
        eprintln!("backend: {e}");
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const APP_ID: &str = crosshair_core::desktop_entry::APP_ID;

fn main() {
    // auto-update (src/update.rs): `--check-update` (the overlay starts it every 4 h) runs headless and exits
    if update::cli() {
        return;
    }
    // downloaded copies (AppImage / tar.gz): the menu entry + icons, so Alt+Tab / the taskbar find the icon
    // (core::desktop_entry: skips cargo builds, Flatpak and test copies; leaves another Aimdot's entry alone)
    #[cfg(target_os = "linux")]
    let _ = crosshair_core::desktop_entry::ensure();
    crosshair_core::log::start("settings", "");
    select_backend();
    if std::env::var("CROSSHAIR_MEASURE_LAZY").is_ok() {
        return measure::lazy();
    }
    let ui = AppWindow::new().expect("window");
    // auto-update: banner + Settings > About card; kept alive as long as the window
    let _updater = update::wire!(ui);
    let dir = paths::config_dir();
    let _ = std::fs::create_dir_all(&dir);
    let presets = Rc::new(VecModel::<Preset>::default());
    {
        let s = ui.global::<S>();
        s.set_presets(ModelRc::from(presets.clone()));
        // Test hooks for screenshots on Linux: CROSSHAIR_UI_WINDOWS=1 shows the Windows-only parts, CROSSHAIR_PAGE=n
        s.set_is_windows(cfg!(windows) || std::env::var("CROSSHAIR_UI_WINDOWS").is_ok());
        if let Some(p) = std::env::var("CROSSHAIR_PAGE").ok().and_then(|v| v.parse().ok()) {
            s.set_page(p);
        }
        s.set_version(crosshair_core::VERSION.into());
        s.set_anticheat(crosshair_core::ANTICHEAT_NOTE.into());
        s.set_monitors(ModelRc::new(VecModel::from(vec![Opt { label: "1".into(), value: 0 }])));
        // test hooks for screenshots: CROSSHAIR_CVD=1..3, CROSSHAIR_PAN=1
        if let Some(m) = std::env::var("CROSSHAIR_CVD").ok().and_then(|v| v.parse::<i32>().ok()) {
            s.set_cvd(m.clamp(0, 3));
        }
    }
    let mut app = App {
        m: Model::new(dir, crosshair_core::load_png),
        ui: ui.as_weak(),
        conn: None,
        hsv: (0.0, 0.0, 1.0),
        presets,
        preset_key: u64::MAX,
        save_timer: slint::Timer::default(),
        anim: slint::Timer::default(),
        rgb: slint::Timer::default(),
        hue: 0.0,
        look_key: String::new(),
        autostart: None,
        pan_on: false,
        pan: 0.0,
        cvd: 0,
        bg_thumbs: range::ORDER
            .iter()
            .map(|&i| Backdrop { name: range::NAMES[i as usize].into(), img: to_image(&range::backdrop(i, 96, 64, 0.0)), value: i as i32, file: "".into() })
            .collect(),
        own_bg: None,
        hk_kde: None,
        more: crosshair_core::presets_more::all(),
        gal_q: String::new(),
        gal_key: u64::MAX,
    };
    app.refresh();
    APP.with(|a| *a.borrow_mut() = Some(app));
    images::init(&ui);
    painter::init(&ui);
    profiles::init(&ui);
    // the first refresh ran before the pages were wired: once more, so the Images list is filled at start
    with_app(|a| {
        if let Some(ui) = a.ui() {
            a.cvd = ui.global::<S>().get_cvd() as u32;
        }
        a.pan_on = std::env::var_os("CROSSHAIR_PAN").is_some();
        a.refresh_backdrops();
        // test hook: CROSSHAIR_GALLERY=<search> (or 1) opens "More presets" at start
        if let Ok(g) = std::env::var("CROSSHAIR_GALLERY") {
            a.gal_q = if g == "1" { String::new() } else { g };
            if let Some(ui) = a.ui() {
                ui.global::<S>().set_gallery(true);
                ui.global::<S>().set_gal_q(a.gal_q.as_str().into());
            }
        }
        if let Ok(k) = std::env::var("CROSSHAIR_KDE") {
            a.on_msg("info", &format!("hk_ok=1,1,1,1\nhk_kde={k}\n"));
        }
        if let Some(ui) = a.ui() {
            ui.global::<S>().set_pan(a.pan_on);
        }
        a.refresh()
    });

    // Echo's settings search: every word of the query somewhere in the text, case-insensitive
    ui.global::<Find>().on_matches(|q, text| {
        let t = text.to_lowercase();
        q.to_lowercase().split_whitespace().all(|w| t.contains(w))
    });
    // test hooks: CROSSHAIR_TOAST=<text> shows a toast at start; CROSSHAIR_KDE=<hk_kde line> fakes the overlay's info
    if let Ok(t) = std::env::var("CROSSHAIR_TOAST") {
        let w = ui.as_weak();
        slint::Timer::single_shot(Duration::from_millis(2500), move || {
            if let Some(ui) = w.upgrade() {
                let g = ui.global::<Toast>();
                g.set_text(t.replace('_', " ").into());
                g.set_seq(g.get_seq() + 1);
            }
        });
    }
    // test hook: CROSSHAIR_FIND=<words> starts with that search in Settings
    if let Ok(q) = std::env::var("CROSSHAIR_FIND") {
        ui.global::<Find>().set_q(q.into());
    }
    let s = ui.global::<S>();
    s.on_num(|key, v| {
        with_app(|a| {
            let key = key.as_str();
            if key == "ui.cvd" {
                a.cvd = (v as u32).min(3);
                if let Some(ui) = a.ui() {
                    ui.global::<S>().set_cvd(a.cvd as i32);
                }
                a.preview();
                return;
            }
            if key == "zoom" {
                a.m.set.preview_zoom = match v as i32 { 1 => 0, 2 => 1, _ => 2 };
            } else if let Some(k) = key.strip_prefix("app.") {
                a.m.set.set(k, &num_text(v));
                if k == "preview_bg" {
                    a.m.set.preview_img.clear(); // a built-in background instead of your own picture
                }
                if k == "aim_mode" {
                    a.m.set.hide_ads = a.m.set.aim_mode == 2; // so sanitize() doesn't read the old hide_ads as "hide"
                }
            } else {
                a.m.cur.set(key, &num_text(v));
                a.m.cur.mode = 0;
            }
            a.changed();
        })
    });
    s.on_flag(|key, on| {
        with_app(|a| {
            // preview-only switches (not saved, not sent to the overlay)
            if key == "ui.pan" {
                a.pan_on = on;
                a.anim.stop(); // restart it from the current pan position
                if let Some(ui) = a.ui() {
                    ui.global::<S>().set_pan(on);
                }
                a.animate();
                a.preview();
                return;
            }
            let v = if on { "1" } else { "0" };
            match key.as_str().strip_prefix("app.") {
                Some(k) => a.m.set.set(k, v),
                None => {
                    a.m.cur.set(key.as_str(), v);
                    a.m.cur.mode = 0;
                }
            }
            a.changed();
        })
    });
    s.on_open_picker(|key| {
        with_app(|a| {
            let Some(ui) = a.ui() else { return };
            let s = ui.global::<S>();
            let c = a.color_of(key.as_str());
            a.hsv = hsv_of(c);
            s.set_ph(a.hsv.0);
            s.set_ps(a.hsv.1);
            s.set_pv(a.hsv.2);
            s.set_phex(c.to_hex().into());
            s.set_pick_key(key);
        })
    });
    s.on_pick(|key, h, sat, v| {
        with_app(|a| {
            let c = Color::from_hsv(h / 360.0, sat, v);
            a.put_color(key.as_str(), c);
            a.hsv = (h, sat, v);
            if let Some(ui) = a.ui() {
                let s = ui.global::<S>();
                s.set_ph(h);
                s.set_ps(sat);
                s.set_pv(v);
                s.set_phex(c.to_hex().into());
            }
            a.changed();
        })
    });
    s.on_hex(|key, text| {
        with_app(|a| {
            let Some(c) = Color::parse(text.as_str()) else { return };
            a.put_color(key.as_str(), c);
            if let Some(ui) = a.ui() {
                let s = ui.global::<S>();
                if s.get_pick_key() == key {
                    a.hsv = hsv_of(c);
                    s.set_ph(a.hsv.0);
                    s.set_ps(a.hsv.1);
                    s.set_pv(a.hsv.2);
                    s.set_phex(SharedString::from(c.to_hex()));
                }
            }
            a.changed();
        })
    });
    s.on_set_color(|key, c| {
        with_app(|a| {
            let c = Color { r: c.red(), g: c.green(), b: c.blue(), a: 255 };
            a.put_color(key.as_str(), c);
            if let Some(ui) = a.ui() {
                let s = ui.global::<S>();
                if s.get_pick_key() == key {
                    a.hsv = hsv_of(c);
                    s.set_ph(a.hsv.0);
                    s.set_ps(a.hsv.1);
                    s.set_pv(a.hsv.2);
                    s.set_phex(SharedString::from(c.to_hex()));
                }
            }
            a.changed();
        })
    });
    // About > Log: open its folder / copy its last 64 KB
    s.on_open_log_folder(|| {
        let dir = crosshair_core::log::folder();
        let _ = std::fs::create_dir_all(&dir);
        let opener = if cfg!(windows) { "explorer" } else { "xdg-open" };
        let note = match std::process::Command::new(opener).arg(&dir).spawn() {
            Ok(_) => String::new(),
            Err(e) => format!("Couldn't open the folder ({e})."),
        };
        with_app(|a| {
            if let Some(ui) = a.ui() {
                ui.global::<S>().set_log_note(note.into());
            }
        });
    });
    s.on_copy_log(|| {
        let text = crosshair_core::log::tail_text(64 * 1024);
        let note = if text.trim().is_empty() { "The log is empty so far.".to_string() } else { copy_text(&text) };
        with_app(|a| {
            if let Some(ui) = a.ui() {
                ui.global::<S>().set_log_note(note.into());
            }
        });
    });
    // the preview's own background pictures (copied into <config>/backgrounds/ like the Images page imports)
    s.on_bg_pick(|file| {
        with_app(|a| {
            a.m.set.preview_img = file.to_string();
            a.changed();
        })
    });
    s.on_bg_remove(|file| {
        with_app(|a| {
            let _ = std::fs::remove_file(bg_dir().join(file.as_str()));
            if a.m.set.preview_img == file.as_str() {
                a.m.set.preview_img.clear();
            }
            a.refresh_backdrops();
            a.changed();
        })
    });
    s.on_bg_add(|| {
        let owner = images::owner_handle();
        if let Some(ui) = APP.with(|a| a.borrow().as_ref().and_then(|a| a.ui())) {
            ui.global::<S>().set_bg_status("Choose a picture…".into());
        }
        std::thread::spawn(move || {
            let picked = images::pick_file(owner);
            let _ = slint::invoke_from_event_loop(move || {
                with_app(|a| {
                    let msg = match picked {
                        Ok(Some(p)) => match images::import_file(&bg_dir(), &p) {
                            Ok(name) => {
                                a.m.set.preview_img = name;
                                a.refresh_backdrops();
                                a.changed();
                                String::new()
                            }
                            Err(e) => e,
                        },
                        Ok(None) => String::new(),
                        Err(e) => e,
                    };
                    if let Some(ui) = a.ui() {
                        ui.global::<S>().set_bg_status(msg.into());
                    }
                })
            });
        });
    });
    s.on_set_theme(|id| {
        with_app(|a| {
            let st = &mut a.m.set;
            // Custom starts from the look you had (Echo)
            if id == "custom" && st.ui_theme != "custom" {
                let p = theme::palette(&st.ui_theme, "", "");
                if let (Some(acc), Some(bg)) = (Color::parse(&p.accent_hex), Color::parse(&p.bg_hex)) {
                    st.ui_accent = acc;
                    st.ui_bg = bg;
                }
            }
            st.ui_theme = id.to_string();
            if let Some(ui) = a.ui() {
                ui.global::<S>().set_pick_key("".into());
            }
            a.changed();
        })
    });
    s.on_theme_reset(|| {
        with_app(|a| {
            let d = crosshair_core::config::Settings::default();
            a.m.set.ui_theme = d.ui_theme;
            a.m.set.ui_accent = d.ui_accent;
            a.m.set.ui_bg = d.ui_bg;
            if let Some(ui) = a.ui() {
                ui.global::<S>().set_pick_key("".into());
            }
            a.changed();
        })
    });
    s.on_gal_search(|q| {
        with_app(|a| {
            a.gal_q = q.to_string();
            if let Some(ui) = a.ui() {
                ui.global::<S>().set_gal_q(q);
            }
            a.refresh_gallery();
        })
    });
    s.on_gal_pick(|i| {
        with_app(|a| {
            if let Some(m) = a.more.get(i as usize).cloned() {
                crosshair_core::presets_more::apply(&mut a.m.cur, &m);
                a.changed();
            }
        })
    });
    s.on_preset(|i| {
        with_app(|a| {
            if let Some((_, shape)) = a.m.presets.get(i as usize).cloned() {
                crosshair_core::presets_more::apply_builtin(&mut a.m.cur, i as usize, &shape);
                a.changed();
            }
        })
    });
    s.on_randomize(|| {
        with_app(|a| {
            a.m.time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1.0, |d| d.as_secs_f64() % 100_000.0);
            a.m.randomize();
            a.changed();
        })
    });
    s.on_reset_design(|| {
        with_app(|a| {
            let mut d = a.m.cur.clone();
            crosshair::apply_shape(&mut d, &Profile::default());
            a.m.cur = d;
            a.changed();
        })
    });
    s.on_hk_start(|n| {
        with_app(|a| {
            if let Some(ui) = a.ui() {
                ui.global::<S>().set_hk_capture(n);
            }
            // The overlay lets go of its hotkeys meanwhile, so the keys reach this window.
            if let Some(c) = &mut a.conn {
                let _ = ipc::write_msg(c, "capture", "1");
            }
        })
    });
    s.on_hk_cancel(|| {
        with_app(|a| {
            if let Some(ui) = a.ui() {
                ui.global::<S>().set_hk_capture(-1);
            }
            if let Some(c) = &mut a.conn {
                let _ = ipc::write_msg(c, "capture", "0");
            }
        })
    });
    s.on_hk_key(|n, text, ctrl, alt, shift, meta| {
        use slint::platform::Key as K;
        let t = text.as_str();
        let clear = t == SharedString::from(K::Backspace).as_str() || t == SharedString::from(K::Delete).as_str();
        let vk = if clear { Some(0) } else { vk_of(t) };
        let Some(vk) = vk else { return }; // a modifier alone: keep listening
        use crosshair_core::config::{MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_WIN};
        let mods = if vk == 0 { 0 } else { (alt as u32 * MOD_ALT) | (ctrl as u32 * MOD_CTRL) | (shift as u32 * MOD_SHIFT) | (meta as u32 * MOD_WIN) };
        with_app(|a| a.set_hotkey(n, crosshair_core::config::Hotkey { mods, vk }));
    });
    // Esc while capturing, or the × next to a key: no key
    s.on_hk_clear(|n| with_app(|a| a.set_hotkey(n, crosshair_core::config::Hotkey { mods: 0, vk: 0 })));
    s.on_start_overlay(|| {
        if let Some(ui) = APP.with(|a| a.borrow().as_ref().and_then(|a| a.ui())) {
            ui.global::<S>().set_conn("starting".into());
        }
        start_connect(true);
    });
    s.on_quit_overlay(|| {
        with_app(|a| {
            if let Some(c) = &mut a.conn {
                let _ = ipc::write_msg(c, "quit", "");
                let _ = c.flush();
            }
        });
        let _ = slint::quit_event_loop();
    });

    // Test hook: CROSSHAIR_WINDOW=<w>x<h> (screenshots of long pages)
    if let Some((w, h)) = std::env::var("CROSSHAIR_WINDOW").ok().and_then(|v| {
        let (w, h) = v.split_once('x')?;
        Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))
    }) {
        ui.window().set_size(slint::LogicalSize::new(w, h));
    }
    titlebar::wire(&ui);
    start_connect(true);
    ui.run().expect("event loop");
    // Closing the window: make sure nothing is lost if no overlay runs.
    with_app(|a| {
        if a.conn.is_none() {
            a.m.save();
        }
    });
}

mod measure {
    //! `CROSSHAIR_MEASURE_LAZY=1 aimdot-settings`: what "lazy init inside the overlay" would cost - memory
    //! before Slint, with the window open, and after the window is closed while the event loop lives on
    //! (an always-running overlay can't end winit's event loop and start it again).
    use super::*;

    pub fn rss() -> String {
        #[cfg(target_os = "linux")]
        {
            std::fs::read_to_string("/proc/self/status")
                .unwrap_or_default()
                .lines()
                .filter(|l| l.starts_with("VmRSS") || l.starts_with("RssAnon") || l.starts_with("RssFile"))
                .map(|l| l.split_whitespace().skip(1).collect::<Vec<_>>().join(" "))
                .collect::<Vec<_>>()
                .join(" / ")
        }
        #[cfg(not(target_os = "linux"))]
        {
            String::from("(Linux only)")
        }
    }

    pub fn lazy() {
        println!("before Slint:            RSS / anon / file = {}", rss());
        let ui = AppWindow::new().unwrap();
        ui.show().unwrap();
        let keep = RefCell::new(Some(ui));
        let t = slint::Timer::default();
        let step = std::cell::Cell::new(0);
        t.start(slint::TimerMode::Repeated, Duration::from_secs(3), move || {
            let n = step.get();
            step.set(n + 1);
            match n {
                0 => {
                    println!("window open (3 s):       RSS / anon / file = {}", rss());
                    // hide + drop the last handle: the component is freed, the event loop stays
                    if let Some(u) = keep.borrow_mut().take() {
                        let _ = u.hide();
                    }
                }
                1 => println!("closed + dropped (3 s):  RSS / anon / file = {}", rss()),
                _ => {
                    println!("closed + dropped (6 s):  RSS / anon / file = {}", rss());
                    let _ = slint::quit_event_loop();
                }
            }
        });
        let _ = slint::run_event_loop_until_quit();
    }
}

#[cfg(test)]
mod tests {
    use super::vk_of;
    use slint::platform::Key as K;

    #[test]
    fn key_names_to_virtual_keys() {
        assert_eq!(vk_of("x"), Some(0x58));
        assert_eq!(vk_of("X"), Some(0x58));
        assert_eq!(vk_of("7"), Some(0x37));
        assert_eq!(vk_of("&"), Some(0x37));
        assert_eq!(vk_of(slint::SharedString::from(K::F6).as_str()), Some(0x75));
        assert_eq!(vk_of(slint::SharedString::from(K::RightArrow).as_str()), Some(0x27));
        assert_eq!(vk_of(slint::SharedString::from(K::Shift).as_str()), None);
        assert_eq!(vk_of(slint::SharedString::from(K::Control).as_str()), None);
        assert_eq!(vk_of("ab"), None);
        let hk = crosshair_core::config::Hotkey { mods: 3, vk: vk_of("x").unwrap() };
        assert_eq!(hk.name(), "Ctrl + Alt + X");
    }
}
