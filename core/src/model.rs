//! The app model shared by the overlay and the settings window: settings, the current crosshair,
//! saved profiles, presets and the image library (the first version's app model, without its editor UI).

use crate::config::{self, Profile, Settings};
use crate::crosshair::{self, Loader, Renderer};
use crate::gfx::{Canvas, Color};
use crate::{games, share};
use std::path::{Path, PathBuf};

pub const IMAGE_EXTS: [&str; 6] = ["png", "jpg", "jpeg", "bmp", "gif", "tif"];

pub struct Model {
    pub set: Settings,
    pub cur: Profile,
    pub profiles: Vec<(String, Profile)>,
    pub presets: Vec<(&'static str, Profile)>,
    pub rend: Renderer,
    pub images: Vec<String>,
    pub dir: PathBuf,
    /// Number of monitors reported by the platform.
    pub monitors: u32,
    pub time: f64,
    pub toast: String,
    pub toast_until: f64,
    pub name_buf: String,
    /// False when the platform could not register that hotkey (already taken by another app).
    /// Per hotkey (toggle, next, prev, settings, nudge up / down / left / right): registered fine.
    pub hk_ok: [bool; 8],
    /// Per-game switch: what to go back to when the game leaves the front (overlay only).
    pub game_auto: Option<games::Auto>,
    last_saved: String,
}

impl Model {
    pub fn new(dir: PathBuf, loader: Loader) -> Model {
        let img_dir = dir.join("images");
        let _ = std::fs::create_dir_all(&img_dir);
        let text = std::fs::read_to_string(dir.join("config.ini")).unwrap_or_default();
        let (set, cur, profiles) = config::parse(&text);
        let mut m = Model {
            name_buf: set.active_profile.clone(),
            set,
            cur,
            profiles,
            presets: crosshair::presets(),
            rend: Renderer::new(img_dir, loader),
            images: Vec::new(),
            dir,
            monitors: 1,
            time: 0.0,
            toast: String::new(),
            toast_until: 0.0,
            hk_ok: [true; 8],
            game_auto: None,
            last_saved: text,
        };
        m.scan_images();
        m
    }

