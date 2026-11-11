//! Unit tests (the editor-widget tests went with the first version's built-in editor).

use crate::config::{self, Hotkey, Profile, Settings};
use crate::crosshair::{self, build_masks, compose};
use crate::gfx::{Canvas, Color};
use crate::image::{decode_png, inflate, scale_to, Image};
use crate::model::Model;
use std::path::{Path, PathBuf};

fn loader(p: &Path) -> Option<Image> {
    decode_png(&std::fs::read(p).ok()?).ok()
}

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("crosshair-test-{}-{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn color_parse_and_hsv() {
    assert_eq!(Color::parse("#00ff7f"), Some(Color::hex(0x00FF7F)));
    assert_eq!(Color::parse("FA0"), Some(Color::hex(0xFFAA00)));
    assert_eq!(Color::parse("#12345678").unwrap().a, 0x78);
    assert_eq!(Color::parse("#12"), None);
    assert_eq!(Color::parse("#zzzzzz"), None);
    assert_eq!(Color::parse("#ééé"), None);
    for v in [0xFF0000u32, 0x00FF00, 0x0000FF, 0x123456, 0xFEDCBA, 0x808080, 0x000000, 0xFFFFFF, 0x2DE2A6] {
        let c = Color::hex(v);
        let (h, s, val) = c.to_hsv();
        let back = Color::from_hsv(h, s, val);
        assert!((back.r as i32 - c.r as i32).abs() <= 1 && (back.g as i32 - c.g as i32).abs() <= 1 && (back.b as i32 - c.b as i32).abs() <= 1, "{v:06X}");
    }
}

#[test]
fn config_roundtrip() {
    let mut set = Settings::default();
    set.theme = 3;
    set.accent = Color::hex(0x123456);
    set.offset_x = -42.0;
    set.hk_toggle = Hotkey { mods: 6, vk: 0x77 };
    set.active_profile = "My = odd [name]".into();
    let mut cur = Profile::default();
    cur.rainbow = true;
    cur.rotation = 45.0;
    cur.inner_len = 17.0;
    cur.image = "a b.png".into();
    let mut other = Profile::default();
    other.dot = true;
    other.color = Color::hex(0xABCDEF);
    let profiles = vec![("One".to_string(), other.clone()), ("Two words".to_string(), cur.clone())];
    let text = config::serialize(&set, &cur, &profiles);
    let (s2, c2, p2) = config::parse(&text);
    assert_eq!(set, s2);
    assert_eq!(cur, c2);
    assert_eq!(profiles, p2);
    // Stable: serialising the parsed result gives identical text.
    assert_eq!(text, config::serialize(&s2, &c2, &p2));
}

#[test]
fn config_survives_garbage() {
    let junk = "[app]\ntheme=99\nradius=nan\noffset_x=1e30\nui_scale=77\naccent=#nothex\nhk_toggle=x\n\
                [current]\nscale=-5\ninner_len=inf\nmode=9\nopacity=abc\n===\n[profile:]\n[profile:x]\ndot_size=1e9\n[weird\nfoo";
    let (s, c, p) = config::parse(junk);
    assert_eq!(s.theme, 3);
    assert_eq!(s.radius, 10.0);
    assert_eq!(s.offset_x, 500.0);
    assert_eq!(s.accent, Settings::default().accent);
    assert_eq!(s.hk_toggle, Settings::default().hk_toggle);
    assert_eq!(c.scale, 0.5);
    assert_eq!(c.mode, 1);
    assert_eq!(c.inner_len, Profile::default().inner_len);
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].1.dot_size, 40.0);
    let (d, _, _) = config::parse("");
    assert_eq!(d, Settings::default());
}

#[test]
fn hotkey_names() {
    assert_eq!(Hotkey { mods: 3, vk: 0x58 }.name(), "Ctrl + Alt + X");
    assert_eq!(Hotkey { mods: 4, vk: 0x75 }.name(), "Shift + F6");
    assert_eq!(Hotkey { mods: 0, vk: 0 }.name(), "Not set");
}

macro_rules! fixture {
    ($name:literal) => {
        (
            $name,
            include_bytes!(concat!("../tests/fixtures/", $name, ".png")) as &[u8],
            include_bytes!(concat!("../tests/fixtures/", $name, ".rgba")) as &[u8],
        )
    };
}

