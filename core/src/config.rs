//! Settings + crosshair profile model and a tiny INI-style (de)serialiser.

use crate::gfx::Color;

pub trait Val: Sized {
    fn enc(&self) -> String;
    fn dec(s: &str) -> Option<Self>;
}
impl Val for f32 {
    fn enc(&self) -> String {
        format!("{}", self)
    }
    fn dec(s: &str) -> Option<Self> {
        s.trim().parse::<f32>().ok().filter(|v| v.is_finite())
    }
}
impl Val for u32 {
    fn enc(&self) -> String {
        format!("{}", self)
    }
    fn dec(s: &str) -> Option<Self> {
        s.trim().parse().ok()
    }
}
impl Val for bool {
    fn enc(&self) -> String {
        if *self { "1".into() } else { "0".into() }
    }
    fn dec(s: &str) -> Option<Self> {
        match s.trim() {
            "1" | "true" => Some(true),
            "0" | "false" => Some(false),
            _ => None,
        }
    }
}
impl Val for Color {
    fn enc(&self) -> String {
        self.to_hex()
    }
    fn dec(s: &str) -> Option<Self> {
        Color::parse(s).map(|c| Color { a: 255, ..c })
    }
}
impl Val for String {
    fn enc(&self) -> String {
        self.replace(['\n', '\r'], " ")
    }
    fn dec(s: &str) -> Option<Self> {
        Some(s.to_string())
    }
}

pub const MOD_ALT: u32 = 1;
pub const MOD_CTRL: u32 = 2;
pub const MOD_SHIFT: u32 = 4;
pub const MOD_WIN: u32 = 8;

/// A global hotkey: Win32 modifier flags + virtual-key code. `vk == 0` means unbound.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Hotkey {
    pub mods: u32,
    pub vk: u32,
}
impl Val for Hotkey {
    fn enc(&self) -> String {
        format!("{},{}", self.mods, self.vk)
    }
    fn dec(s: &str) -> Option<Self> {
        let (a, b) = s.split_once(',')?;
        Some(Hotkey { mods: a.trim().parse::<u32>().ok()? & 15, vk: b.trim().parse::<u32>().ok()? & 255 })
    }
}
impl Hotkey {
    pub fn name(&self) -> String {
        if self.vk == 0 {
            return "Not set".into();
        }
        let mut s = String::new();
        for (m, n) in [(MOD_CTRL, "Ctrl + "), (MOD_ALT, "Alt + "), (MOD_SHIFT, "Shift + "), (MOD_WIN, "Win + ")] {
            if self.mods & m != 0 {
                s.push_str(n);
            }
        }
        let v = self.vk;
        let key = match v {
            0x30..=0x39 | 0x41..=0x5A => (v as u8 as char).to_string(),
            0x60..=0x69 => format!("Num {}", v - 0x60),
            0x70..=0x87 => format!("F{}", v - 0x6F),
            0x08 => "Backspace".into(),
            0x09 => "Tab".into(),
            0x0D => "Enter".into(),
            0x13 => "Pause".into(),
            0x14 => "Caps Lock".into(),
            0x20 => "Space".into(),
            0x21 => "Page Up".into(),
            0x22 => "Page Down".into(),
            0x23 => "End".into(),
            0x24 => "Home".into(),
            0x25 => "Left".into(),
            0x26 => "Up".into(),
            0x27 => "Right".into(),
            0x28 => "Down".into(),
            0x2D => "Insert".into(),
            0x2E => "Delete".into(),
            0x6A => "Num *".into(),
            0x6B => "Num +".into(),
            0x6D => "Num -".into(),
            0x6E => "Num .".into(),
            0x6F => "Num /".into(),
            0xBA => ";".into(),
            0xBB => "=".into(),
            0xBC => ",".into(),
            0xBD => "-".into(),
            0xBE => ".".into(),
            0xBF => "/".into(),
            0xC0 => "`".into(),
            0xDB => "[".into(),
            0xDC => "\\".into(),
            0xDD => "]".into(),
            0xDE => "'".into(),
            _ => format!("Key {:02X}", v),
        };
        s + &key
    }
}

