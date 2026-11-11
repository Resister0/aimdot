//! Import crosshair codes from other games into the nearest Aimdot crosshair:
//! - **CS2** share codes, all four layouts: the old `CSGO-xxxxx-xxxxx-xxxxx-xxxxx-xxxxx` code (CS:GO era sizes, and
//!   the two 2026 layouts that kept that container) and the current `CS` + 44 letters code (CS2 1.41.8.8, 30 Sep 2026).
//! - **Valorant** profile codes (`0;P;c;1;o;1;...`), the primary crosshair.
//!
//! The layouts follow the public byte tables of the MIT-licensed `csgo-sharecode` project (2026); the Valorant keys
//! are the ones the game writes. What can't be drawn by Aimdot (lines that move while shooting / walking, quads and
//! squares, separate vertical length) is listed in the returned note. Sizes: CS2 codes from 2026 store pixels for
//! the screen height they were made on, scaled here to `screen_h`; CS:GO-era codes use the game's own units
//! (480 units = the screen height), so their pixels are an approximation (⚠ not compared on screen with CS2 yet).

use crate::config::Profile;
use crate::gfx::Color;

pub struct Imported {
    /// "CS2 crosshair" / "Valorant crosshair".
    pub name: String,
    pub profile: Profile,
    /// What didn't carry over, in plain words ("" = everything).
    pub note: String,
}

/// Finds a CS2 or Valorant code in `text`. None = no such code; Some(Err) = found but not readable.
pub fn read(text: &str, screen_h: u32) -> Option<Result<Imported, String>> {
    let h = if (240..=8640).contains(&screen_h) { screen_h as f32 } else { 1080.0 };
    if let Some(bytes) = find_cs2(text) {
        return Some(cs2(&bytes, h));
    }
    if let Some(bytes) = find_legacy(text) {
        return Some(legacy(&bytes, h));
    }
    find_valorant(text).map(valorant)
}

// ------------------------------------------------------------------ CS2 containers

const DICT: &[u8; 57] = b"ABCDEFGHJKLMNOPQRSTUVWXYZabcdefhijkmnopqrstuvwxyz23456789";

/// Base-57 letters (least significant first) -> `n` bytes, big-endian. None when it doesn't fit.
fn chars_to_bytes(chars: &[u8], n: usize) -> Option<Vec<u8>> {
    let mut out = vec![0u8; n];
    for &c in chars.iter().rev() {
        let d = DICT.iter().position(|&x| x == c)? as u32;
        let mut carry = d;
        for b in out.iter_mut().rev() {
            let v = *b as u32 * 57 + carry;
            *b = v as u8;
            carry = v >> 8;
        }
        if carry != 0 {
            return None;
        }
    }
    Some(out)
}

fn checksum_ok(b: &[u8]) -> bool {
    b[1..].iter().map(|&x| x as u32).sum::<u32>() % 256 == b[0] as u32
}

/// `CSGO-` + 25 letters (dashes optional) -> 18 bytes.
fn find_legacy(text: &str) -> Option<Vec<u8>> {
    let mut rest = text;
    while let Some(i) = rest.find("CSGO") {
        let tail = &rest[i + 4..];
        let letters: Vec<u8> = tail.bytes().take_while(|&c| c == b'-' || DICT.contains(&c)).filter(|&c| c != b'-').collect();
        if letters.len() == 25 {
            if let Some(b) = chars_to_bytes(&letters, 18) {
                return Some(b);
            }
        }
        rest = &rest[i + 4..];
    }
    None
}

/// `CS` + exactly 44 letters -> 32 bytes.
fn find_cs2(text: &str) -> Option<Vec<u8>> {
    let b = text.as_bytes();
    for i in 0..b.len().saturating_sub(1) {
        if &b[i..i + 2] != b"CS" || (i > 0 && DICT.contains(&b[i - 1])) {
            continue;
        }
        let letters: Vec<u8> = b[i + 2..].iter().copied().take_while(|c| DICT.contains(c)).collect();
        if letters.len() == 44 {
            if let Some(v) = chars_to_bytes(&letters, 32) {
                return Some(v);
            }
        }
    }
    None
}

/// What all CS layouts end up as, in pixels on the target screen.
struct Cs {
    style: u8,
    dot: bool,
    t: bool,
    color: Color,
    alpha: f32,
    /// 0 none, 1 full, 2 half
    outline: u8,
    outline_px: f32,
    outline_color: Color,
    outline_alpha: f32,
    gap: f32,
    len: f32,
    thick: f32,
    pixel_styles: bool,
}