    pub fn scan_images(&mut self) {
        let mut v: Vec<String> = std::fs::read_dir(&self.rend.img_dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|n| {
                        let ext = n.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
                        IMAGE_EXTS.contains(&ext.as_str())
                    })
                    .collect()
            })
            .unwrap_or_default();
        v.sort_by_key(|a| a.to_lowercase());
        self.images = v;
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toast = msg.into();
        self.toast_until = self.time + 2.6;
    }

    /// Writes the config if anything changed since the last save.
    pub fn save(&mut self) {
        let s = config::serialize(&self.set, &self.cur, &self.profiles);
        if s == self.last_saved {
            return;
        }
        let path = self.dir.join("config.ini");
        let tmp = self.dir.join("config.ini.tmp");
        if std::fs::write(&tmp, &s).and_then(|_| std::fs::rename(&tmp, &path)).is_ok() {
            self.last_saved = s;
        }
    }

    /// Copies an image into the library and selects it as the crosshair.
    pub fn import_image(&mut self, src: &Path) -> bool {
        let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if !IMAGE_EXTS.contains(&ext.as_str()) {
            self.toast("Unsupported file type — use PNG, JPG, BMP or GIF");
            return false;
        }
        let stem: String = src
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("image")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ' '))
            .take(40)
            .collect();
        let stem = if stem.trim().is_empty() { "image".to_string() } else { stem.trim().to_string() };
        let mut name = format!("{stem}.{ext}");
        let mut n = 2;
        while self.rend.img_dir.join(&name).exists() {
            name = format!("{stem} ({n}).{ext}");
            n += 1;
        }
        let dst = self.rend.img_dir.join(&name);
        if std::fs::copy(src, &dst).is_err() {
            self.toast("Could not copy that file");
            return false;
        }
        if self.rend.image(&name).is_none() {
            let _ = std::fs::remove_file(&dst);
            self.rend.forget(&name);
            self.toast("That image could not be read");
            return false;
        }
        self.scan_images();
        self.select_image(&name);
        self.toast(format!("Imported {name}"));
        true
    }

    pub fn select_image(&mut self, name: &str) {
        self.cur.mode = 1;
        if self.cur.image != name {
            self.cur.image = name.to_string();
            // Start large images at a sensible on-screen size.
            if let Some(img) = self.rend.image(name) {
                let big = img.w.max(img.h) as f32;
                self.cur.img_scale = if big > 96.0 { ((64.0 / big) * 20.0).round().max(1.0) / 20.0 } else { 1.0 };
            }
        }
    }

    pub fn delete_image(&mut self, name: &str) {
        let _ = std::fs::remove_file(self.rend.img_dir.join(name));
        self.rend.forget(name);
        if self.cur.image == name {
            self.cur.image.clear();
            self.cur.mode = 0;
        }
        self.scan_images();
    }

    pub fn save_profile(&mut self, name: &str) {
        let name = share::clean_name(name);
        if name.is_empty() {
            self.toast("Give the profile a name first");
            return;
        }
        match self.profiles.iter_mut().find(|p| p.0.eq_ignore_ascii_case(&name)) {
            Some(p) => p.1 = self.cur.clone(),
            None => self.profiles.push((name.clone(), self.cur.clone())),
        }
        self.set.active_profile = name.clone();
        self.name_buf = name.clone();
        self.toast(format!("Saved \"{name}\""));
    }

    /// `name`, or `name (2)`, `name (3)` ... when a profile of that name exists (case doesn't count).
    pub fn free_profile_name(&self, name: &str) -> String {
        let base = share::clean_name(name);
        let base = if base.is_empty() { "Crosshair".to_string() } else { base };
        let taken = |n: &str| self.profiles.iter().any(|p| p.0.eq_ignore_ascii_case(n));
        let mut n = base.clone();
        let mut i = 2;
        while taken(&n) {
            let tail = format!(" ({i})");
            let mut head = base.clone();
            while head.len() + tail.len() > share::MAX_NAME {
                head.pop();
            }
            n = format!("{}{tail}", head.trim_end());
            i += 1;
        }
        n
    }

    /// Adds a profile under a free name (pasted code, import). Returns its index. Doesn't switch to it.
    pub fn add_profile(&mut self, name: &str, p: Profile) -> Option<usize> {
        if self.profiles.len() >= 200 {
            self.toast("That's 200 profiles - delete some first");
            return None;
        }
        let name = self.free_profile_name(name);
        self.profiles.push((name, p));
        Some(self.profiles.len() - 1)
    }

    /// Renames a profile (and its game rules). Err = a message for the user.
    pub fn rename_profile(&mut self, i: usize, new: &str) -> Result<String, String> {
        let new = share::clean_name(new);
        if new.is_empty() {
            return Err("Give the profile a name first".into());
        }
        let Some(old) = self.profiles.get(i).map(|p| p.0.clone()) else { return Err("That profile is gone".into()) };
        if self.profiles.iter().enumerate().any(|(j, p)| j != i && p.0.eq_ignore_ascii_case(&new)) {
            return Err(format!("There is already a profile called \"{new}\""));
        }
        self.profiles[i].0 = new.clone();
        if self.set.active_profile == old {
            self.set.active_profile = new.clone();
            self.name_buf = new.clone();
        }
        let mut rules = games::parse(&self.set.games);
        for r in rules.iter_mut().filter(|r| r.profile == old) {
            r.profile = new.clone();
        }
        self.set.games = games::format(&rules);
        Ok(new)
    }

    /// Deletes a profile and its game rules. The crosshair on screen stays as it is.
    pub fn delete_profile(&mut self, i: usize) {
        if i >= self.profiles.len() {
            return;
        }
        let (name, _) = self.profiles.remove(i);
        if self.set.active_profile == name {
            self.set.active_profile.clear();
        }
        let rules: Vec<_> = games::parse(&self.set.games).into_iter().filter(|r| r.profile != name).collect();
        self.set.games = games::format(&rules);
    }

    /// Moves a profile one place up (-1) or down (+1) in the list (the hotkey order).
    pub fn move_profile(&mut self, i: usize, dir: i32) {
        let j = i as i32 + dir;
        if i < self.profiles.len() && j >= 0 && (j as usize) < self.profiles.len() {
            self.profiles.swap(i, j as usize);
        }
    }

    /// Uses `game` (any form: "cs2.exe", a full path) for profile `profile`; None = remove the game's rule.
    pub fn set_game_rule(&mut self, game: &str, profile: Option<&str>) {
        let key = games::exe_key(game);
        if key.is_empty() {
            return;
        }
        let mut rules: Vec<_> = games::parse(&self.set.games).into_iter().filter(|r| r.game != key).collect();
        if let Some(p) = profile {
            rules.push(games::Rule { game: key, profile: p.to_string() });
        }
        self.set.games = games::format(&rules);
    }

    /// The overlay: true while the per-game switch needs to know what is in front.
    pub fn games_needed(&self) -> bool {
        self.set.games_on && !games::parse(&self.set.games).is_empty()
    }

    /// The window in front changed (`seen` = its program names; empty = nothing known / the desktop).
    /// Switches to that game's profile, or back to the crosshair from before when the game left.
    /// Returns true when the crosshair changed.
    pub fn game_focus(&mut self, seen: &[String]) -> bool {
        let rules = if self.set.games_on { games::parse(&self.set.games) } else { Vec::new() };
        let hit = games::rule_for(&rules, seen).and_then(|r| self.profiles.iter().position(|p| p.0 == r.profile));
        match hit {
            Some(j) => {
                let (name, p) = self.profiles[j].clone();
                if self.set.active_profile == name && self.cur == p {
                    return false;
                }
                let before = self.game_auto.take();
                let (before_name, before) = match before {
                    // switching from one game straight to another: keep the crosshair from before the first one
                    Some(a) => (a.before_name, a.before),
                    None => (self.set.active_profile.clone(), self.cur.clone()),
                };
                self.load_profile(j);
                self.game_auto = Some(games::Auto { before_name, before, loaded: p });
                true
            }
            None => match self.game_auto.take() {
                // back to what it was - unless the crosshair was changed by hand in the meantime
                Some(a) if self.cur == a.loaded => {
                    self.cur = a.before;
                    self.set.active_profile = a.before_name.clone();
                    self.name_buf = a.before_name;
                    true
                }
                _ => false,
            },
        }
    }

    pub fn load_profile(&mut self, i: usize) {
        if let Some((n, p)) = self.profiles.get(i) {
            self.cur = p.clone();
            self.set.active_profile = n.clone();
            self.name_buf = n.clone();
        }
    }

    /// Steps through saved profiles (hotkeys).
    pub fn cycle_profile(&mut self, dir: i32) {
        let n = self.profiles.len() as i32;
        if n == 0 {
            return;
        }
        let at = self.profiles.iter().position(|p| p.0 == self.set.active_profile).map_or(if dir > 0 { -1 } else { 0 }, |i| i as i32);
        self.load_profile((at + dir).rem_euclid(n) as usize);
    }

    pub fn randomize(&mut self) {
        // Small xorshift seeded from the clock; no external RNG needed.
        let mut s = (self.time * 1e6) as u64 ^ 0x9E3779B97F4A7C15 | 1;
        let mut rnd = |a: f32, b: f32| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            a + (s % 10_000) as f32 / 10_000.0 * (b - a)
        };
        let p = &mut self.cur;
        p.mode = 0;
        p.color = Color::from_hsv(rnd(0.0, 1.0), rnd(0.7, 1.0), 1.0);
        p.inner = rnd(0.0, 1.0) > 0.2;
        p.inner_len = rnd(3.0, 12.0).round();
        p.inner_thick = rnd(1.0, 4.0).round();
        p.inner_gap = rnd(0.0, 8.0).round();
        p.dot = rnd(0.0, 1.0) > 0.5 || !p.inner;
        p.dot_size = rnd(2.0, 6.0).round();
        p.dot_round = rnd(0.0, 1.0) > 0.5;
        p.circle = rnd(0.0, 1.0) > 0.7;
        p.circle_radius = rnd(8.0, 20.0).round();
        p.circle_thick = rnd(1.0, 3.0).round();
        p.outer = rnd(0.0, 1.0) > 0.8;
        p.outer_gap = p.inner_gap + p.inner_len + rnd(2.0, 6.0).round();
        p.t_style = rnd(0.0, 1.0) > 0.85;
        p.rotation = if rnd(0.0, 1.0) > 0.8 { 45.0 } else { 0.0 };
        p.outline = rnd(0.0, 1.0) > 0.25;
        p.scale = 1.0;
    }

    /// The whole state as config text (what is saved and what goes over the IPC socket).
    pub fn to_text(&self) -> String {
        config::serialize(&self.set, &self.cur, &self.profiles)
    }

    /// Replaces the state from config text (from the settings window). Returns true if anything changed.
    pub fn apply_text(&mut self, text: &str) -> bool {
        let (set, cur, profiles) = config::parse(text);
        let changed = set != self.set || cur != self.cur || profiles != self.profiles;
        self.set = set;
        self.cur = cur;
        self.profiles = profiles;
        changed
    }

    /// Renders the live crosshair (the overlay's picture). The design centre is the canvas centre.
    pub fn render(&mut self, out: &mut Canvas) {
        self.rend.render(&self.cur, self.time, out);
    }
}

/// 32x32-style app icon (rounded tile with a crosshair), at any size.
pub fn icon(size: i32, accent: Color) -> Canvas {
    let mut c = Canvas::new(size, size);
    let s = size as f32;
    c.rrect(0.0, 0.0, s, s, s * 0.24, Color::hex(0x101216));
    let (m, t, g, l) = (s / 2.0, (s * 0.11).max(2.0).round(), s * 0.12, s * 0.24);
    for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0f32)] {
        let (x0, y0) = (m + dx * g, m + dy * g);
        let (x1, y1) = (m + dx * (g + l), m + dy * (g + l));
        c.rrect(x0.min(x1) - t / 2.0 * dy.abs(), y0.min(y1) - t / 2.0 * dx.abs(), (x1 - x0).abs().max(t), (y1 - y0).abs().max(t), 0.0, accent);
    }
    c
}
