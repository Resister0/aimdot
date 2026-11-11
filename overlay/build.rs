//! Windows (MSVC): puts the Aimdot icon into aimdot.exe (Explorer, taskbar, Alt+Tab, the tray icon).
//!
//! No crates and no resource compiler: this writes a tiny `.res` file (the format rc.exe produces) straight from
//! `assets/icons/aimdot.ico` and hands it to the linker - MSVC's link.exe takes `.res` files as input.
//! Resources: every picture of the .ico as RT_ICON 1..n, plus RT_GROUP_ICON 1 (= `ICON_ID` in win.rs).
//! Other targets (Linux, the windows-gnu type check) get nothing.

use std::path::PathBuf;

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const LANG_EN_US: u16 = 0x0409;

fn main() {
    let ico_path = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../assets/icons/aimdot.ico");
    println!("cargo:rerun-if-changed={}", ico_path.display());
    println!("cargo:rerun-if-changed=build.rs");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os != "windows" || env != "msvc" {
        return;
    }
    let ico = std::fs::read(&ico_path).expect("assets/icons/aimdot.ico");
    let res = res_from_ico(&ico).expect("assets/icons/aimdot.ico is not a valid .ico");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("aimdot-icon.res");
    std::fs::write(&out, res).unwrap();
    println!("cargo:rustc-link-arg-bins={}", out.display());
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

/// One resource entry: header (numeric type + name) + data, both padded to 4 bytes.
fn entry(out: &mut Vec<u8>, ty: u16, id: u16, flags: u16, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes()); // DataSize
    out.extend_from_slice(&32u32.to_le_bytes()); // HeaderSize
    out.extend_from_slice(&[0xFF, 0xFF]);
    out.extend_from_slice(&ty.to_le_bytes());
    out.extend_from_slice(&[0xFF, 0xFF]);
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // DataVersion
    out.extend_from_slice(&flags.to_le_bytes()); // MemoryFlags
    out.extend_from_slice(&(if ty == 0 { 0 } else { LANG_EN_US }).to_le_bytes()); // the empty first entry: 0
    out.extend_from_slice(&0u32.to_le_bytes()); // Version
    out.extend_from_slice(&0u32.to_le_bytes()); // Characteristics
    out.extend_from_slice(data);
    out.resize(out.len().next_multiple_of(4), 0);
}

fn res_from_ico(ico: &[u8]) -> Option<Vec<u8>> {
    if u16_at(ico, 0)? != 0 || u16_at(ico, 2)? != 1 {
        return None;
    }
    let n = u16_at(ico, 4)? as usize;
    let mut out = Vec::new();
    // A .res file starts with an empty entry (type 0, name 0) that marks it as 32-bit.
    entry(&mut out, 0, 0, 0, &[]);
    // GRPICONDIR + one 14-byte GRPICONDIRENTRY per picture (the .ico entry with an id instead of a file offset).
    let mut group = Vec::new();
    group.extend_from_slice(&[0, 0, 1, 0]);
    group.extend_from_slice(&(n as u16).to_le_bytes());
    for i in 0..n {
        let e = 6 + 16 * i;
        let head = ico.get(e..e + 8)?;
        let size = u32_at(ico, e + 8)? as usize;
        let off = u32_at(ico, e + 12)? as usize;
        let data = ico.get(off..off + size)?;
        let id = i as u16 + 1;
        entry(&mut out, RT_ICON, id, 0x1010, data); // MOVEABLE | DISCARDABLE
        group.extend_from_slice(&head[..4]); // width, height, colours, reserved
        let planes = u16_at(head, 4)?.max(1); // Pillow writes 0
        let bits = match u16_at(head, 6)? {
            0 => 32,
            b => b,
        };
        group.extend_from_slice(&planes.to_le_bytes());
        group.extend_from_slice(&bits.to_le_bytes());
        group.extend_from_slice(&(size as u32).to_le_bytes());
        group.extend_from_slice(&id.to_le_bytes());
    }
    entry(&mut out, RT_GROUP_ICON, 1, 0x1030, &group); // MOVEABLE | PURE | DISCARDABLE
    Some(out)
}
