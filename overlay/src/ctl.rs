//! The overlay's brain, the same on every platform: the model, saving, the settings-window connections, and
//! what is visible. The platform layers (win.rs, linux/) feed it events and do what the returned `Out` says.

use crosshair_core::{ipc, log, model::Model, paths};
use std::io::Write;
use std::process::Child;
use std::time::{Duration, Instant};

#[cfg_attr(not(windows), allow(dead_code))] // tray: Windows only for now
pub enum Ev {
    /// A settings window connected (its write half).
    Connected(u64, Box<dyn Write + Send>),
    Msg(u64, String, String),
    Gone(u64),
    /// Right mouse button (hide while aiming).
    Aim(bool),
    /// 1 toggle, 2 next profile, 3 previous profile, 4 open settings (the original hotkey ids),
    /// 5..8 nudge the centre 1 px up / down / left / right.
    Hotkey(u32),
    /// Linux: the keys the desktop's shortcuts portal really uses, one per hotkey ("" = no key there);
    /// None = no shortcuts portal (hotkeys can't work).
    #[cfg_attr(windows, allow(dead_code))]
    KdeKeys(Option<Vec<String>>),
    /// Linux: XDG activation token that came with a portal hotkey (lets KWin bring the settings window up).
    #[cfg_attr(windows, allow(dead_code))]
    Activation(String),
    /// The window in front changed: its program names (per-game profiles, games.rs).
    Game(Vec<String>),
    OpenSettings,
    ToggleOverlay,
    Quit,
}

/// All hotkeys in Ev::Hotkey order (1..8).
pub fn hotkeys(s: &crosshair_core::config::Settings) -> [crosshair_core::config::Hotkey; 8] {
    [s.hk_toggle, s.hk_next, s.hk_prev, s.hk_editor, s.hk_nudge_up, s.hk_nudge_down, s.hk_nudge_left, s.hk_nudge_right]
}

#[derive(Default)]
pub struct Out {
    /// The crosshair picture or its placement may have changed.
    pub redraw: bool,
    /// Hotkey bindings (or capture mode) changed: register them again.
    pub hotkeys: bool,
    /// Aim-hide was switched on/off.
    pub aim: bool,
    /// Game rules changed: start / stop the detection.
    pub games: bool,
    pub quit: bool,
}

struct Client {
    id: u64,
    w: Box<dyn Write + Send>,
    ok: bool,
    /// Writing failed (it hung up). Kept until `Gone`, so a message it sent before hanging up (`quit`) still counts.
    dead: bool,
}

pub struct Ctl {
    pub m: Model,
    clients: Vec<Client>,
    token: Option<String>,
    save_at: Option<Instant>,
    start: Instant,
    child: Option<Child>,
    /// Hotkey capture running in the settings window: hotkeys are let go meanwhile.
    pub capture: bool,
    pub aim_held: bool,
    /// Shown to the settings window (`info` message).
    pub platform: String,
    pub monitors: Vec<String>,
    /// Aim-hide status: "ok", "no-permission" (Linux: not in the input group), "none" (no mouse found), "off".
    pub aim_status: String,
    pub gamebar: String,
    /// Test hook `CROSSHAIR_FAKE_AIM=1` (scripts/test-hide-show.sh): the mouse isn't opened, aim-hide counts as
    /// working, and the socket message `test-aim` (body 1/0) stands in for the right button.
    pub fake_aim: bool,
    /// Linux: info `hk_kde` - the portal's keys joined with `|`, or "unavailable". None = not used (Windows).
    pub kde_keys: Option<String>,
    /// Windows: the mouse cursor is showing right now (a game hides it while you play; menus show it).
    pub cursor_shown: bool,
    /// Windows, smart outline (#36): what's behind the centre is bright (Some(true)) / dark; None = not measured.
    pub bright_bg: Option<bool>,
    activation: Option<String>,
    /// What the log said last (log_changes).
    logged: Option<Logged>,
}

/// The state `Ctl::log_changes` compares.
#[derive(Clone, PartialEq)]
struct Logged {
    on: bool,
    mode: u32,
    aim_mode: u32,
    aim_status: String,
    hotkeys: String,
    profile: String,
    by_game: bool,
    gamebar: String,
}

