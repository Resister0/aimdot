//! "Start with my PC" (config key `autostart`, default off).
//! Windows: the overlay already writes / removes its Run value when the key changes (overlay/src/win.rs) - nothing here.
//! Linux: this window writes / removes the XDG autostart entry `~/.config/autostart/link.sonela.Aimdot.desktop`,
//! which starts the OVERLAY (`aimdot --autostart`: it opens this window too unless "Start minimised" is on).
//! Echo's lesson (rec_core.rs, 10 Nov 2026): an entry that starts ANOTHER Aimdot which still exists (an installed
//! AppImage, another build folder) is left alone - a dev build must not take it over, nor remove it when switched off.

/// What happened, for the note under the switch: "" = fine, else a short text
#[cfg(target_os = "linux")]
pub fn apply(on: bool) -> String {
    use std::path::PathBuf;
    let cfg = match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d),
        // a test copy (CROSSHAIR_HOME) never touches the real ~/.config/autostart - only a scratch XDG_CONFIG_HOME
        None if std::env::var_os("CROSSHAIR_HOME").is_some() => return String::new(),
        None => match std::env::var_os("HOME") {
            Some(h) => PathBuf::from(h).join(".config"),
            None => return String::new(),
        },
    };
    let dir = cfg.join("autostart");
    let f = dir.join(format!("{}.desktop", crate::APP_ID));
    // the overlay next to this program (an AppImage: the AppImage itself, which starts the overlay with --autostart ⚠ untested)
    let exe = std::env::var("APPIMAGE")
        .ok()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| crosshair_core::paths::sibling_exe(crosshair_core::OVERLAY_EXE).to_string_lossy().into_owned());
    if let Some(p) = other_entry(&f, &exe) {
        return format!("Another Aimdot already starts with your PC ({p}) - left as it is.");
    }
    if on {
        let text = format!(
            "[Desktop Entry]\nType=Application\nName=Aimdot\nComment=Crosshair overlay\nExec=\"{exe}\" --autostart\nIcon={id}\nTerminal=false\nX-GNOME-Autostart-enabled=true\nX-KDE-autostart-phase=2\n",
            id = crate::APP_ID
        );
        if std::fs::read_to_string(&f).ok().as_deref() != Some(text.as_str()) {
            let _ = std::fs::create_dir_all(&dir);
            if let Err(e) = std::fs::write(&f, text) {
                return format!("Couldn't write {}: {e}", f.display());
            }
        }
    } else {
        let _ = std::fs::remove_file(&f);
    }
    String::new()
}

#[cfg(not(target_os = "linux"))]
pub fn apply(_on: bool) -> String {
    String::new()
}

/// The program an existing entry starts, when that is not us and still exists
#[cfg(target_os = "linux")]
fn other_entry(f: &std::path::Path, exe: &str) -> Option<String> {
    let s = std::fs::read_to_string(f).ok()?;
    let e = s.lines().find_map(|l| l.strip_prefix("Exec="))?.trim().to_string();
    let path = match e.strip_prefix('"') {
        Some(r) => r.split('"').next()?.to_string(),
        None => e.split_whitespace().next()?.to_string(),
    };
    (path != exe && std::path::Path::new(&path).exists()).then_some(path)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn another_existing_aimdot_is_left_alone() {
        let d = std::env::temp_dir().join(format!("aimdot-autostart-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        let f = d.join("x.desktop");
        // an entry for a program that exists (/bin/sh) and isn't us: left alone
        std::fs::write(&f, "[Desktop Entry]\nExec=\"/bin/sh\" --tray\n").unwrap();
        assert_eq!(super::other_entry(&f, "/somewhere/aimdot").as_deref(), Some("/bin/sh"));
        // our own entry, or one whose program is gone: ours to change
        assert_eq!(super::other_entry(&f, "/bin/sh"), None);
        std::fs::write(&f, "[Desktop Entry]\nExec=/gone/aimdot --tray\n").unwrap();
        assert_eq!(super::other_entry(&f, "/somewhere/aimdot"), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
