//! The "More presets" gallery (Designer page): about a hundred crosshairs made from the engine's own settings,
//! grouped in sections. Data only, plus the tests that keep every one clean.
//!
//! Colour rule: every preset has its OWN colour (the colours people really use: cyan, green, yellow, magenta / pink,
//! white, red, orange, light blue, black with a white outline) and picking it applies that colour too - it's part of
//! the design; change it afterwards as usual. The "Colour sets" pick theirs by name, the rest get one from PALETTE,
//! spread so tiles next to each other differ. The 19 built-in tiles have theirs in BUILTIN_COLOURS.
//!
//! Every value is on a share-code step (whole / half pixels, opacity in 0.01), so each preset round-trips through
//! an AIM1 code unchanged; at 1x every non-rotated, non-round preset is crisp (no half-covered pixels) and centred.
use crate::config::Profile;
use crate::gfx::Color;

#[derive(Clone)]
pub struct More {
    pub name: &'static str,
    pub section: &'static str,
    /// extra words for the search (the name and section are searched too)
    pub tags: &'static str,
    /// true = a colour chosen by name ("Colour sets"); the others get theirs from PALETTE
    pub colour: bool,
    pub p: Profile,
}

pub const SECTIONS: [&str; 11] = [
    "Classic crosses",
    "Dots",
    "Circles & rings",
    "T-shapes",
    "X & diagonal",
    "Precise (1 px)",
    "Bold / high visibility",
    "Scopes",
    "Minimal",
    "Fun",
    "Colour sets",
];

fn base() -> Profile {
    Profile { inner: false, dot: false, outer: false, circle: false, ..Profile::default() }
}
fn cross(p: &mut Profile, len: f32, thick: f32, gap: f32) {
    p.inner = true;
    p.inner_len = len;
    p.inner_thick = thick;
    p.inner_gap = gap;
}
fn outer(p: &mut Profile, len: f32, thick: f32, gap: f32, opacity: f32) {
    p.outer = true;
    p.outer_len = len;
    p.outer_thick = thick;
    p.outer_gap = gap;
    p.outer_opacity = opacity;
}
fn dot(p: &mut Profile, size: f32, round: bool) {
    p.dot = true;
    p.dot_size = size;
    p.dot_round = round;
}
fn ring(p: &mut Profile, r: f32, t: f32) {
    p.circle = true;
    p.circle_radius = r;
    p.circle_thick = t;
}

/// (crosshair colour, outline colour) - the colours competitive players really use
pub const PALETTE: [(u32, u32); 10] = [
    (0x00FFFF, 0x000000), // cyan
    (0x00FF00, 0x000000), // green
    (0xFFFF00, 0x000000), // yellow
    (0xFF00FF, 0x000000), // magenta
    (0xFFFFFF, 0x000000), // white
    (0xFF2828, 0x000000), // red
    (0xFF9500, 0x000000), // orange
    (0x5AC8FA, 0x000000), // light blue
    (0xFF4FA3, 0x000000), // pink
    (0x000000, 0xFFFFFF), // black with a white outline
];

/// The colours of the 19 built-in tiles + Dual, in crosshair::presets() order
pub const BUILTIN_COLOURS: [(u32, u32); 20] = [
    (0x00FF00, 0x000000), // Classic: green
    (0x00FFFF, 0x000000), // Classic Dot: cyan
    (0xFFFF00, 0x000000), // Compact: yellow
    (0xFF00FF, 0x000000), // Plus: magenta
    (0xFF2828, 0x000000), // Bold Plus: red
    (0xFFFFFF, 0x000000), // Wide Gap: white
    (0x5AC8FA, 0x000000), // Needle: light blue
    (0xFF2828, 0x000000), // Dot: red
    (0x00FFFF, 0x000000), // Micro Dot: cyan
    (0xFF9500, 0x000000), // Round Dot: orange
    (0xFFFF00, 0x000000), // Box: yellow
    (0x00FF00, 0x000000), // Circle: green
    (0xFF4FA3, 0x000000), // Circle Dot: pink
    (0xFFFFFF, 0x000000), // Halo: white
    (0x00FFFF, 0x000000), // Ring Cross: cyan
    (0xFF2828, 0x000000), // Scope: red
    (0x000000, 0xFFFFFF), // T-Shape: black, white outline
    (0xFF00FF, 0x000000), // X: magenta
    (0xFF9500, 0x000000), // X Dot: orange
    (0x00FF00, 0x000000), // Dual: green
];