fn cs_profile(c: Cs, what: &str) -> Imported {
    let mut notes: Vec<&str> = Vec::new();
    let mut p = Profile { inner: false, dot: false, outer: false, circle: false, ..Profile::default() };
    p.color = c.color;
    p.opacity = c.alpha.clamp(0.05, 1.0);
    p.outline = c.outline > 0;
    p.outline_thick = c.outline_px.round().clamp(1.0, 6.0);
    p.outline_color = c.outline_color;
    p.outline_opacity = if c.outline == 2 { c.outline_alpha * 0.5 } else { c.outline_alpha };
    let thick = c.thick.round().max(1.0);
    let len = c.len.round();
    let gap = c.gap.round().max(0.0);
    if c.gap < 0.0 {
        notes.push("a gap below 0 is drawn as 0");
    }
    // the 2026 styles: 0 dyn cross, 1 dyn circle, 2 dyn cross (legacy), 3 static circle, 4 static cross,
    // 5 static cross (shot feedback), 6 dot only, 7 dyn quad, 8 static square, 9 static quad.
    // CS:GO era: 0-3 classic / dynamic, 4 classic static, 5 legacy.
    let (circle, dot_only, square) = if c.pixel_styles { (matches!(c.style, 1 | 3), c.style == 6, matches!(c.style, 7..=9)) } else { (false, false, false) };
    let moving = if c.pixel_styles { matches!(c.style, 0 | 1 | 2 | 5 | 7) } else { c.style != 4 };
    if moving {
        notes.push("its lines move while you shoot in CS2 - here they stand still");
    }
    if square {
        notes.push("Aimdot has no quad / square style - it's a cross here");
    }
    if circle {
        p.circle = true;
        p.circle_radius = (gap + len / 2.0).max(2.0);
        p.circle_thick = thick;
    } else if !dot_only && len > 0.0 {
        p.inner = true;
        p.inner_len = len;
        p.inner_thick = thick;
        p.inner_gap = gap;
        p.t_style = c.t;
    }
    if c.dot || dot_only {
        p.dot = true;
        p.dot_size = thick;
    }
    if !p.inner && !p.dot && !p.circle {
        // length 0 and no dot: CS2 shows nothing; a small dot is more useful
        p.dot = true;
        p.dot_size = thick;
        notes.push("it had no lines and no dot - here it is a dot");
    }
    p.sanitize();
    Imported { name: what.into(), profile: p, note: note_text(&notes) }
}

fn note_text(v: &[&str]) -> String {
    if v.is_empty() {
        String::new()
    } else {
        let mut s = String::from("Not the same as in the game: ");
        s.push_str(&v.join("; "));
        s.push('.');
        s
    }
}

/// The current code (`CS` + 44 letters), version 1.
fn cs2(b: &[u8], h: f32) -> Result<Imported, String> {
    if !checksum_ok(b) {
        return Err("That CS2 code is damaged - copy it again".into());
    }
    if b[1] != 1 {
        return Err("That CS2 code is from a newer CS2 than Aimdot knows".into());
    }
    let sh = (b[2] as u32 | (b[3] as u32) << 8).max(240) as f32;
    if b[2] == 0 && b[3] == 0 {
        return Err("That CS2 code is damaged - copy it again".into());
    }
    let k = h / sh;
    let gap = (((b[14] as u16) | (b[15] as u16) << 8) as i16).clamp(-3840, 3840) as f32;
    Ok(cs_profile(
        Cs {
            style: (b[4] & 0x1F).min(9),
            dot: b[4] & 0x40 != 0,
            t: b[4] & 0x80 != 0,
            color: Color::hex((b[5] as u32) << 16 | (b[6] as u32) << 8 | b[7] as u32),
            alpha: b[8] as f32 / 255.0,
            outline: (b[13] >> 6).min(2),
            outline_px: 1.0,
            outline_color: Color::hex((b[9] as u32) << 16 | (b[10] as u32) << 8 | b[11] as u32),
            outline_alpha: b[12] as f32 / 255.0,
            gap: gap * k,
            len: b[16] as f32 * k,
            thick: (b[13] & 0x3F).min(32) as f32 * k,
            pixel_styles: true,
        },
        "CS2 crosshair",
    ))
}