macro_rules! kv_struct {
    ($(#[$m:meta])* pub struct $name:ident { $(pub $f:ident : $t:ty = $d:expr),* $(,)? }) => {
        $(#[$m])*
        pub struct $name { $(pub $f: $t),* }
        impl Default for $name {
            fn default() -> Self { Self { $($f: $d),* } }
        }
        impl $name {
            pub fn write(&self, out: &mut String) {
                $( out.push_str(stringify!($f)); out.push('='); out.push_str(&Val::enc(&self.$f)); out.push('\n'); )*
            }
            pub fn set(&mut self, k: &str, v: &str) {
                match k {
                    $( stringify!($f) => { if let Some(x) = <$t as Val>::dec(v) { self.$f = x; } } )*
                    _ => {}
                }
            }
        }
    };
}

kv_struct! {
    /// One complete crosshair: either a designed reticle or an imported image.
    #[derive(Clone, PartialEq, Debug)]
    pub struct Profile {
        pub mode: u32 = 0,                       // 0 = designer, 1 = image
        pub color: Color = Color::hex(0x00FF7F),
        pub opacity: f32 = 1.0,
        pub rainbow: bool = false,
        pub rainbow_mode: u32 = 0,               // 0 cycle, 1 wave, 2 spin
        pub rainbow_speed: f32 = 1.0,
        pub rainbow_sat: f32 = 1.0,
        pub outline: bool = true,
        pub outline_thick: f32 = 1.0,
        pub outline_color: Color = Color::BLACK,
        pub outline_opacity: f32 = 1.0,
        pub dot: bool = false,
        pub dot_size: f32 = 2.0,
        pub dot_round: bool = false,
        pub dot_opacity: f32 = 1.0,
        pub inner: bool = true,
        pub inner_len: f32 = 6.0,
        pub inner_thick: f32 = 2.0,
        pub inner_gap: f32 = 3.0,
        pub inner_opacity: f32 = 1.0,
        pub outer: bool = false,
        pub outer_len: f32 = 3.0,
        pub outer_thick: f32 = 2.0,
        pub outer_gap: f32 = 12.0,
        pub outer_opacity: f32 = 0.6,
        pub circle: bool = false,
        pub circle_radius: f32 = 12.0,
        pub circle_thick: f32 = 2.0,
        pub circle_opacity: f32 = 1.0,
        pub t_style: bool = false,
        pub rotation: f32 = 0.0,
        pub scale: f32 = 1.0,
        pub image: String = String::new(),
        pub img_scale: f32 = 1.0,
        pub img_opacity: f32 = 1.0,
        pub img_tint: bool = false,
        pub img_smooth: bool = true,
    }
}

impl Profile {
    /// Clamp everything into the ranges the UI exposes (protects against hand-edited files).
    pub fn sanitize(&mut self) {
        let c = |v: &mut f32, a: f32, b: f32| *v = if v.is_finite() { v.clamp(a, b) } else { a };
        self.mode = self.mode.min(1);
        self.rainbow_mode = self.rainbow_mode.min(2);
        c(&mut self.opacity, 0.05, 1.0);
        c(&mut self.rainbow_speed, 0.1, 5.0);
        c(&mut self.rainbow_sat, 0.2, 1.0);
        c(&mut self.outline_thick, 1.0, 6.0);
        c(&mut self.outline_opacity, 0.0, 1.0);
        c(&mut self.dot_size, 1.0, 40.0);
        c(&mut self.dot_opacity, 0.0, 1.0);
        c(&mut self.inner_len, 1.0, 60.0);
        c(&mut self.inner_thick, 1.0, 20.0);
        c(&mut self.inner_gap, 0.0, 60.0);
        c(&mut self.inner_opacity, 0.0, 1.0);
        c(&mut self.outer_len, 1.0, 60.0);
        c(&mut self.outer_thick, 1.0, 20.0);
        c(&mut self.outer_gap, 0.0, 100.0);
        c(&mut self.outer_opacity, 0.0, 1.0);
        c(&mut self.circle_radius, 2.0, 100.0);
        c(&mut self.circle_thick, 1.0, 20.0);
        c(&mut self.circle_opacity, 0.0, 1.0);
        c(&mut self.rotation, 0.0, 359.0);
        c(&mut self.scale, 0.5, 4.0);
        c(&mut self.img_scale, 0.05, 4.0);
        c(&mut self.img_opacity, 0.05, 1.0);
    }
    /// True when the rendered output changes over time (RGB rainbow, or an animated picture from the painter).
    pub fn animated(&self) -> bool {
        self.rainbow_animated() || (self.mode == 1 && crate::paint::anim_of(&self.image).is_some())
    }
    /// True when the RGB rainbow changes the colours over time.
    pub fn rainbow_animated(&self) -> bool {
        self.rainbow && (self.mode == 0 || self.img_tint)
    }
}

kv_struct! {
    #[derive(Clone, PartialEq, Debug)]
    pub struct Settings {
        pub overlay_on: bool = true,
        pub monitor: u32 = 0,
        pub monitor_follow: bool = false,        // follow the screen of the focused window (the game) instead
        pub offset_x: f32 = 0.0,
        pub offset_y: f32 = 0.0,
        // Size: 0 = fixed pixels, 1 = % of the screen height (sizes are designed at 1080p and scaled to the monitor)
        pub size_mode: u32 = 0,
        pub hide_ads: bool = false,              // the original key; kept in step with aim_mode == 2 (old configs)
        pub close_to_tray: bool = true,
        pub autostart: bool = false,
        pub start_minimized: bool = false,       // started with the PC: stay in the tray (opening Aimdot yourself always shows the window)
        // Hotkeys: none set on a fresh install (most people don't want keys taken for them); existing configs keep theirs
        pub hk_toggle: Hotkey = Hotkey { mods: 0, vk: 0 },
        pub hk_next: Hotkey = Hotkey { mods: 0, vk: 0 },
        pub hk_prev: Hotkey = Hotkey { mods: 0, vk: 0 },
        pub hk_editor: Hotkey = Hotkey { mods: 0, vk: 0 },
        // Fine centre nudge: moves the offset by 1 px (Ctrl+Alt+Shift+arrows)
        pub hk_nudge_up: Hotkey = Hotkey { mods: 0, vk: 0 },
        pub hk_nudge_down: Hotkey = Hotkey { mods: 0, vk: 0 },
        pub hk_nudge_left: Hotkey = Hotkey { mods: 0, vk: 0 },
        pub hk_nudge_right: Hotkey = Hotkey { mods: 0, vk: 0 },
        pub theme: u32 = 0,
        pub accent: Color = Color::hex(0x2DE2A6),
        pub accent_rainbow: bool = false,
        pub radius: f32 = 10.0,
        pub ui_scale: u32 = 1,                   // index into UI_SCALES
        pub animations: bool = true,
        pub preview_bg: u32 = 3,
        pub preview_zoom: u32 = 1,
        pub preview_img: String = String::new(), // the settings preview's own background picture (file in backgrounds/), "" = preview_bg
        pub page: u32 = 0,
        pub active_profile: String = String::new(),
        // New in Aimdot (not in the first version):
        pub display_mode: u32 = 0,               // 0 = normal overlay, 1 = Game Bar mode (Windows)
        pub eco: bool = true,                    // Windows: efficiency mode (EcoQoS + low priority)
        pub seen_anticheat: bool = false,        // the anti-cheat note was shown once
        pub aim_mode: u32 = 0,                   // 0 = always shown, 1 = only while aiming (right button held), 2 = hide while aiming
        pub hide_cursor: bool = false,           // Windows: hide while the mouse cursor shows (game menus, desktop)
        pub show_fade: bool = false,             // Show / hide: false = Instant (default), true = Fade
        pub smart_outline: bool = false,         // Windows: the outline turns black / white by what's behind the centre
        pub hk_zoom: Hotkey = Hotkey { mods: 0, vk: 0 }, // Windows: hold to zoom around the centre (no key by default)
        pub zoom_level: f32 = 2.0,               // 1.5x - 6x (2 / 3 / 4 in the settings; the wheel while zoomed: 0.5 steps)
        pub zoom_scroll: bool = false,           // Windows: the mouse wheel changes the zoom while the zoom key is held
        pub zoom_size: f32 = 22.0,               // lens size, % of the screen height (22 % = about 6 cm on a 24" screen)
        pub zoom_round: bool = true,             // lens shape: round (default) or square
        pub zoom_ring: Color = Color::WHITE,     // the 2 px ring at the lens' edge (white by default, user, 11 Nov 2026)
        pub zoom_toggle: bool = false,           // zoom key: false = hold (default), true = press to start / again to stop
        pub zoom_aim_only: bool = false,         // the zoom key works only while the "Aim with" button is held
        // Aim with: 0 right mouse (default), 1 middle, 2 mouse 4 (back), 3 mouse 5 (forward), 4 a keyboard key (aim_vk)
        pub aim_button: u32 = 0,
        pub aim_vk: u32 = 0,
        // The settings window's look (Echo's themes). Only the settings window reads these; the overlay just keeps them.
        pub ui_theme: String = String::new(),    // "" = Aimdot's own coral, else a theme id ("ocean", "light", "system", "custom", …)
        pub ui_accent: Color = Color::hex(0xEF6A4C), // "custom" theme: accent
        pub ui_bg: Color = Color::hex(0x0D0D10),     // "custom" theme: background
        pub ui_accent_rgb: bool = false,         // "custom" theme: the accent cycles through the rainbow (RGB, Echo's)
        pub ui_bg_rgb: bool = false,             // "custom" theme: the background too
        // Per-game profiles (core/src/games.rs): "game=Profile|game=Profile"; "" = no rules, no detection at all
        pub games: String = String::new(),
        pub games_on: bool = true,               // the per-game switch as a whole (rules are kept when off)
    }
}

pub const UI_SCALES: [f32; 5] = [0.9, 1.0, 1.15, 1.3, 1.5];

impl Settings {
    pub fn sanitize(&mut self) {
        self.monitor = self.monitor.min(7);
        self.offset_x = if self.offset_x.is_finite() { self.offset_x.clamp(-500.0, 500.0).round() } else { 0.0 };
        self.offset_y = if self.offset_y.is_finite() { self.offset_y.clamp(-500.0, 500.0).round() } else { 0.0 };
        self.size_mode = self.size_mode.min(1);
        self.zoom_level = if self.zoom_level.is_finite() { self.zoom_level.clamp(1.5, 6.0) } else { 2.0 };
        self.zoom_size = if self.zoom_size.is_finite() { self.zoom_size.clamp(15.0, 45.0) } else { 22.0 };
        self.aim_button = if self.aim_button == 4 && self.aim_vk == 0 { 0 } else { self.aim_button.min(4) };
        self.theme = self.theme.min(3);
        self.radius = if self.radius.is_finite() { self.radius.clamp(0.0, 16.0) } else { 10.0 };
        self.ui_scale = self.ui_scale.min(UI_SCALES.len() as u32 - 1);
        self.preview_bg = self.preview_bg.min(16); // the settings window's test-range backgrounds (settings/src/range.rs)
        self.preview_zoom = self.preview_zoom.min(2);
        self.page = self.page.min(7);
        self.display_mode = self.display_mode.min(1);
        // Old configs only have hide_ads: that means "hide while aiming".
        self.aim_mode = self.aim_mode.min(2);
        if self.hide_ads && self.aim_mode == 0 {
            self.aim_mode = 2;
        }
        self.hide_ads = self.aim_mode == 2;
    }
}

pub fn serialize(set: &Settings, cur: &Profile, profiles: &[(String, Profile)]) -> String {
    let mut s = String::with_capacity(4096);
    s.push_str("; Aimdot configuration\n[app]\n");
    set.write(&mut s);
    s.push_str("\n[current]\n");
    cur.write(&mut s);
    for (name, p) in profiles {
        s.push_str("\n[profile:");
        s.push_str(&name.enc());
        s.push_str("]\n");
        p.write(&mut s);
    }
    s
}

pub fn parse(text: &str) -> (Settings, Profile, Vec<(String, Profile)>) {
    enum Sec {
        None,
        App,
        Cur,
        Prof(usize),
    }
    let mut set = Settings::default();
    let mut cur = Profile::default();
    let mut profiles: Vec<(String, Profile)> = Vec::new();
    let mut sec = Sec::None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        let t = line.trim();
        if t.is_empty() || t.starts_with(';') {
            continue;
        }
        if t.starts_with('[') && t.ends_with(']') {
            let name = &t[1..t.len() - 1];
            sec = match name {
                "app" => Sec::App,
                "current" => Sec::Cur,
                _ => match name.strip_prefix("profile:") {
                    Some(n) if !n.trim().is_empty() && profiles.len() < 200 => {
                        profiles.push((n.trim().to_string(), Profile::default()));
                        Sec::Prof(profiles.len() - 1)
                    }
                    _ => Sec::None,
                },
            };
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        match sec {
            Sec::App => set.set(k.trim(), v),
            Sec::Cur => cur.set(k.trim(), v),
            Sec::Prof(i) => profiles[i].1.set(k.trim(), v),
            Sec::None => {}
        }
    }
    set.sanitize();
    cur.sanitize();
    for (_, p) in &mut profiles {
        p.sanitize();
    }
    (set, cur, profiles)
}
