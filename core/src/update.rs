//! Auto-update: the shared, std-only part (no network code here - the overlay stays crate-free).
//!
//! Who does what (docs/release-plan.md "Auto-update"):
//! - `aimdot-settings --check-update` (settings/src/update.rs) asks
//!   `https://sonela.app/aimdot/update/<windows|linux>/<x86_64|aarch64>/<version>`: 204 = up to date, 200 = JSON
//!   `{version, notes, pub_date, url, signature}` (Echo's / Tauri's updater format). The answer goes into
//!   `update.ini` in the config folder (this file reads and writes it).
//! - The overlay starts that check in the background every few hours (`spawn_check_if_due`) and shows
//!   "Update to Aimdot X" in its tray menu (`tray_label`); a click opens `aimdot-settings --update`.
//! - The settings window checks on start + every few hours, shows the update, and installs it (download, minisign
//!   check, then the installer / the new AppImage).
//!
//! Only release builds (CI sets AIMDOT_RELEASE=1 at build time) ever check; own builds never do.

use std::path::PathBuf;

/// The update server (the sonela-hub Worker). `AIMDOT_UPDATE_URL` overrides it at run time (tests).
pub const UPDATE_BASE: &str = "https://sonela.app/aimdot/update";
/// Where people download Aimdot by hand (portable zip, tar.gz, or when an update can't install itself).
pub const DOWNLOAD_PAGE: &str = "https://sonela.app/aimdot";
/// How often the overlay starts a background check.
pub const CHECK_EVERY_SECS: u64 = 4 * 3600;

/// True for CI release builds (AIMDOT_RELEASE=1 when this was compiled). Own builds never check for updates.
pub fn is_release_build() -> bool {
    option_env!("AIMDOT_RELEASE").is_some_and(|v| v == "1") || std::env::var_os("AIMDOT_TEST_RELEASE").is_some()
}

/// This copy's version (`AIMDOT_FAKE_VERSION` for tests).
pub fn current_version() -> String {
    std::env::var("AIMDOT_FAKE_VERSION").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| crate::VERSION.to_string())
}

pub fn os() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

pub fn arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    }
}

/// The URL this copy asks.
pub fn check_url() -> String {
    let base = std::env::var("AIMDOT_UPDATE_URL").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| UPDATE_BASE.to_string());
    format!("{}/{}/{}/{}", base.trim_end_matches('/'), os(), arch(), current_version())
}

/// a > b for "x.y.z" versions (anything after - or + is ignored).
pub fn ver_gt(a: &str, b: &str) -> bool {
    let p = |s: &str| -> Vec<u64> { s.trim().trim_start_matches('v').split(['.', '-', '+']).take(3).map(|x| x.parse().unwrap_or(0)).collect() };
    let (x, y) = (p(a), p(b));
    for i in 0..3 {
        let (u, w) = (x.get(i).copied().unwrap_or(0), y.get(i).copied().unwrap_or(0));
        if u != w {
            return u > w;
        }
    }
    false
}

/// What the last check found (`update.ini`). `version` empty = up to date.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub version: String,
    pub notes: String,
    pub url: String,
    pub signature: String,
    /// unix seconds of the last answer (or failure)
    pub checked: u64,
    /// last failure ("" = the last check worked)
    pub error: String,
}

impl State {
    /// A newer version than this copy is known.
    pub fn newer(&self) -> bool {
        !self.version.is_empty() && ver_gt(&self.version, &current_version())
    }
}

pub fn state_path() -> PathBuf {
    crate::paths::config_dir().join("update.ini")
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Values on one line: \ and newlines escaped.
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "")
}

fn unesc(s: &str) -> String {
    let mut o = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => o.push('\n'),
                Some(x) => o.push(x),
                None => {}
            }
        } else {
            o.push(c);
        }
    }
    o
}

pub fn read() -> State {
    let mut s = State::default();
    let Ok(t) = std::fs::read_to_string(state_path()) else { return s };
    for l in t.lines() {
        let Some((k, v)) = l.split_once('=') else { continue };
        let v = unesc(v);
        match k.trim() {
            "version" => s.version = v,
            "notes" => s.notes = v,
            "url" => s.url = v,
            "signature" => s.signature = v,
            "checked" => s.checked = v.trim().parse().unwrap_or(0),
            "error" => s.error = v,
            _ => {}
        }
    }
    s
}

