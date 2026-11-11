//! Linux: the app menu entry + icons for a downloaded Aimdot (AppImage or tar.gz), written into the user's own
//! folders on start - like Echo (`prototype-native/src/linux.rs` desktop_entry(), 10 Nov 2026).
//!
//! Why: KDE's taskbar and Alt+Tab take a window's icon only from a VISIBLE `.desktop` file named exactly like the
//! window's app_id (`link.sonela.Aimdot`, settings/src/main.rs) - without one, Alt+Tab shows a generic icon. So:
//! - never `NoDisplay` (a hidden entry gives no Alt+Tab icon), `StartupWMClass` = the app_id;
//! - written only when missing, when it differs, or when it starts a copy that no longer exists;
//! - an entry that starts ANOTHER existing Aimdot (another AppImage, the dev build's entry from
//!   `scripts/install-dev-linux.sh`) is left alone - the window still gets the icon through it;
//! - the icons (hicolor 16-512 + SVG) are rewritten when missing or a different size (same names for every copy).
//!
//! Not for: the Flatpak (brings its own), cargo builds in `target/` (the dev script does those), test copies
//! (`CROSSHAIR_HOME` set without a scratch `XDG_DATA_HOME`), or an AppImage run without `$APPIMAGE`.
//!
//! Std only, no crates; the icons are compiled in but only read when they're written, so the overlay's memory
//! doesn't change.

use std::path::{Path, PathBuf};

pub const APP_ID: &str = "link.sonela.Aimdot";

/// (size, PNG) for hicolor/<size>x<size>/apps, from assets/icons (make_icons.py).
pub const ICONS: &[(u32, &[u8])] = &[
    (16, include_bytes!("../../assets/icons/aimdot-16.png")),
    (24, include_bytes!("../../assets/icons/aimdot-24.png")),
    (32, include_bytes!("../../assets/icons/aimdot-32.png")),
    (48, include_bytes!("../../assets/icons/aimdot-48.png")),
    (64, include_bytes!("../../assets/icons/aimdot-64.png")),
    (128, include_bytes!("../../assets/icons/aimdot-128.png")),
    (256, include_bytes!("../../assets/icons/aimdot-256.png")),
    (512, include_bytes!("../../assets/icons/aimdot-512.png")),
];
pub const SVG: &[u8] = include_bytes!("../../assets/icons/aimdot.svg");

/// What `ensure` did (for a log line / the tests).
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Not a packaged copy (Flatpak, cargo build, test copy...) - nothing touched.
    Skipped(&'static str),
    /// The entry starts another Aimdot that still exists (path) - entry left alone (icons still checked).
    OtherCopy(String),
    /// Entry written (new, changed, or the copy it started is gone).
    Written,
    /// Entry already right.
    Current,
    /// Couldn't write (message).
    Failed(String),
}

/// Call once at start (aimdot-settings and the overlay). Cheap when everything is in place: reads one small file
/// and the icon files.
pub fn ensure() -> Outcome {
    if !cfg!(target_os = "linux") {
        return Outcome::Skipped("not Linux");
    }
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    if var("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists() {
        return Outcome::Skipped("flatpak");
    }
    // test copies must never reach the real desktop folders
    if var("CROSSHAIR_HOME").is_some() && var("XDG_DATA_HOME").is_none() {
        return Outcome::Skipped("test copy without XDG_DATA_HOME");
    }
    let Some(data) = var("XDG_DATA_HOME").map(PathBuf::from).or_else(|| var("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    else {
        return Outcome::Skipped("no HOME");
    };
    let exe = match var("APPIMAGE") {
        Some(a) => PathBuf::from(a),
        None => {
            if var("APPDIR").is_some() {
                return Outcome::Skipped("AppImage without $APPIMAGE");
            }
            let Ok(me) = std::env::current_exe() else { return Outcome::Skipped("no exe path") };
            // the menu starts the overlay (it opens the settings window); fall back to this program
            let overlay = me.with_file_name(crate::OVERLAY_EXE);
            if overlay.exists() { overlay } else { me }
        }
    };
    ensure_at(&data, &exe)
}

/// `ensure` with explicit folders (tests use a scratch folder).
pub fn ensure_at(data: &Path, exe: &Path) -> Outcome {
    if is_cargo_build(exe) {
        return Outcome::Skipped("cargo build (scripts/install-dev-linux.sh does dev builds)");
    }
    let exe_s = exe.to_string_lossy().to_string();
    let icons = data.join("icons/hicolor");
    for (size, png) in ICONS {
        write_if_differs(&icons.join(format!("{size}x{size}/apps/{APP_ID}.png")), png);
    }
    write_if_differs(&icons.join(format!("scalable/apps/{APP_ID}.svg")), SVG);

    let file = data.join(format!("applications/{APP_ID}.desktop"));
    let old = std::fs::read_to_string(&file).ok();
    if let Some(p) = old.as_deref().and_then(exec_path) {
        if p != exe_s && Path::new(&p).exists() {
            return Outcome::OtherCopy(p);
        }
    }
    let entry = entry_text(&exe_s);
    if old.as_deref() == Some(entry.as_str()) {
        return Outcome::Current;
    }
    match write_atomic(&file, entry.as_bytes()) {
        Ok(()) => Outcome::Written,
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

/// The .desktop text for this copy (same fields as scripts/install-dev-linux.sh and the AppImage's own entry).
pub fn entry_text(exe: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Aimdot\nGenericName=Crosshair overlay\n\
         Comment=A light crosshair overlay - change your crosshair here\nExec=\"{}\"\nIcon={APP_ID}\n\
         StartupWMClass={APP_ID}\nCategories=Game;\nKeywords=crosshair;overlay;aim;\nTerminal=false\n",
        quote(exe)
    )
}

/// Inside "..." of an Exec line: " ` $ \ get a backslash (desktop entry spec: quoting), then every backslash is
/// doubled (string values escape \ as \\).
fn quote(s: &str) -> String {
    let mut arg = String::new();
    for c in s.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            arg.push('\\');
        }
        arg.push(c);
    }
    arg.replace('\\', "\\\\")
}