/// A built-in tile: its shape (crosshair::apply_shape) + its colour
pub fn apply_builtin(cur: &mut Profile, i: usize, shape: &Profile) {
    crate::crosshair::apply_shape(cur, shape);
    if let Some(&(c, o)) = BUILTIN_COLOURS.get(i) {
        cur.color = Color::hex(c);
        cur.outline_color = Color::hex(o);
        cur.rainbow = false;
    }
}

pub fn all() -> Vec<More> {
    let mut v = all_shapes();
    // the colour: spread over PALETTE with a step of 3, so neighbours differ (and the next row starts elsewhere)
    for (i, m) in v.iter_mut().enumerate().filter(|(_, m)| !m.colour) {
        let (c, o) = PALETTE[(i * 3) % PALETTE.len()];
        m.p.color = Color::hex(c);
        m.p.outline_color = Color::hex(o);
    }
    v
}

fn all_shapes() -> Vec<More> {
    let mut v = Vec::new();
    let mut add = |section: &'static str, name: &'static str, tags: &'static str, f: &dyn Fn(&mut Profile)| {
        let mut p = base();
        f(&mut p);
        v.push(More { name, section, tags, colour: false, p });
    };
    // ---- Classic crosses: what most competitive players run - small, a gap, an outline
    let s = "Classic crosses";
    add(s, "Tight", "small competitive cs valorant", &|p| cross(p, 3.0, 2.0, 1.0));
    add(s, "Small", "competitive cs", &|p| cross(p, 4.0, 2.0, 2.0));
    add(s, "Medium", "default", &|p| cross(p, 6.0, 2.0, 3.0));
    add(s, "Long", "", &|p| cross(p, 9.0, 2.0, 4.0));
    add(s, "Wide", "gap", &|p| cross(p, 7.0, 2.0, 6.0));
    add(s, "Big Gap", "gap open", &|p| cross(p, 6.0, 2.0, 10.0));
    add(s, "Short Gap", "gap", &|p| cross(p, 3.0, 2.0, 4.0));
    add(s, "Gapless", "plus solid", &|p| cross(p, 5.0, 2.0, 0.0));
    add(s, "Long Gapless", "plus solid", &|p| cross(p, 10.0, 2.0, 0.0));
    add(s, "Cross Dot", "dot", &|p| { cross(p, 5.0, 2.0, 3.0); dot(p, 2.0, false); });
    add(s, "Long Cross Dot", "dot", &|p| { cross(p, 8.0, 2.0, 5.0); dot(p, 2.0, false); });
    add(s, "Dual", "outer two lines", &|p| { cross(p, 5.0, 2.0, 3.0); outer(p, 3.0, 2.0, 12.0, 0.6); });
    add(s, "Dual Close", "outer two lines", &|p| { cross(p, 3.0, 2.0, 2.0); outer(p, 3.0, 2.0, 8.0, 0.6); });
    add(s, "Soft Cross", "transparent see-through", &|p| { cross(p, 6.0, 2.0, 3.0); p.inner_opacity = 0.75; });
    // ---- Dots
    let s = "Dots";
    add(s, "Pin Dot", "tiny 1px", &|p| dot(p, 1.0, false));
    add(s, "Dot 2", "small", &|p| dot(p, 2.0, false));
    add(s, "Dot 3", "", &|p| dot(p, 3.0, false));
    add(s, "Dot 4", "", &|p| dot(p, 4.0, false));
    add(s, "Dot 6", "big", &|p| dot(p, 6.0, false));
    add(s, "Round 4", "circle round", &|p| dot(p, 4.0, true));
    add(s, "Round 6", "circle round", &|p| dot(p, 6.0, true));
    add(s, "Round 8", "circle round big", &|p| dot(p, 8.0, true));
    add(s, "Bare Dot", "no outline clean", &|p| { dot(p, 3.0, false); p.outline = false; });
    add(s, "Glass Dot", "transparent see-through", &|p| { dot(p, 4.0, false); p.dot_opacity = 0.7; });
    // ---- Circles & rings
    let s = "Circles & rings";
    add(s, "Ring 6", "small circle", &|p| ring(p, 6.0, 1.0));
    add(s, "Ring 8", "circle", &|p| ring(p, 8.0, 2.0));
    add(s, "Ring 14", "circle", &|p| ring(p, 14.0, 2.0));
    add(s, "Thin Ring", "circle 1px", &|p| { ring(p, 12.0, 1.0); p.outline = false; });
    add(s, "Ring Dot", "circle dot", &|p| { ring(p, 10.0, 1.0); dot(p, 2.0, false); });
    add(s, "Big Ring Dot", "circle dot", &|p| { ring(p, 20.0, 1.0); dot(p, 2.0, false); });
    add(s, "Ring Round Dot", "circle dot", &|p| { ring(p, 9.0, 2.0); dot(p, 3.0, true); });
    add(s, "Ring Ticks", "circle lines", &|p| { ring(p, 10.0, 1.0); cross(p, 4.0, 2.0, 12.0); });
    add(s, "Ring Cross", "circle lines", &|p| { ring(p, 12.0, 2.0); cross(p, 4.0, 2.0, 3.0); });
    add(s, "Soft Ring", "circle transparent", &|p| { ring(p, 14.0, 2.0); p.circle_opacity = 0.6; dot(p, 2.0, false); });
    // ---- T-shapes (no top line)
    let s = "T-shapes";
    add(s, "T Small", "t", &|p| { cross(p, 4.0, 2.0, 2.0); p.t_style = true; });
    add(s, "T Medium", "t", &|p| { cross(p, 6.0, 2.0, 3.0); p.t_style = true; });
    add(s, "T Long", "t", &|p| { cross(p, 9.0, 2.0, 4.0); p.t_style = true; });
    add(s, "T Dot", "t dot", &|p| { cross(p, 6.0, 2.0, 3.0); dot(p, 2.0, false); p.t_style = true; });
    add(s, "T Wide", "t gap", &|p| { cross(p, 6.0, 2.0, 6.0); p.t_style = true; });
    add(s, "T Thin", "t 1px precise", &|p| { cross(p, 7.0, 1.0, 3.0); p.t_style = true; });
    add(s, "T Bold", "t thick", &|p| { cross(p, 6.0, 4.0, 3.0); p.t_style = true; });
    add(s, "T Gapless", "t solid", &|p| { cross(p, 6.0, 2.0, 0.0); p.t_style = true; });
    // ---- X & diagonal
    let s = "X & diagonal";
    add(s, "X Small", "x diagonal", &|p| { cross(p, 4.0, 2.0, 2.0); p.rotation = 45.0; });
    add(s, "X Medium", "x diagonal", &|p| { cross(p, 6.0, 2.0, 3.0); p.rotation = 45.0; });
    add(s, "X Long", "x diagonal", &|p| { cross(p, 9.0, 2.0, 4.0); p.rotation = 45.0; });
    add(s, "X Dot", "x diagonal dot", &|p| { cross(p, 5.0, 2.0, 4.0); dot(p, 2.0, false); p.rotation = 45.0; });
    add(s, "X Gapless", "x diagonal solid", &|p| { cross(p, 6.0, 2.0, 0.0); p.rotation = 45.0; });
    add(s, "X Thin", "x diagonal 1px", &|p| { cross(p, 7.0, 1.0, 3.0); p.rotation = 45.0; });
    add(s, "X Ring", "x diagonal circle", &|p| { cross(p, 4.0, 2.0, 3.0); ring(p, 12.0, 1.0); p.rotation = 45.0; });
    add(s, "Diamond", "x diagonal square rotated", &|p| { dot(p, 5.0, false); p.rotation = 45.0; });
    // ---- Precise (1 px)
    let s = "Precise (1 px)";
    add(s, "Hair Cross", "thin 1px", &|p| cross(p, 5.0, 1.0, 2.0));
    add(s, "Hair Long", "thin 1px", &|p| cross(p, 10.0, 1.0, 3.0));
    add(s, "Hair Wide", "thin 1px gap", &|p| cross(p, 6.0, 1.0, 6.0));
    add(s, "Hair Plus", "thin 1px solid", &|p| cross(p, 6.0, 1.0, 0.0));
    add(s, "Hair Dot", "thin 1px dot", &|p| { cross(p, 5.0, 1.0, 3.0); dot(p, 1.0, false); });
    add(s, "Needle Long", "thin 1px long sniper", &|p| { cross(p, 16.0, 1.0, 4.0); p.outline = false; });
    add(s, "Bare Hair", "thin 1px no outline", &|p| { cross(p, 6.0, 1.0, 2.0); p.outline = false; });
    add(s, "Bare Hair Dot", "thin 1px no outline dot", &|p| { cross(p, 6.0, 1.0, 3.0); dot(p, 1.0, false); p.outline = false; });
    add(s, "Hair Ring", "thin 1px circle", &|p| { ring(p, 8.0, 1.0); dot(p, 1.0, false); p.outline = false; });
    add(s, "Hair Dual", "thin 1px outer", &|p| { cross(p, 4.0, 1.0, 2.0); outer(p, 4.0, 1.0, 10.0, 0.6); });
    // ---- Bold / high visibility
    let s = "Bold / high visibility";
    add(s, "Chunky", "thick bold", &|p| cross(p, 6.0, 4.0, 3.0));
    add(s, "Chunky Plus", "thick bold solid", &|p| cross(p, 8.0, 4.0, 0.0));
    add(s, "Fat Dot", "thick bold big", &|p| dot(p, 8.0, false));
    add(s, "Fat Round", "thick bold big round", &|p| dot(p, 10.0, true));
    add(s, "Heavy Outline", "thick bold outline", &|p| { cross(p, 6.0, 2.0, 3.0); p.outline_thick = 2.0; });
    add(s, "Bold Ring", "thick bold circle", &|p| { ring(p, 10.0, 3.0); dot(p, 2.0, false); });
    add(s, "Big Cross", "thick bold big", &|p| { cross(p, 12.0, 3.0, 4.0); p.outline_thick = 2.0; });
    add(s, "Beacon", "thick bold big dot circle", &|p| { ring(p, 14.0, 2.0); dot(p, 6.0, true); p.outline_thick = 2.0; });
    // ---- Scopes
    let s = "Scopes";
    add(s, "Scope", "sniper circle", &|p| { ring(p, 16.0, 2.0); cross(p, 9.0, 2.0, 20.0); dot(p, 2.0, false); });
    add(s, "Scope Thin", "sniper circle 1px", &|p| { ring(p, 20.0, 1.0); cross(p, 10.0, 1.0, 22.0); dot(p, 1.0, false); });
    add(s, "Mil-dot", "sniper outer", &|p| { cross(p, 8.0, 1.0, 2.0); outer(p, 6.0, 3.0, 16.0, 1.0); });
    add(s, "Duplex", "sniper outer thick", &|p| { cross(p, 10.0, 1.0, 0.0); outer(p, 10.0, 3.0, 10.0, 1.0); });
    add(s, "German", "sniper t outer", &|p| { cross(p, 10.0, 1.0, 1.0); outer(p, 12.0, 3.0, 11.0, 1.0); p.t_style = true; });
    add(s, "Red Dot Sight", "sight circle dot", &|p| { ring(p, 24.0, 1.0); dot(p, 3.0, true); p.circle_opacity = 0.5; });
    add(s, "Holo", "sight circle ticks", &|p| { ring(p, 18.0, 2.0); cross(p, 3.0, 2.0, 13.0); dot(p, 2.0, false); });
    add(s, "Range", "sniper circle long", &|p| { ring(p, 28.0, 1.0); cross(p, 24.0, 1.0, 6.0); p.outline = false; });
    // ---- Minimal
    let s = "Minimal";
    add(s, "Speck", "tiny 1px no outline", &|p| { dot(p, 1.0, false); p.outline = false; });
    add(s, "Ghost Dot", "transparent", &|p| { dot(p, 2.0, false); p.dot_opacity = 0.6; p.outline = false; });
    add(s, "Four Ticks", "gap tiny", &|p| { cross(p, 2.0, 1.0, 6.0); p.outline = false; });
    add(s, "Ticks", "gap tiny", &|p| cross(p, 2.0, 2.0, 4.0));
    add(s, "Whisper", "transparent thin", &|p| { cross(p, 5.0, 1.0, 3.0); p.inner_opacity = 0.6; p.outline = false; });
    add(s, "Mini Plus", "tiny", &|p| cross(p, 2.0, 2.0, 0.0));
    add(s, "Corner Ticks", "x tiny diagonal", &|p| { cross(p, 2.0, 1.0, 6.0); p.rotation = 45.0; p.outline = false; });
    add(s, "Point", "tiny no outline", &|p| { dot(p, 2.0, false); p.outline = false; });
    // ---- Fun
    let s = "Fun";
    add(s, "Target", "circle dot bullseye", &|p| { ring(p, 9.0, 2.0); dot(p, 4.0, true); });
    add(s, "Bullseye", "circle dot", &|p| { ring(p, 6.0, 2.0); dot(p, 2.0, false); });
    add(s, "Star", "outer diagonal", &|p| { cross(p, 4.0, 2.0, 0.0); outer(p, 3.0, 2.0, 7.0, 1.0); p.rotation = 45.0; });
    add(s, "Box Sight", "square", &|p| { cross(p, 3.0, 2.0, 6.0); dot(p, 2.0, false); p.outline_thick = 2.0; });
    add(s, "Plus Ring", "circle solid", &|p| { ring(p, 7.0, 2.0); cross(p, 10.0, 2.0, 0.0); });
    add(s, "Halo X", "circle diagonal", &|p| { ring(p, 16.0, 1.0); cross(p, 5.0, 1.0, 16.0); p.rotation = 45.0; });
    add(s, "Arrows", "outer t", &|p| { cross(p, 4.0, 2.0, 6.0); outer(p, 2.0, 4.0, 12.0, 1.0); p.t_style = true; });
    add(s, "Orbit", "circle outer", &|p| { ring(p, 11.0, 1.0); dot(p, 3.0, true); outer(p, 4.0, 1.0, 15.0, 0.7); });
    // ---- Colour sets: these also set their colour (common competitive picks)
    let s = "Colour sets";
    let mut add_c = |name: &'static str, tags: &'static str, rgb: u32, f: &dyn Fn(&mut Profile)| {
        let mut p = base();
        f(&mut p);
        p.color = Color::hex(rgb);
        p.outline_color = Color::BLACK;
        v.push(More { name, section: s, tags, colour: true, p });
    };
    add_c("Cyan Tight", "colour color blue competitive", 0x00FFFF, &|p| cross(p, 3.0, 2.0, 1.0));
    add_c("Green Classic", "colour color", 0x00FF00, &|p| cross(p, 5.0, 2.0, 3.0));
    add_c("Lime Small", "colour color neon", 0x39FF14, &|p| cross(p, 4.0, 2.0, 2.0));
    add_c("Yellow Cross", "colour color", 0xFFFF00, &|p| cross(p, 5.0, 2.0, 2.0));
    add_c("Magenta Dot", "colour color pink", 0xFF00FF, &|p| dot(p, 4.0, false));
    add_c("Red Dot", "colour color", 0xFF2020, &|p| dot(p, 3.0, false));
    add_c("White Hair", "colour color thin", 0xFFFFFF, &|p| cross(p, 6.0, 1.0, 2.0));
    add_c("White Dot", "colour color", 0xFFFFFF, &|p| dot(p, 2.0, false));
    add_c("Pink Plus", "colour color", 0xFF4FA3, &|p| cross(p, 5.0, 2.0, 0.0));
    add_c("Orange Ring", "colour color circle", 0xFF9500, &|p| { ring(p, 8.0, 2.0); dot(p, 2.0, false); });
    add_c("Blue T", "colour color t", 0x3B82F6, &|p| { cross(p, 6.0, 2.0, 3.0); p.t_style = true; });
    add_c("Cyan Dot Cross", "colour color dot", 0x00E5FF, &|p| { cross(p, 4.0, 2.0, 3.0); dot(p, 2.0, false); });
    let mut add_bw = |name: &'static str, tags: &'static str, f: &dyn Fn(&mut Profile)| {
        let mut p = base();
        f(&mut p);
        p.color = Color::BLACK;
        p.outline_color = Color::WHITE;
        v.push(More { name, section: s, tags, colour: true, p });
    };
    add_bw("Ink Cross", "colour color black white outline", &|p| cross(p, 5.0, 2.0, 2.0));
    add_bw("Ink Dot", "colour color black white outline", &|p| dot(p, 3.0, false));
    v
}

