//! Echo's themes for the settings window (copied from Echo's prototype-native/src/settings.rs: THEMES,
//! themeVars, theme cards). Aimdot's own look - coral #ef6a4c - is the default (id ""); Echo's violet is "violet".
//! Saved in config.ini [app] as ui_theme / ui_accent / ui_bg (only this window reads them).
use crate::{AppWindow, ThemeCard, ThemeLine, T};
use slint::{Color, ComponentHandle, ModelRc, VecModel};

pub struct Palette {
    pub bg: Color,
    pub rail: Color,
    pub panel: Color,
    pub panel2: Color,
    pub line: Color,
    pub hover: Color,
    pub text: Color,
    pub muted: Color,
    pub dim: Color,
    pub accent: Color,
    pub accent2: Color,
    pub grad: [Color; 3],
    pub accent_hex: String,
    pub bg_hex: String,
}

/// (id, name, accent, accent2, gradient, background) - Echo's list, Aimdot's coral first and in Light
const THEMES: [(&str, &str, &str, &str, [&str; 3], &str); 20] = [
    ("", "Coral", "#ef6a4c", "#f49782", ["#ff8f6b", "#ef6a4c", "#e0485e"], "#0d0d10"),
    ("violet", "Violet", "#7c5cff", "#9d85ff", ["#8b6cff", "#6a4cf5", "#4f7bff"], "#0d0d10"),
    ("ocean", "Ocean", "#2fa8e0", "#7cc9f0", ["#4cc3f0", "#2f8fe0", "#3b5cf0"], "#0b0e12"),
    ("ember", "Ember", "#f07a1a", "#ffa154", ["#ffa154", "#f07a1a", "#e0484a"], "#100d0b"),
    ("emerald", "Emerald", "#2fbf86", "#6fd9ab", ["#6fd9ab", "#2fbf86", "#1d9a9a"], "#0b100e"),
    ("crimson", "Crimson", "#e03232", "#e97070", ["#e55151", "#e03232", "#a82626"], "#0d0d10"),
    ("rose", "Rosé", "#e0609e", "#f29ac4", ["#f29ac4", "#e0609e", "#9b5be0"], "#100c0f"),
    ("slate", "Slate", "#8a96a8", "#c3ccd8", ["#c3ccd8", "#8a96a8", "#5d6a7c"], "#0c0d0f"),
    ("frost", "Frost", "#4fb8f0", "#9ad8f8", ["#bfe8ff", "#4fb8f0", "#5a7cf0"], "#0a0e12"),
    ("lime", "Lime", "#8fcf3c", "#b8e57e", ["#c5ef8a", "#8fcf3c", "#2fae6f"], "#0c0f0a"),
    ("gold", "Gold", "#e0a93a", "#f0cb7a", ["#f5d68a", "#e0a93a", "#c06a2a"], "#100e0a"),
    ("midnight", "Midnight", "#6a6ff0", "#a3a6fa", ["#a3a6fa", "#6a6ff0", "#3a3fb8"], "#08080f"),
    ("christmas", "Christmas", "#e0484a", "#f08a8a", ["#f08a8a", "#e0484a", "#2f9e5a"], "#0a100c"),
    ("autumn", "Autumn", "#e07a2a", "#f0a868", ["#f5c27a", "#e07a2a", "#a8402a"], "#110d09"),
    ("halloween", "Halloween", "#f08a1a", "#ffb15a", ["#ffb15a", "#f08a1a", "#8a3ae0"], "#0e0a12"),
    ("winter", "Winter", "#7cc4ec", "#b8e2f8", ["#e2f4ff", "#7cc4ec", "#5a86d8"], "#0a0d12"),
    ("rain", "Rain", "#6f9bd1", "#a3c2e8", ["#c4d8f0", "#6f9bd1", "#4a6a9a"], "#0d1015"),
    ("light", "Light", "#e0583a", "#ef6a4c", ["#ff8f6b", "#e0583a", "#d0405a"], "#f4f4f7"),
    ("paper", "Paper", "#c0602a", "#d9844a", ["#e8a06a", "#c0602a", "#9a3f2a"], "#f6f2ea"),
    ("system", "Match system", "", "", ["", "", ""], ""),
];