/// The program an entry starts: the first word of `Exec=` (quoted or not), unescaped (both levels).
fn exec_path(text: &str) -> Option<String> {
    let raw = text.lines().find_map(|l| l.strip_prefix("Exec="))?.trim();
    // value level: \\ -> \ (other escapes like \s don't occur in a path we wrote)
    let mut e = String::new();
    let mut it = raw.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next()? {
                's' => e.push(' '),
                n => e.push(n),
            }
        } else {
            e.push(c);
        }
    }
    let Some(r) = e.strip_prefix('"') else { return e.split_whitespace().next().map(str::to_string) };
    // inside the quotes: \x -> x, up to the closing quote
    let mut o = String::new();
    let mut it = r.chars();
    while let Some(c) = it.next() {
        match c {
            '\\' => o.push(it.next()?),
            '"' => return Some(o),
            _ => o.push(c),
        }
    }
    None
}

fn is_cargo_build(exe: &Path) -> bool {
    let s = exe.to_string_lossy();
    s.contains("/target/release/") || s.contains("/target/debug/") || s.contains("/target/x86_64-")
}

/// Same size = same icon (a new icon set changes the sizes): the compiled-in bytes are only touched when they're
/// written, so they never come into the overlay's memory on a normal start.
fn write_if_differs(p: &Path, bytes: &[u8]) {
    if std::fs::metadata(p).map_or(true, |m| m.len() != bytes.len() as u64) {
        let _ = write_atomic(p, bytes);
    }
}

/// Temp file + rename: the overlay and the settings window may both do this at the same moment.
fn write_atomic(p: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, p).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aimdot-de-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn writes_entry_and_icons_then_current() {
        let d = scratch("new");
        let exe = d.join("Aimdot-x86_64.AppImage");
        std::fs::write(&exe, b"x").unwrap();
        let data = d.join("share");
        assert_eq!(ensure_at(&data, &exe), Outcome::Written);
        let e = std::fs::read_to_string(data.join("applications/link.sonela.Aimdot.desktop")).unwrap();
        assert!(e.contains("StartupWMClass=link.sonela.Aimdot\n") && e.contains("Icon=link.sonela.Aimdot\n"));
        assert!(!e.contains("NoDisplay"));
        assert_eq!(exec_path(&e).as_deref(), Some(exe.to_str().unwrap()));
        for (s, png) in ICONS {
            assert_eq!(std::fs::read(data.join(format!("icons/hicolor/{s}x{s}/apps/link.sonela.Aimdot.png"))).unwrap(), *png);
        }
        assert!(data.join("icons/hicolor/scalable/apps/link.sonela.Aimdot.svg").exists());
        assert_eq!(ensure_at(&data, &exe), Outcome::Current);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn leaves_another_existing_copy_alone() {
        let d = scratch("other");
        let data = d.join("share");
        let other = d.join("dev/aimdot-settings");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&other, b"x").unwrap();
        let f = data.join("applications/link.sonela.Aimdot.desktop");
        let text = format!("[Desktop Entry]\nExec=\"{}\"\n", other.display());
        write_atomic(&f, text.as_bytes()).unwrap();
        let me = d.join("Aimdot.AppImage");
        assert_eq!(ensure_at(&data, &me), Outcome::OtherCopy(other.to_string_lossy().to_string()));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), text);
        // its copy is gone -> taken over
        std::fs::remove_file(&other).unwrap();
        assert_eq!(ensure_at(&data, &me), Outcome::Written);
        assert_eq!(exec_path(&std::fs::read_to_string(&f).unwrap()).as_deref(), Some(me.to_str().unwrap()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn quoting_round_trips_and_cargo_builds_skip() {
        for p in ["/opt/my apps/aimdot", "/opt/a$b\"c`d\\e/aimdot", "/plain/aimdot"] {
            assert_eq!(exec_path(&entry_text(p)).as_deref(), Some(p));
        }
        assert_eq!(exec_path("Exec=/usr/bin/aimdot --tray %U\n").as_deref(), Some("/usr/bin/aimdot"));
        assert!(matches!(ensure_at(Path::new("/nonexistent"), Path::new("/r/target/release/aimdot")), Outcome::Skipped(_)));
    }
}