impl Ctl {
    pub fn new(token: Option<String>) -> Ctl {
        let dir = paths::config_dir();
        let _ = std::fs::create_dir_all(&dir);
        Ctl {
            m: Model::new(dir, crosshair_core::load_png),
            clients: Vec::new(),
            token,
            save_at: None,
            start: Instant::now(),
            child: None,
            capture: false,
            aim_held: false,
            platform: String::new(),
            monitors: Vec::new(),
            aim_status: "off".into(),
            gamebar: String::new(),
            fake_aim: std::env::var("CROSSHAIR_FAKE_AIM").is_ok_and(|v| v == "1"),
            kde_keys: None,
            cursor_shown: false,
            bright_bg: None,
            activation: None,
            logged: None,
        }
    }

    pub fn first_run(&self) -> bool {
        !self.m.dir.join("config.ini").exists()
    }

    /// Seconds since start (rainbow animation clock).
    pub fn now(&mut self) -> f64 {
        self.m.time = self.start.elapsed().as_secs_f64();
        self.m.time
    }

    /// The normal overlay window shows the crosshair right now.
    pub fn visible(&self) -> bool {
        let s = &self.m.set;
        // An aim mode only counts while the right button can really be watched ("ok"); otherwise (Linux: no
        // access to the mouse, no mouse found) the crosshair is shown all the time - "only while aiming" would
        // never show it, and the settings window says why it isn't working.
        let aim_works = self.aim_status == "ok";
        s.overlay_on
            && s.display_mode == 0
            && !(s.hide_cursor && self.cursor_shown)
            && match s.aim_mode {
                1 if aim_works => self.aim_held,  // only while aiming
                2 if aim_works => !self.aim_held, // hide while aiming
                _ => true,
            }
    }

    /// The right mouse button has to be watched (aim mode "only while aiming" or "hide while aiming").
    pub fn aim_needed(&self) -> bool {
        self.m.set.aim_mode != 0
    }

    /// The profile as drawn on a monitor `mon_h` physical pixels high: with "size as % of the screen height"
    /// (size_mode 1) the design is made for 1080p and scaled (1440p = 1.33x, 4K = 2x); else unchanged.
    pub fn sized(&self, mon_h: Option<i32>) -> crosshair_core::config::Profile {
        let mut p = self.m.cur.clone();
        // Smart outline (#36, Windows): black on a bright background, white on a dark one.
        if let (true, Some(bright)) = (self.m.set.smart_outline, self.bright_bg) {
            p.outline = true;
            p.outline_color = if bright { crosshair_core::gfx::Color::BLACK } else { crosshair_core::gfx::Color::WHITE };
        }
        if let (1, Some(h)) = (self.m.set.size_mode, mon_h.filter(|&h| h > 0)) {
            let f = (h as f32 / 1080.0).clamp(0.25, 8.0);
            let s0 = p.scale;
            // The thickness that sets the centre (crosshair.rs "primary") keeps its odd / even pixel count, so a
            // design that is exactly centred at 1080p stays exactly centred (an odd line on an even screen can't be).
            let odd = |v: f32| (v.round().max(1.0) as i32) % 2 == 1;
            let t = if p.inner {
                Some(&mut p.inner_thick)
            } else if p.outer {
                Some(&mut p.outer_thick)
            } else if p.dot {
                Some(&mut p.dot_size)
            } else if p.circle {
                Some(&mut p.circle_thick)
            } else {
                None
            };
            if let Some(t) = t {
                let exact = *t * s0 * f;
                let mut n = exact.round().max(1.0);
                if odd(n) != odd(*t * s0) {
                    n = if exact >= n { n + 1.0 } else { (n - 1.0).max(if odd(*t * s0) { 1.0 } else { 2.0 }) };
                }
                *t = n / (s0 * f);
            }
            p.scale = s0 * f;
            p.img_scale *= f;
        }
        p
    }

    pub fn animated(&self) -> bool {
        self.visible() && self.m.cur.animated()
    }