/// `CSGO-` codes: version 1 (CS:GO era units), 3 and 4 (2026, pixels at a screen height).
fn legacy(b: &[u8], h: f32) -> Result<Imported, String> {
    if !checksum_ok(b) {
        return Err("That CS2 code is damaged - copy it again".into());
    }
    let rgb = |r: u8, g: u8, bl: u8| Color::hex((r as u32) << 16 | (g as u32) << 8 | bl as u32);
    match b[1] {
        1 => {
            // sizes in the game's units: 480 = the screen height (YRES); the static gap starts 4 px out
            let yres = h / 480.0;
            let gap = (b[2] as i8) as f32 / 10.0;
            let color = match b[10] & 7 {
                0 => rgb(250, 50, 50),
                1 => rgb(50, 250, 50),
                2 => rgb(250, 250, 50),
                3 => rgb(50, 50, 250),
                4 => rgb(50, 250, 250),
                _ => rgb(b[4], b[5], b[6]),
            };
            let alpha_on = (b[13] >> 4) & 4 != 0;
            Ok(cs_profile(
                Cs {
                    style: (b[13] & 0xF) >> 1,
                    dot: (b[13] >> 4) & 1 != 0,
                    t: (b[13] >> 4) & 8 != 0,
                    color,
                    alpha: if alpha_on { b[7] as f32 / 255.0 } else { 1.0 },
                    outline: if b[10] & 8 != 0 { 1 } else { 0 },
                    outline_px: b[3] as f32 / 2.0,
                    outline_color: Color::BLACK,
                    outline_alpha: 1.0,
                    gap: (4.0 + gap) * h / 1080.0,
                    len: b[14] as f32 / 10.0 * yres,
                    thick: b[12] as f32 / 10.0 * yres,
                    pixel_styles: false,
                },
                "CS2 crosshair",
            ))
        }
        3 | 4 => {
            let bits = b[10] as u32 | (b[11] as u32) << 8 | (b[12] as u32) << 16 | (b[13] as u32) << 24;
            let sh = (b[14] as u32 | (b[15] as u32) << 8).max(240) as f32;
            let k = h / sh;
            let outline = if b[1] == 3 { (b[2] & 0x20 != 0) as u8 } else { ((b[13] >> 4) & 3).min(2) };
            Ok(cs_profile(
                Cs {
                    style: b[2] & 0xF,
                    dot: b[2] & 0x40 != 0,
                    t: b[2] & 0x80 != 0,
                    color: rgb(b[3], b[4], b[5]),
                    alpha: b[6] as f32 / 255.0,
                    outline,
                    outline_px: 1.0,
                    outline_color: Color::BLACK,
                    outline_alpha: 1.0,
                    gap: b[7] as f32 * k,
                    len: b[8] as f32 * k,
                    thick: ((bits >> 23) & 0x1F) as f32 * k,
                    pixel_styles: true,
                },
                "CS2 crosshair",
            ))
        }
        _ => Err("That CS2 code's layout isn't known to Aimdot".into()),
    }
}

// ------------------------------------------------------------------ Valorant

/// A word that starts with "0;" and has the primary section ("P;") or at least a few key;value pairs.
fn find_valorant(text: &str) -> Option<&str> {
    text.split_whitespace()
        .map(|w| w.trim_matches(|c: char| c == '"' || c == '\'' || c == '`' || c == ','))
        .find(|w| w.starts_with("0;") && w.len() <= 2000 && w.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b';' | b'.' | b'-')) && w.split(';').count() >= 3)
}