const FIXTURES: &[(&str, &[u8], &[u8])] = &[
    fixture!("rgba8"),
    fixture!("rgb8"),
    fixture!("gray8"),
    fixture!("graya8"),
    fixture!("pal8"),
    fixture!("pal_trns"),
    fixture!("gray1"),
    fixture!("stored"),
    fixture!("big"),
    fixture!("cross"),
    fixture!("rgba16"),
    fixture!("gray16"),
    fixture!("noise_rgba"),
    fixture!("pal4"),
    fixture!("pal8_trns"),
    fixture!("rgb8_trns"),
];

#[test]
fn png_decoder_matches_reference() {
    for (name, png, raw) in FIXTURES {
        let img = decode_png(png).unwrap_or_else(|e| panic!("{name}: {e}"));
        let w = u32::from_le_bytes(raw[0..4].try_into().unwrap()) as usize;
        let h = u32::from_le_bytes(raw[4..8].try_into().unwrap()) as usize;
        assert_eq!((img.w, img.h), (w, h), "{name}");
        assert!(img.rgba == raw[8..], "{name}: pixels differ");
    }
}

#[test]
fn png_decoder_never_panics_on_corrupt_input() {
    let png = FIXTURES[0].1;
    for cut in 0..png.len() {
        let _ = decode_png(&png[..cut]);
    }
    let mut seed = 12345u32;
    for _ in 0..3000 {
        let mut d = png.to_vec();
        for _ in 0..3 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let i = (seed >> 8) as usize % d.len();
            d[i] ^= (seed >> 3) as u8 | 1;
        }
        let _ = decode_png(&d);
    }
    assert!(inflate(&[0x78, 0x9C, 0xFF, 0xFF], 100).is_err());
    assert!(decode_png(b"not a png at all").is_err());
}