    pub fn handle(&mut self, ev: Ev) -> Out {
        let mut out = Out::default();
        match ev {
            Ev::Connected(id, w) => {
                let ok = self.token.is_none();
                self.clients.push(Client { id, w, ok, dead: false });
                if ok {
                    self.greet(id);
                }
            }
            Ev::Gone(id) => {
                self.clients.retain(|c| c.id != id);
                if self.capture && self.clients.is_empty() {
                    self.capture = false;
                    out.hotkeys = true;
                }
            }
            Ev::Msg(id, kind, body) => {
                let Some(c) = self.clients.iter_mut().find(|c| c.id == id) else { return out };
                if !c.ok {
                    if kind == "hello" && Some(body.trim()) == self.token.as_deref() {
                        c.ok = true;
                        self.greet(id);
                    }
                    return out;
                }
                match kind.as_str() {
                    "state" => {
                        let old = self.m.set.clone();
                        if self.m.apply_text(&body) {
                            self.dirty();
                            out.redraw = true;
                            let s = &self.m.set;
                            out.hotkeys = hotkeys(s) != hotkeys(&old)
                                || s.hk_zoom != old.hk_zoom
                                || s.autostart != old.autostart
                                || s.eco != old.eco;
                            out.aim = (s.aim_mode, s.aim_button, s.aim_vk) != (old.aim_mode, old.aim_button, old.aim_vk);
                            out.games = (s.games.as_str(), s.games_on) != (old.games.as_str(), old.games_on);
                            // Another settings window (rare) gets the change too.
                            self.broadcast("state", &self.m.to_text(), Some(id));
                        }
                    }
                    "capture" => {
                        self.capture = body.trim() == "1";
                        out.hotkeys = true;
                    }
                    "open" => self.open_settings(),
                    "test-aim" if self.fake_aim => return self.handle(Ev::Aim(body.trim() == "1")),
                    "quit" => out.quit = true,
                    _ => {}
                }
            }
            Ev::Aim(held) => {
                if held != self.aim_held {
                    self.aim_held = held;
                    out.redraw = self.m.set.aim_mode != 0;
                }
            }
            // While the settings window captures a new key, a hotkey must not fire (Windows lets them go then).
            Ev::Hotkey(_) if self.capture => {}
            Ev::KdeKeys(k) => {
                self.kde_keys = Some(k.map_or_else(|| "unavailable".into(), |v| v.join("|")));
                self.send_info();
            }
            Ev::Activation(t) => self.activation = Some(t),
            Ev::Game(seen) => {
                if self.m.game_focus(&seen) {
                    self.changed_here();
                    out.redraw = true;
                }
            }
            Ev::Hotkey(n) => {
                match n {
                    1 => self.m.set.overlay_on = !self.m.set.overlay_on,
                    2 => self.m.cycle_profile(1),
                    3 => self.m.cycle_profile(-1),
                    4 => self.open_settings(),
                    5..=8 => {
                        let s = &mut self.m.set;
                        let (dx, dy) = [(0.0, -1.0), (0.0, 1.0), (-1.0, 0.0), (1.0, 0.0)][n as usize - 5];
                        s.offset_x = (s.offset_x + dx).clamp(-500.0, 500.0);
                        s.offset_y = (s.offset_y + dy).clamp(-500.0, 500.0);
                    }
                    _ => {}
                }
                if n != 4 {
                    self.changed_here();
                    out.redraw = true;
                }
            }
            Ev::ToggleOverlay => {
                self.m.set.overlay_on = !self.m.set.overlay_on;
                self.changed_here();
                out.redraw = true;
            }
            Ev::OpenSettings => self.open_settings(),
            Ev::Quit => out.quit = true,
        }
        out
    }

    /// The overlay itself changed the state (hotkey, tray): save + tell the settings window.
    pub fn changed_here(&mut self) {
        self.dirty();
        let text = self.m.to_text();
        self.broadcast("state", &text, None);
    }

    fn dirty(&mut self) {
        if self.save_at.is_none() {
            self.save_at = Some(Instant::now() + Duration::from_secs(1));
        }
    }

    fn greet(&mut self, id: u64) {
        let text = self.m.to_text();
        let info = self.info_text();
        if let Some(c) = self.clients.iter_mut().find(|c| c.id == id) {
            let ok = ipc::write_msg(&mut c.w, "state", &text).and_then(|_| ipc::write_msg(&mut c.w, "info", &info));
            c.dead |= ok.is_err();
        }
    }

    fn info_text(&self) -> String {
        let hk: Vec<&str> = self.m.hk_ok.iter().map(|&b| if b { "1" } else { "0" }).collect();
        format!(
            "version={}\nplatform={}\nmonitors={}\nhk_ok={}\naim={}\ngamebar={}\n",
            crosshair_core::VERSION,
            self.platform,
            self.monitors.join("|"),
            hk.join(","),
            self.aim_status,
            self.gamebar
        ) + &self.kde_keys.as_ref().map_or(String::new(), |k| format!("hk_kde={k}\n"))
    }

    /// Platform facts changed (monitors, hotkey results, aim status): tell the settings window.
    pub fn send_info(&mut self) {
        let info = self.info_text();
        self.broadcast("info", &info, None);
    }