/// Temp file + rename (the overlay may read it at the same moment).
pub fn write(s: &State) -> std::io::Result<()> {
    let p = state_path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let text = format!(
        "# Aimdot update check (written by aimdot-settings; safe to delete)\nversion={}\nnotes={}\nurl={}\nsignature={}\nchecked={}\nerror={}\n",
        esc(&s.version),
        esc(&s.notes),
        esc(&s.url),
        esc(&s.signature),
        s.checked,
        esc(&s.error)
    );
    let tmp = p.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &p)
}

/// A check is due: release build, and the last one is `every` seconds old (a failed one: retried after 15 min).
pub fn due(every: u64) -> bool {
    if !is_release_build() {
        return false;
    }
    let s = read();
    let wait = if s.error.is_empty() { every } else { every.min(15 * 60) };
    now().saturating_sub(s.checked) >= wait
}

/// The tray menu item, e.g. "Update to Aimdot 0.1.7" - None when nothing newer is known.
pub fn tray_label() -> Option<String> {
    let s = read();
    (is_release_build() && s.newer()).then(|| format!("Update to Aimdot {}", s.version))
}

/// Overlay: start `aimdot-settings --check-update` in the background when a check is due (it writes update.ini
/// and exits; nothing waits for it). Cheap to call often (reads one small file).
pub fn spawn_check_if_due() {
    if !due(CHECK_EVERY_SECS) {
        return;
    }
    // mark it as started now, so a slow / failing check isn't started again on the next call
    let mut s = read();
    s.checked = now();
    let _ = write(&s);
    let _ = std::process::Command::new(crate::paths::sibling_exe(crate::SETTINGS_EXE)).arg("--check-update").spawn();
}

/// Overlay tray click: open the settings window and install the known update there.
pub fn spawn_install() {
    let _ = std::process::Command::new(crate::paths::sibling_exe(crate::SETTINGS_EXE)).arg("--update").spawn();
}

/// The string value of `"key": "..."` in a flat JSON object (the server's answer; no JSON crate in core).
pub fn json_str(body: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let mut rest = &body[body.find(&pat)? + pat.len()..];
    rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let mut it = rest.strip_prefix('"')?.chars();
    let mut o = String::new();
    loop {
        match it.next()? {
            '"' => return Some(o),
            '\\' => match it.next()? {
                'n' => o.push('\n'),
                't' => o.push('\t'),
                'r' => {}
                'u' => {
                    let h: String = (0..4).filter_map(|_| it.next()).collect();
                    let c = u32::from_str_radix(&h, 16).ok().and_then(char::from_u32).unwrap_or('\u{fffd}');
                    o.push(c);
                }
                x => o.push(x),
            },
            c => o.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(ver_gt("0.1.10", "0.1.9"));
        assert!(ver_gt("1.0.0", "0.9.99"));
        assert!(!ver_gt("0.1.3", "0.1.3"));
        assert!(!ver_gt("0.1.2", "0.1.3"));
        assert!(ver_gt("v0.2.0", "0.1.3-dev"));
    }

    #[test]
    fn json_fields() {
        let b = r#"{"version":"0.1.7","notes":"Line one\nsaid \"hi\" \u00e9","pub_date":"2026-11-10T20:00:00Z","url":"https://github.com/Resister0/aimdot/releases/download/latest/Aimdot-Setup.exe","signature":"dW50cnVzdGVk"}"#;
        assert_eq!(json_str(b, "version").as_deref(), Some("0.1.7"));
        assert_eq!(json_str(b, "notes").as_deref(), Some("Line one\nsaid \"hi\" é"));
        assert_eq!(json_str(b, "signature").as_deref(), Some("dW50cnVzdGVk"));
        assert_eq!(json_str(b, "missing"), None);
        assert_eq!(json_str(r#"{ "version" : "1.2.3" }"#, "version").as_deref(), Some("1.2.3"));
    }

    #[test]
    fn state_values_stay_on_one_line() {
        let notes = "a\\b\nc=d";
        let line = esc(notes);
        assert!(!line.contains('\n'));
        assert_eq!(unesc(&line), notes);
    }
}