/// Applies a gallery preset: its shape and its colour (change the colour afterwards as usual)
pub fn apply(cur: &mut Profile, m: &More) {
    crate::crosshair::apply_shape(cur, &m.p);
    cur.color = m.p.color;
    cur.outline_color = m.p.outline_color;
    cur.rainbow = false;
}

/// Gallery filter: true when all typed terms appear in the label, group or keywords (any letter case).
pub fn matches(m: &More, q: &str) -> bool {
    let t = format!("{} {} {}", m.name, m.section, m.tags).to_lowercase();
    q.to_lowercase().split_whitespace().all(|w| t.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crosshair::Renderer;
    use crate::gfx::Canvas;

    fn render(p: &Profile) -> Canvas {
        let mut r = Renderer::new(std::env::temp_dir(), |_| None);
        let mut c = Canvas::new(0, 0);
        r.render(p, 0.0, &mut c);
        c
    }

    #[test]
    fn names_are_unique_sections_known_and_values_already_clean() {
        let all = all();
        assert!(all.len() >= 80 && all.len() <= 120, "{} presets", all.len());
        let mut names: Vec<&str> = all.iter().map(|m| m.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), all.len(), "duplicate names");
        for m in &all {
            assert!(SECTIONS.contains(&m.section), "{}: unknown section", m.name);
            let mut s = m.p.clone();
            s.sanitize();
            assert_eq!(s, m.p, "{}: sanitize changes it (out of range)", m.name);
        }
        // only "Colour sets" choose their colour by name; within a section, tiles next to each other differ
        assert!(all.iter().all(|m| m.colour == (m.section == "Colour sets")));
        for w in all.windows(2).filter(|w| w[0].section == w[1].section && !w[0].colour) {
            assert_ne!(w[0].p.color, w[1].p.color, "{} / {}: same colour side by side", w[0].name, w[1].name);
        }
        assert_eq!(BUILTIN_COLOURS.len(), crate::crosshair::presets().len());
    }

    #[test]
    fn every_preset_round_trips_through_a_share_code() {
        for m in all() {
            let code = crate::share::encode_profile(m.name, &m.p).unwrap();
            let s = crate::share::decode(&code).unwrap();
            assert_eq!(s.name, m.name);
            assert_eq!(s.profile, m.p, "{}: {code}", m.name);
        }
    }

    #[test]
    fn every_preset_is_centred_and_crisp_at_1x() {
        let mut bad = Vec::new();
        for m in all() {
            if let Err(e) = std::panic::catch_unwind(|| check(&m)) {
                bad.push(e.downcast_ref::<String>().cloned().unwrap_or_default());
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    fn check(m: &More) {
        {
            let c = render(&m.p);
            assert!(c.w > 0 && c.h > 0 && c.px.iter().any(|p| p >> 24 != 0), "{}: draws nothing", m.name);
            let at = |x: i32, y: i32| c.px[(y * c.w + x) as usize] >> 24;
            // centred: a mirror image left / right (and up / down unless it's a T) - around the line between the
            // two middle pixels (even widths), or around the middle pixel with one spare empty column / row (odd widths)
            let col_diff = |x: i32, x2: i32| (0..c.h).map(|y| at(x, y).abs_diff(at(x2, y))).max().unwrap_or(0);
            let row_diff = |y: i32, y2: i32| (0..c.w).map(|x| at(x, y).abs_diff(at(x, y2))).max().unwrap_or(0);
            let lr_edge = (0..c.w).all(|x| col_diff(x, c.w - 1 - x) <= 2);
            let lr_pixel = (0..c.w - 1).all(|x| col_diff(x, c.w - 2 - x) <= 2) && (0..c.h).all(|y| at(c.w - 1, y) == 0);
            assert!(lr_edge || lr_pixel, "{}: not centred left / right", m.name);
            if !m.p.t_style {
                let ud_edge = (0..c.h).all(|y| row_diff(y, c.h - 1 - y) <= 2);
                let ud_pixel = (0..c.h - 1).all(|y| row_diff(y, c.h - 2 - y) <= 2) && (0..c.w).all(|x| at(x, c.h - 1) == 0);
                assert!(ud_edge || ud_pixel, "{}: not centred up / down", m.name);
            }
            // crisp: straight lines and square dots land on whole pixels - every pixel is either empty or one of the
            // few full alphas its parts use (no anti-aliased half pixels). Circles, round dots and rotation are smooth on purpose.
            let smooth = m.p.circle || (m.p.dot && m.p.dot_round) || m.p.rotation != 0.0;
            if !smooth {
                // the shape itself, at full opacity (see-through parts blend with their outline on purpose)
                let mut q = m.p.clone();
                (q.opacity, q.inner_opacity, q.outer_opacity, q.dot_opacity, q.outline_opacity) = (1.0, 1.0, 1.0, 1.0, 1.0);
                for p in &render(&q).px {
                    let a = p >> 24;
                    assert!(a == 0 || a == 255, "{}: a half-covered pixel (alpha {a})", m.name);
                }
            }
        }
    }

    #[test]
    fn search_finds_by_name_section_and_tag() {
        let all = all();
        assert!(all.iter().any(|m| matches(m, "sniper")));
        assert!(all.iter().filter(|m| matches(m, "dots")).count() >= 8);
        assert!(all.iter().any(|m| matches(m, "CYAN tight")));
        assert!(!all.iter().any(|m| matches(m, "zzzz")));
    }
}