    fn broadcast(&mut self, kind: &str, body: &str, skip: Option<u64>) {
        for c in self.clients.iter_mut().filter(|c| c.ok && !c.dead && Some(c.id) != skip) {
            c.dead = ipc::write_msg(&mut c.w, kind, body).is_err();
        }
    }

    pub fn open_settings(&mut self) {
        // Already open (connected) -> it brings itself to the front.
        if self.clients.iter().any(|c| c.ok && !c.dead) {
            self.broadcast("raise", "", None);
            return;
        }
        // Started a moment ago and not connected yet -> don't start a second one.
        if let Some(ch) = &mut self.child {
            if matches!(ch.try_wait(), Ok(None)) {
                return;
            }
        }
        let mut cmd = std::process::Command::new(paths::sibling_exe(crosshair_core::SETTINGS_EXE));
        if let Some(t) = self.activation.take() {
            cmd.env("XDG_ACTIVATION_TOKEN", t); // winit uses it for the first window
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0000_0020); // NORMAL_PRIORITY_CLASS: not the overlay's idle priority (Efficiency mode)
        }
        match cmd.spawn() {
            Ok(ch) => self.child = Some(ch),
            Err(e) => eprintln!("aimdot: could not start the settings window: {e}"),
        }
    }

    /// Saves when due. Returns how long the platform loop may sleep before calling again.
    /// One log line per real change: crosshair on/off, normal overlay <-> Game Bar mode, aim mode + whether the
    /// mouse can be watched, which hotkeys got registered, profile switches (by hand / by a game rule), Game Bar
    /// widget installed / missing. Hide-while-aiming itself is NOT logged (it would be a line per right click).
    /// Cheap (a few string compares); called from housekeeping(), so nothing is written while nothing changes.
    pub fn log_changes(&mut self) {
        let s = &self.m.set;
        let keys = hotkeys(s);
        let hk: Vec<String> = keys
            .iter()
            .zip(self.m.hk_ok)
            .filter(|(k, _)| k.vk != 0)
            .map(|(k, ok)| if ok { k.name() } else { format!("{} FAILED", k.name()) })
            .collect();
        let now = Logged {
            on: s.overlay_on,
            mode: s.display_mode,
            aim_mode: s.aim_mode,
            aim_status: self.aim_status.clone(),
            hotkeys: if self.capture { "let go (the settings window is capturing a key)".into() } else { hk.join(", ") },
            profile: s.active_profile.clone(),
            by_game: self.m.game_auto.is_some(),
            gamebar: self.gamebar.clone(),
        };
        let old = self.logged.replace(now.clone());
        let o = old.as_ref();
        if o.map_or(true, |o| o.on != now.on) {
            log::line("window", if now.on { "crosshair on" } else { "crosshair off (hidden by the user)" });
        }
        if o.map_or(true, |o| o.mode != now.mode) {
            log::line("mode", if now.mode == 1 { "Game Bar mode - the Aimdot widget draws the crosshair, the overlay window stays hidden" } else { "normal overlay window" });
        }
        if o.map_or(true, |o| o.aim_mode != now.aim_mode || o.aim_status != now.aim_status) {
            let m = ["always", "only while aiming", "hide while aiming"].get(now.aim_mode as usize).copied().unwrap_or("?");
            log::line("aim", &format!("{m} (right button: {})", now.aim_status));
        }
        if o.map_or(true, |o| o.hotkeys != now.hotkeys) {
            log::line("hotkey", &format!("registered: {}", if now.hotkeys.is_empty() { "none" } else { &now.hotkeys }));
        }
        if o.map_or(true, |o| o.profile != now.profile || o.by_game != now.by_game) {
            let name = if now.profile.is_empty() { "(unsaved crosshair)" } else { &now.profile };
            log::line("profile", &format!("\"{name}\"{}", if now.by_game { " (game rule)" } else { "" }));
        }
        if o.map_or(true, |o| o.gamebar != now.gamebar) && !now.gamebar.is_empty() {
            log::line("gamebar", &format!("widget {}", now.gamebar));
        }
    }

    pub fn housekeeping(&mut self) -> Option<Duration> {
        self.log_changes();
        if let Some(t) = self.save_at {
            let now = Instant::now();
            if now >= t {
                self.m.save();
                self.save_at = None;
            } else {
                return Some(t - now);
            }
        }
        // Reap a finished settings window.
        if let Some(ch) = &mut self.child {
            if !matches!(ch.try_wait(), Ok(None)) {
                self.child = None;
            }
        }
        None
    }

    pub fn shutdown(&mut self) {
        self.log_changes();
        self.m.save();
        self.broadcast("bye", "", None);
    }
}