fn parse_hex(h: &str) -> Option<(f32, f32, f32)> {
    let h = h.trim().trim_start_matches('#');
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(h, 16).ok()?;
    Some((((n >> 16) & 255) as f32, ((n >> 8) & 255) as f32, (n & 255) as f32))
}
fn hex((r, g, b): (f32, f32, f32)) -> String {
    format!("#{:02x}{:02x}{:02x}", r.round().clamp(0.0, 255.0) as u8, g.round().clamp(0.0, 255.0) as u8, b.round().clamp(0.0, 255.0) as u8)
}
fn mix(a: &str, b: &str, t: f32) -> String {
    let (x, y) = (parse_hex(a).unwrap_or((0.0, 0.0, 0.0)), parse_hex(b).unwrap_or((0.0, 0.0, 0.0)));
    hex((x.0 + (y.0 - x.0) * t, x.1 + (y.1 - x.1) * t, x.2 + (y.2 - x.2) * t))
}
fn lum(h: &str) -> f32 {
    let (r, g, b) = parse_hex(h).unwrap_or((0.0, 0.0, 0.0));
    (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
}
fn col(h: &str) -> Color {
    let (r, g, b) = parse_hex(h).unwrap_or((0.0, 0.0, 0.0));
    Color::from_rgb_u8(r as u8, g as u8, b as u8)
}

/// Echo's themeVars(): the whole palette from a theme id (+ the custom colours)
pub fn palette(id: &str, accent: &str, bg: &str) -> Palette {
    // "Match system": the desktop's light / dark setting -> Light / Coral
    let id = if id == "system" { if system_light() { "light" } else { "" } } else { id };
    let t = THEMES.iter().find(|t| t.0 == id && id != "system");
    let (a, a2, g, b) = match t {
        Some(t) if id != "custom" => (t.2.to_string(), t.3.to_string(), [t.4[0].to_string(), t.4[1].to_string(), t.4[2].to_string()], t.5.to_string()),
        _ => {
            let a = if parse_hex(accent).is_some() { accent.to_string() } else { THEMES[0].2.into() };
            let mut b = if parse_hex(bg).is_some() { bg.to_string() } else { "#0d0d10".into() };
            // Echo: a custom background can be dark or light, nothing in between (text must stay readable)
            if lum(&b) <= 0.5 {
                for _ in 0..12 {
                    if lum(&b) <= 0.16 {
                        break;
                    }
                    b = mix(&b, "#000000", 0.2);
                }
            }
            (a.clone(), mix(&a, "#ffffff", 0.3), [mix(&a, "#ffffff", 0.15), a.clone(), mix(&a, "#000000", 0.25)], b)
        }
    };
    let light = lum(&b) > 0.5;
    let (rail, panel, panel2, line, hover, text, muted, dim) = if light {
        (mix(&b, "#000000", 0.035), "#ffffff".into(), mix(&b, "#000000", 0.045), mix(&b, "#000000", 0.11), mix(&b, "#000000", 0.18), "#16161c", "#55555f", "#86868f")
    } else {
        (mix(&b, "#000000", 0.22), mix(&b, "#ffffff", 0.05), mix(&b, "#ffffff", 0.085), mix(&b, "#ffffff", 0.13), mix(&b, "#ffffff", 0.2), "#f5f5f7", "#a0a0ab", "#6b6b76")
    };
    Palette {
        bg: col(&b),
        rail: col(&rail),
        panel: col(&panel),
        panel2: col(&panel2),
        line: col(&line),
        hover: col(&hover),
        text: col(text),
        muted: col(muted),
        dim: col(dim),
        accent: col(&a),
        accent2: col(&a2),
        grad: [col(&g[0]), col(&g[1]), col(&g[2])],
        accent_hex: a,
        bg_hex: b,
    }
}

/// Puts the palette into the window's T global (Echo's apply_theme)
pub fn apply(ui: &AppWindow, p: &Palette) {
    let t = ui.global::<T>();
    t.set_bg(p.bg);
    t.set_rail(p.rail);
    t.set_panel(p.panel);
    t.set_panel2(p.panel2);
    t.set_line(p.line);
    t.set_hover(p.hover);
    t.set_text(p.text);
    t.set_muted(p.muted);
    t.set_dim(p.dim);
    t.set_accent(p.accent);
    t.set_accent2(p.accent2);
    t.set_g0(p.grad[0]);
    t.set_g1(p.grad[1]);
    t.set_g2(p.grad[2]);
}

/// The theme cards, 4 per line (Echo's .themegrid)
pub fn lines(cur: &str, accent: &str, bg: &str) -> ModelRc<ThemeLine> {
    let mut all: Vec<(&str, &str)> = THEMES.iter().map(|t| (t.0, t.1)).collect();
    all.push(("custom", "Custom"));
    let cards: Vec<ThemeCard> = all
        .iter()
        .map(|(id, name)| {
            let p = palette(id, accent, bg);
            ThemeCard { id: (*id).into(), name: (*name).into(), on: *id == cur, bg: p.bg, rail: p.rail, panel: p.panel, accent: p.accent, text: p.text }
        })
        .collect();
    let lines: Vec<ThemeLine> = cards.chunks(4).map(|c| ThemeLine { items: ModelRc::new(VecModel::from(c.to_vec())) }).collect();
    ModelRc::new(VecModel::from(lines))
}

/// Is a theme id one of ours? (an unknown id from a newer version falls back to the default)
pub fn known(id: &str) -> bool {
    id == "custom" || THEMES.iter().any(|t| t.0 == id)
}

/// Does the desktop use a light theme? Asked once per window (the settings window is short-lived).
/// Only called for "Match system". Test copies (CROSSHAIR_HOME) never ask: no portal / D-Bus calls in tests.
pub fn system_light() -> bool {
    use std::sync::OnceLock;
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| std::env::var_os("CROSSHAIR_HOME").is_none() && ask_light())
}