fn valorant(code: &str) -> Result<Imported, String> {
    let tok: Vec<&str> = code.split(';').collect();
    let mut sec = "0";
    let mut kv: Vec<(&str, &str)> = Vec::new();
    let mut i = 1;
    while i < tok.len() {
        let t = tok[i];
        if matches!(t, "P" | "A" | "S") {
            sec = t;
            i += 1;
            continue;
        }
        let v = tok.get(i + 1).copied().unwrap_or("");
        if sec == "P" {
            kv.push((t, v));
        }
        i += 2;
    }
    let get = |k: &str| kv.iter().rev().find(|e| e.0 == k).map(|e| e.1);
    let num = |k: &str, d: f32| get(k).and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(d);
    let on = |k: &str, d: bool| get(k).map_or(d, |v| v != "0");
    let colors = [0xFFFFFF, 0x00FF00, 0x7FFF00, 0xDFFF00, 0xFFFF00, 0x00FFFF, 0xFF00FF, 0xFF0000];
    let mut notes = Vec::new();
    let mut p = Profile { inner: false, dot: false, outer: false, circle: false, ..Profile::default() };
    let ci = num("c", 0.0) as usize;
    p.color = if ci == 8 {
        get("u").filter(|h| h.len() >= 6).and_then(|h| u32::from_str_radix(&h[..6], 16).ok()).map_or(Color::WHITE, Color::hex)
    } else {
        Color::hex(*colors.get(ci).unwrap_or(&0xFFFFFF))
    };
    p.outline = on("h", true);
    p.outline_thick = num("t", 1.0).round().clamp(1.0, 6.0);
    p.outline_opacity = num("o", 0.5).clamp(0.0, 1.0);
    p.outline_color = Color::BLACK;
    if on("d", false) {
        p.dot = true;
        p.dot_size = num("z", 2.0).round().max(1.0);
        p.dot_opacity = num("a", 1.0);
    }
    let lines = |p: &mut Profile, pre: &str, d: (f32, f32, f32, f32), outer: bool| {
        let (len, thick, gap, op) = (num(&format!("{pre}l"), d.0), num(&format!("{pre}t"), d.1), num(&format!("{pre}o"), d.2), num(&format!("{pre}a"), d.3));
        if !on(&format!("{pre}b"), true) || len <= 0.0 || thick <= 0.0 || op <= 0.0 {
            return;
        }
        if outer {
            p.outer = true;
            (p.outer_len, p.outer_thick, p.outer_gap, p.outer_opacity) = (len.round(), thick.round(), gap.round(), op);
        } else {
            p.inner = true;
            (p.inner_len, p.inner_thick, p.inner_gap, p.inner_opacity) = (len.round(), thick.round(), gap.round(), op);
        }
    };
    lines(&mut p, "0", (6.0, 2.0, 3.0, 0.8), false);
    lines(&mut p, "1", (2.0, 2.0, 10.0, 0.35), true);
    if !p.inner && p.outer {
        // Aimdot's outer lines need a first set; the outer ones become the main lines
        (p.inner, p.outer) = (true, false);
        (p.inner_len, p.inner_thick, p.inner_gap, p.inner_opacity) = (p.outer_len, p.outer_thick, p.outer_gap, p.outer_opacity);
    }
    if on("0g", false) || on("1g", false) {
        notes.push("a different vertical line length isn't possible here - both use the side length");
    }
    if on("0f", true) && p.inner || on("1f", true) && p.outer || on("0m", false) || on("1m", false) {
        notes.push("lines that move while shooting or walking stand still here");
    }
    if !p.inner && !p.dot {
        p.dot = true;
        notes.push("it had no lines and no dot - here it is a dot");
    }
    p.sanitize();
    Ok(Imported { name: "Valorant crosshair".into(), profile: p, note: note_text(&notes) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(code: &str) -> Imported {
        read(code, 1080).expect("found").expect("readable")
    }

    #[test]
    fn cs2_current_code() {
        // csgo-sharecode's README example: style 2, dot, no outline, red, gap 0, length 5, thickness 1, made at 768 px
        let i = get("CSvbPubOq37zTGqtsPTP5QTrp5CB4xFXiKRLfzJsm49ZRe");
        let p = &i.profile;
        assert_eq!(p.color, Color::hex(0xFF0000));
        assert!(p.inner && p.dot && !p.outline && !p.t_style);
        assert_eq!((p.inner_len, p.inner_thick, p.inner_gap), ((5.0f32 * 1080.0 / 768.0).round(), 1.0, 0.0)); // 7, 1, 0
        assert!(i.note.contains("stand still"), "{}", i.note); // style 2 is dynamic
        // at 768 px it's the code's own pixels
        let p = read("CSvbPubOq37zTGqtsPTP5QTrp5CB4xFXiKRLfzJsm49ZRe", 768).unwrap().unwrap().profile;
        assert_eq!((p.inner_len, p.inner_thick), (5.0, 1.0));
    }

    #[test]
    fn cs2_old_codes() {
        // CS:GO era: purple 175,81,213 alpha 137 (alpha on), outline 2, thickness 1.2, length 4.6, gap -1.3, dot, T, style 2
        let p = get("Here: CSGO-Cn37R-YE7vo-pLCAL-aURmZ-z6zkG !").profile;
        assert_eq!(p.color, Color::hex(0xAF51D5));
        assert!((p.opacity - 137.0 / 255.0).abs() < 1e-4);
        assert!(p.inner && p.dot && p.t_style && p.outline);
        assert_eq!((p.inner_len, p.inner_thick, p.inner_gap, p.outline_thick), (10.0, 3.0, 3.0, 2.0)); // 4.6*2.25, 1.2*2.25, 4-1.3
        // green preset colour, no outline, no alpha, thickness 0.5, length 5, style 3
        let p = get("CSGO-LibdP-VCVEd-ESayK-rSivi-2UBtG").profile;
        assert_eq!((p.color, p.opacity, p.outline, p.dot), (Color::hex(0x32FA32), 1.0, false, false));
        assert_eq!((p.inner_len, p.inner_thick, p.inner_gap), (11.0, 1.0, 5.0));
        // 2026 (legacy-v3): dot only, pink, outline, thickness 3 at 1080 px
        let i = get("CSGO-MnUCC-89iG7-2cVar-wy7Yn-amCpF");
        let p = &i.profile;
        assert_eq!(p.color, Color::hex(0xFC0FC0));
        assert!(p.dot && !p.inner && p.outline);
        assert_eq!(p.dot_size, 3.0);
        // static cross with length 0 + dot, blue, made at 768 px -> only the dot, 3 * 1080/768 = 4 px
        let p = get("CSGO-tsJdC-2PFX6-fw8qD-6GDaX-rYCUM").profile;
        assert!(p.dot && !p.inner && !p.outline);
        assert_eq!((p.color, p.dot_size), (Color::hex(0x0028FF), 4.0));
    }

    #[test]
    fn valorant_codes() {
        // white, outlines off, dot off, inner 4/2/2 full opacity, no outer
        let i = get("0;P;h;0;0l;4;0o;2;0a;1;0f;0;1b;0");
        let p = &i.profile;
        assert_eq!(p.color, Color::WHITE);
        assert!(!p.outline && !p.dot && p.inner && !p.outer);
        assert_eq!((p.inner_len, p.inner_thick, p.inner_gap, p.inner_opacity), (4.0, 2.0, 2.0, 1.0));
        assert_eq!(i.note, "");
        // cyan dot only, custom colour section ignored for ADS, defaults for the rest
        let p = get("my crosshair: 0;s;1;P;c;5;o;1;d;1;z;3;0b;0;1b;0;A;c;7").profile;
        assert_eq!(p.color, Color::hex(0x00FFFF));
        assert!(p.dot && !p.inner && !p.outer && p.outline);
        assert_eq!((p.dot_size, p.outline_opacity), (3.0, 1.0));
        // custom colour + the game's defaults (inner + outer lines, firing error on)
        let i = get("0;P;c;8;u;FF8800FF");
        assert_eq!(i.profile.color, Color::hex(0xFF8800));
        assert!(i.profile.inner && i.profile.outer);
        assert_eq!((i.profile.outer_len, i.profile.outer_gap, i.profile.outer_opacity), (2.0, 10.0, 0.35));
        assert!(i.note.contains("stand still"));
    }

    #[test]
    fn junk_is_not_a_code() {
        assert!(read("hello", 1080).is_none());
        assert!(read("CSGO-12345-12345-12345-12345-12345", 1080).map_or(true, |r| r.is_err()));
        assert!(read("CSGO-11111-22222-33333-44444-55555", 1080).map_or(true, |r| r.is_err()));
        assert!(read("CSGO-12345-12345", 1080).is_none());
        assert!(read("0;", 1080).is_none());
        // damaged checksum: one letter changed
        assert!(read("CSGO-Cn37R-YE7vo-pLCAL-aURmZ-z6zkH", 1080).unwrap().is_err());
        assert!(read("CSvbPubOq37zTGqtsPTP5QTrp5CB4xFXiKRLfzJsm49ZRf", 1080).map_or(true, |r| r.is_err()));
        // a huge or odd Valorant-looking word doesn't panic
        let _ = read(&format!("0;P;{}", "c;99;".repeat(300)), 1080);
        let _ = read("0;P;0l;-5;0t;1e30;z;nan;d;1", 1080);
    }
}