#[test]
fn axis_aligned_masks_are_crisp_and_symmetric() {
    for (name, shape) in crosshair::presets() {
        if shape.rotation != 0.0 || shape.circle || shape.dot_round {
            continue;
        }
        for scale in [1.0f32, 2.0, 3.0] {
            for thick in [1.0f32, 2.0, 3.0] {
                let mut p = shape.clone();
                p.scale = scale;
                p.inner_thick = thick;
                p.outer_thick = thick;
                p.outer_opacity = 1.0;
                if p.dot && !p.inner {
                    p.dot_size = thick;
                } else {
                    p.dot_size = thick; // keep parity consistent with the lines
                }
                let m = build_masks(&p);
                let n = m.size as usize;
                assert!(m.fill.iter().all(|&v| v == 0 || v == 255), "{name}: blurry fill");
                assert!(m.outline.iter().all(|&v| v == 0 || v == 255), "{name}: blurry outline");
                assert!(m.fill.iter().any(|&v| v == 255), "{name}: empty");
                // Fill is always inside the outline shape.
                assert!(m.fill.iter().zip(&m.outline).all(|(f, o)| f <= o), "{name}");
                // Mirror symmetry about the design centre (pixel centre when thickness is odd).
                let odd = ((thick * scale).round() as i32) % 2 == 1;
                let mir = |i: usize| if odd { n - 2 - i } else { n - 1 - i };
                for y in 0..n {
                    for x in 0..n - 1 {
                        if odd && x == n - 1 {
                            continue;
                        }
                        assert_eq!(m.fill[y * n + x], m.fill[y * n + mir(x)], "{name} s{scale} t{thick} h-sym at {x},{y}");
                        if !p.t_style && y < n - 1 {
                            assert_eq!(m.fill[y * n + x], m.fill[mir(y) * n + x], "{name} s{scale} t{thick} v-sym at {x},{y}");
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn classic_mask_has_expected_pixels() {
    // Classic: 4 arms, length 6, thickness 2, gap 3, 1 px outline.
    let p = crosshair::presets()[0].1.clone();
    let m = build_masks(&p);
    let filled = m.fill.iter().filter(|&&v| v == 255).count();
    assert_eq!(filled, 4 * 6 * 2);
    let outlined = m.outline.iter().filter(|&&v| v == 255).count();
    assert_eq!(outlined, 4 * 8 * 4);
    // Gap is empty.
    let n = m.size;
    let at = |x: i32, y: i32| m.fill[((n / 2 + y) * n + n / 2 + x) as usize];
    assert_eq!(at(0, 0), 0);
    assert_eq!(at(2, 0), 0);
    assert_eq!(at(3, 0), 255);
    assert_eq!(at(8, 0), 255);
    assert_eq!(at(9, 0), 0);
    assert_eq!(at(-4, -1), 255);
    assert_eq!(at(0, -9), 255);
    assert_eq!(at(0, -10), 0);
}

#[test]
fn rotated_and_round_shapes_render() {
    for (name, shape) in crosshair::presets() {
        for rot in [0.0f32, 30.0, 45.0, 90.0, 180.0, 270.0] {
            let mut p = shape.clone();
            p.rotation = rot;
            p.scale = 2.0;
            let m = build_masks(&p);
            let sum: u64 = m.fill.iter().map(|&v| v as u64).sum();
            assert!(sum > 255, "{name} rot {rot}: nothing drawn");
            // Nothing touches the canvas edge (the bounding box is large enough).
            let n = m.size as usize;
            for i in 0..n {
                assert_eq!(m.outline[i], 0, "{name} rot {rot} clipped top");
                assert_eq!(m.outline[(n - 1) * n + i], 0, "{name} rot {rot} clipped bottom");
                assert_eq!(m.outline[i * n], 0, "{name} rot {rot} clipped left");
                assert_eq!(m.outline[i * n + n - 1], 0, "{name} rot {rot} clipped right");
            }
        }
    }
    // Rotating by 90 degrees preserves the amount of ink exactly.
    let mut p = crosshair::presets()[0].1.clone();
    let a: u64 = build_masks(&p).fill.iter().map(|&v| v as u64).sum();
    p.rotation = 90.0;
    let b: u64 = build_masks(&p).fill.iter().map(|&v| v as u64).sum();
    assert_eq!(a, b);
}

#[test]
fn composed_pixels_are_valid_premultiplied() {
    let mut out = Canvas::new(0, 0);
    for (_, shape) in crosshair::presets() {
        for (rainbow, mode) in [(false, 0), (true, 0), (true, 1), (true, 2)] {
            let mut p = shape.clone();
            p.rainbow = rainbow;
            p.rainbow_mode = mode;
            p.opacity = 0.7;
            p.rotation = 20.0;
            let m = build_masks(&p);
            compose(&m, &p, 3.21, &mut out);
            assert_eq!(out.w % 2, 0);
            for &px in &out.px {
                let a = px >> 24;
                assert!(((px >> 16) & 255) <= a && ((px >> 8) & 255) <= a && (px & 255) <= a);
                assert!(a <= 179, "opacity not applied: {a}");
            }
        }
    }
    // Rainbow actually changes colour over time; solid colour does not.
    let mut p = Profile::default();
    let m = build_masks(&p);
    let mut a = Canvas::new(0, 0);
    let mut b = Canvas::new(0, 0);
    compose(&m, &p, 0.0, &mut a);
    compose(&m, &p, 2.0, &mut b);
    assert!(a.px == b.px);
    p.rainbow = true;
    compose(&m, &p, 0.0, &mut a);
    compose(&m, &p, 2.0, &mut b);
    assert!(a.px != b.px);
}

#[test]
fn image_scaling() {
    let img = decode_png(FIXTURES.iter().find(|f| f.0 == "cross").unwrap().1).unwrap();
    for (w, h, smooth) in [(64, 64, true), (32, 32, true), (128, 128, true), (128, 128, false), (7, 90, true), (1, 1, false)] {
        let c = scale_to(&img, w, h, smooth);
        assert_eq!((c.w, c.h), (w as i32, h as i32));
        for &px in &c.px {
            let a = px >> 24;
            assert!(((px >> 16) & 255) <= a && ((px >> 8) & 255) <= a && (px & 255) <= a);
        }
    }
    // Identity scale keeps opaque pixels exact and transparent pixels empty.
    let c = scale_to(&img, 64, 64, true);
    assert_eq!(c.px[0], 0);
    assert_eq!(c.px[32 * 64 + 2], 0xFFFF2828);
}

#[test]
fn image_import_select_delete() {
    let mut m = Model::new(temp_dir("img"), loader);
    let src = temp_dir("img-src");
    let file = src.join("my cross!.png");
    std::fs::write(&file, FIXTURES.iter().find(|f| f.0 == "cross").unwrap().1).unwrap();
    assert!(m.import_image(&file));
    assert_eq!(m.images, vec!["my cross.png".to_string()]);
    assert_eq!(m.cur.mode, 1);
    assert_eq!(m.cur.image, "my cross.png");
    // Importing the same file again gets a unique name.
    assert!(m.import_image(&file));
    assert_eq!(m.images.len(), 2);
    // Garbage and unsupported files are rejected and leave nothing behind.
    let bad = src.join("bad.png");
    std::fs::write(&bad, b"definitely not an image").unwrap();
    assert!(!m.import_image(&bad));
    let txt = src.join("notes.txt");
    std::fs::write(&txt, b"hi").unwrap();
    assert!(!m.import_image(&txt));
    assert_eq!(m.images.len(), 2);

    // The overlay renders the image at the requested scale, with tint + rainbow.
    let mut out = Canvas::new(0, 0);
    m.cur.img_scale = 0.5;
    m.render(&mut out);
    assert_eq!((out.w, out.h), (32, 32));
    assert!(out.px.iter().any(|&p| p >> 24 > 100));
    m.cur.img_scale = 1.0;
    m.cur.img_tint = true;
    m.cur.color = Color::hex(0x0000FF);
    m.render(&mut out);
    assert_eq!(out.px.iter().filter(|&&p| p >> 24 == 255).count(), 204); // 51 px x 2 thick x 2 axes
    assert!(out.px.iter().filter(|&&p| p >> 24 == 255).all(|&p| p == 0xFF0000FF));

    m.delete_image("my cross (2).png");
    assert_eq!(m.images.len(), 1);
    assert_eq!(m.cur.mode, 0);
    assert!(m.cur.image.is_empty());
}

#[test]
fn extreme_settings_do_not_panic() {
    let mut out = Canvas::new(0, 0);
    let mut rend = crosshair::Renderer::new(temp_dir("ext"), loader);
    let mut p = Profile::default();
    p.dot = true;
    p.outer = true;
    p.circle = true;
    for (len, thick, gap, scale, rot) in [(60.0, 20.0, 60.0, 4.0, 0.0), (60.0, 20.0, 100.0, 4.0, 33.0), (1.0, 1.0, 0.0, 0.5, 0.0), (1.0, 1.0, 0.0, 0.5, 359.0)] {
        p.inner_len = len;
        p.outer_len = len;
        p.inner_thick = thick;
        p.outer_thick = thick;
        p.inner_gap = gap;
        p.outer_gap = gap;
        p.circle_radius = 100.0;
        p.dot_size = 40.0;
        p.outline_thick = 6.0;
        p.scale = scale;
        p.rotation = rot;
        rend.render(&p, 1.0, &mut out);
        assert!(out.w <= 1024 && out.w % 2 == 0);
    }
    // Image mode with a missing file falls back to an empty canvas.
    p.mode = 1;
    p.image = "missing.png".into();
    rend.render(&p, 0.0, &mut out);
    assert!(out.px.iter().all(|&v| v == 0));
}

#[test]
fn ipc_roundtrip_and_model_text() {
    let mut buf = Vec::new();
    crate::ipc::write_msg(&mut buf, "state", "a=1\nb=2").unwrap();
    crate::ipc::write_msg(&mut buf, "quit", "").unwrap();
    let mut r = &buf[..];
    assert_eq!(crate::ipc::read_msg(&mut r).unwrap(), ("state".into(), "a=1\nb=2".into()));
    assert_eq!(crate::ipc::read_msg(&mut r).unwrap(), ("quit".into(), "".into()));
    assert!(crate::ipc::read_msg(&mut r).is_err());
    // A state message carries everything: the other side ends up with the same model.
    let mut a = Model::new(temp_dir("ipc-a"), loader);
    let mut b = Model::new(temp_dir("ipc-b"), loader);
    a.cur.inner_len = 9.0;
    a.set.offset_x = 12.0;
    a.set.display_mode = 1;
    a.save_profile("Mine");
    assert!(b.apply_text(&a.to_text()));
    assert_eq!((&a.set, &a.cur, &a.profiles), (&b.set, &b.cur, &b.profiles));
    assert!(!b.apply_text(&a.to_text()));
    assert_eq!(crate::ipc::token().len(), 64);
}

#[test]
fn aim_mode_and_old_hide_ads() {
    // old config with hide_ads=1 -> "hide while aiming"
    let (s, _, _) = config::parse("[app]\nhide_ads=1\n");
    assert_eq!((s.aim_mode, s.hide_ads), (2, true));
    let (s, _, _) = config::parse("[app]\naim_mode=1\nhide_ads=0\n");
    assert_eq!((s.aim_mode, s.hide_ads), (1, false));
    let (s, _, _) = config::parse("[app]\naim_mode=9\n");
    assert_eq!((s.aim_mode, s.hide_ads), (2, true));
    // Round trip stays the same.
    let (s, c, p) = config::parse("[app]\naim_mode=1\n");
    let t = config::serialize(&s, &c, &p);
    assert_eq!(config::parse(&t).0, s);
}