#[cfg(windows)]
fn ask_light() -> bool {
    // HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\AppsUseLightTheme (DWORD) ⚠ untested
    #[link(name = "advapi32")]
    extern "system" {
        fn RegGetValueW(hkey: isize, sub: *const u16, val: *const u16, flags: u32, ty: *mut u32, data: *mut u32, size: *mut u32) -> i32;
    }
    let w = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (k, n) = (w(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"), w("AppsUseLightTheme"));
    let (mut v, mut sz) = (0u32, 4u32);
    // HKEY_CURRENT_USER, RRF_RT_REG_DWORD
    unsafe { RegGetValueW(0x8000_0001u32 as i32 as isize, k.as_ptr(), n.as_ptr(), 0x10, std::ptr::null_mut(), &mut v, &mut sz) == 0 && v == 1 }
}

#[cfg(not(windows))]
fn ask_light() -> bool {
    // Echo's order: the desktop portal (KDE, GNOME, …; color-scheme 1 = dark, 2 = light, 0 = no preference),
    // then GNOME's gsettings, then KDE's colour scheme name
    let out = |c: &str, a: &[&str]| {
        std::process::Command::new(c)
            .args(a)
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase())
            .unwrap_or_default()
    };
    for m in ["ReadOne", "Read"] {
        let p = out(
            "dbus-send",
            &[
                "--session",
                "--print-reply=literal",
                "--reply-timeout=1500",
                "--dest=org.freedesktop.portal.Desktop",
                "/org/freedesktop/portal/desktop",
                &format!("org.freedesktop.portal.Settings.{m}"),
                "string:org.freedesktop.appearance",
                "string:color-scheme",
            ],
        );
        match p.split_whitespace().last() {
            Some("1") => return false,
            Some("2") => return true,
            Some("0") => break,
            _ => {}
        }
    }
    let g = out("gsettings", &["get", "org.gnome.desktop.interface", "color-scheme"]);
    if g.contains("dark") {
        return false;
    }
    if g.contains("light") {
        return true;
    }
    let k = out("kreadconfig6", &["--group", "General", "--key", "ColorScheme"]);
    let k = if k.trim().is_empty() { out("kreadconfig5", &["--group", "General", "--key", "ColorScheme"]) } else { k };
    !k.trim().is_empty() && !k.contains("dark")
}

#[cfg(test)]
mod tests {
    #[test]
    fn coral_is_the_default_and_custom_follows_its_colours() {
        let p = super::palette("", "", "");
        assert_eq!(p.accent_hex, "#ef6a4c");
        let c = super::palette("custom", "#3b82f6", "#202020");
        assert_eq!(c.accent_hex, "#3b82f6");
        assert!(super::lum(&c.bg_hex) <= 0.16);
        let l = super::palette("custom", "#3b82f6", "#f6f2ea");
        assert_eq!(l.bg_hex, "#f6f2ea"); // a light background stays light
        assert!(super::known("ocean") && super::known("") && super::known("custom") && !super::known("nope"));
    }
}
