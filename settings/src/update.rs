//! Auto-update, the settings program's part (the network + install side; the shared state is core/src/update.rs).
//! Same method as Echo's native app (Echo prototype-native/src/setup.rs check_updates / install / verify):
//! - ask `https://sonela.app/aimdot/update/<os>/<arch>/<version>`: 204 = up to date, 200 = `{version, notes,
//!   pub_date, url, signature}` (Tauri's updater format) - the server contract is in docs/release-plan.md;
//! - download `url`, check its minisign signature (`signature` = base64 of the .minisig text, Tauri signer format)
//!   against Aimdot's own updater public key; never install anything that doesn't verify;
//! - Windows (installed copy = uninstall.exe next to us): run `Aimdot-Setup.exe /P /R /UPDATE` (Tauri's passive
//!   update: progress only, restarts aimdot.exe) and exit;
//! - Linux AppImage: the new file over `$APPIMAGE`, quit the running overlay, start the new AppImage, exit;
//! - portable zip / tar.gz / no key: "Download" opens sonela.app/aimdot.
//!
//! Only release builds check (core::update::is_release_build); Flatpak updates itself.
//!
//! Hooked in from main.rs: `if update::cli() { return; }` first thing in main(), then `update::wire!(ui)` once the
//! window exists (settings/ui/update.slint: global `Upd`, `UpdateBanner`, `UpdateCard`).

use crosshair_core::update as cu;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Aimdot's updater public key = base64 of the minisign `.pub` file (Tauri signer format, like Echo's
/// UPDATER_PUBKEY). Set at build time from the repo variable AIMDOT_UPDATE_PUBKEY (CI), or paste it here once the key
/// exists (docs/release-plan.md, step 7). Public, not a secret. Empty = updates are shown, but the button
/// opens the download page instead of installing.
const PUBKEY_HERE: &str = "";

fn pubkey() -> &'static str {
    match option_env!("AIMDOT_UPDATE_PUBKEY") {
        Some(k) if !k.trim().is_empty() => k,
        _ => PUBKEY_HERE,
    }
}

/// `aimdot-settings --update` (the tray's "Update to Aimdot X"): install right after the window opens.
static AUTO_INSTALL: AtomicBool = AtomicBool::new(false);

fn log(msg: &str) {
    crosshair_core::log::line("update", msg);
}

fn in_flatpak() -> bool {
    std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists()
}

/// Checks at all (release build, not Flatpak).
fn checks() -> bool {
    cu::is_release_build() && !in_flatpak()
}

/// First thing in main(). `--check-update` (started by the overlay every few hours): check, write update.ini, exit
/// - true = main() returns at once, no window. `--update`: remember to install once the window is up.
pub fn cli() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--check-update") {
        if checks() {
            let _ = check_and_save();
        }
        return true;
    }
    if args.iter().any(|a| a == "--update") {
        AUTO_INSTALL.store(true, Ordering::Relaxed);
    }
    false
}

/// GET -> (status, body). reqwest blocking + rustls/ring like Echo (rustls without a default crypto provider:
/// ring is installed once). Redirects are followed (GitHub downloads redirect).
#[cfg(not(all(windows, target_env = "gnu")))]
fn http_get(url: &str) -> Result<(u16, Vec<u8>), String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let c = reqwest::blocking::Client::builder()
        .user_agent(format!("Aimdot/{}", cu::current_version()))
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
    let mut r = c.get(url).send().map_err(|e| format!("no connection ({e})"))?;
    let code = r.status().as_u16();
    let mut body = Vec::new();
    std::io::copy(&mut r, &mut body).map_err(|e| format!("download failed ({e})"))?;
    Ok((code, body))
}

/// The windows-gnu type check from Linux has no HTTPS crates (Cargo.toml) - never a real build.
#[cfg(all(windows, target_env = "gnu"))]
fn http_get(_url: &str) -> Result<(u16, Vec<u8>), String> {
    Err("no HTTPS in this build".into())
}

/// One check: the server's answer as a State (version "" = up to date).
fn check_once() -> Result<cu::State, String> {
    let (code, body) = http_get(&cu::check_url())?;
    let body = String::from_utf8_lossy(&body).to_string();
    let mut s = cu::State { checked: cu::now(), ..Default::default() };
    if code == 204 || body.trim().is_empty() {
        return Ok(s);
    }
    if code != 200 {
        return Err(format!("the update server answered {code}"));
    }
    let f = |k: &str| cu::json_str(&body, k).unwrap_or_default();
    s.version = f("version");
    if !s.newer() {
        s.version.clear();
        return Ok(s);
    }
    s.notes = f("notes");
    s.url = f("url");
    s.signature = f("signature");
    Ok(s)
}

/// Check + write update.ini (a failure keeps the last known update and notes the error).
fn check_and_save() -> Result<cu::State, String> {
    let r = check_once();
    let s = match &r {
        Ok(s) => s.clone(),
        Err(e) => cu::State { error: e.clone(), checked: cu::now(), ..cu::read() },
    };
    let _ = cu::write(&s);
    match &r {
        Ok(s) if s.version.is_empty() => log(&format!("up to date ({})", cu::current_version())),
        Ok(s) => log(&format!("{} available (this is {})", s.version, cu::current_version())),
        Err(e) => log(&format!("check failed: {e}")),
    }
    r
}

/// How this copy can update itself.
enum Kind {
    /// Windows, installed by Aimdot-Setup.exe
    Installer,
    /// Linux AppImage (the file to replace)
    AppImage(PathBuf),
    /// portable zip, tar.gz, own folder: the download page
    Manual,
}

fn kind() -> Kind {
    if cfg!(windows) {
        let installed = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("uninstall.exe").exists())).unwrap_or(false);
        return if installed { Kind::Installer } else { Kind::Manual };
    }
    match std::env::var_os("APPIMAGE").filter(|v| !v.is_empty()) {
        Some(a) => Kind::AppImage(PathBuf::from(a)),
        None => Kind::Manual,
    }
}

fn can_install(s: &cu::State) -> bool {
    !matches!(kind(), Kind::Manual) && !pubkey().is_empty() && !s.url.is_empty() && !s.signature.is_empty()
}

/// minisign check of the downloaded file against Aimdot's updater key (Echo's verify()).
fn verify(data: &[u8], sig_b64: &str) -> Result<(), String> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let text = |b: &str| -> Result<String, String> { String::from_utf8(b64.decode(b.trim()).map_err(|e| e.to_string())?).map_err(|e| e.to_string()) };
    let pk = minisign_verify::PublicKey::decode(&text(pubkey())?).map_err(|e| e.to_string())?;
    let sig = minisign_verify::Signature::decode(&text(sig_b64)?).map_err(|e| e.to_string())?;
    pk.verify(data, &sig, false).map_err(|e| e.to_string())
}

fn open_url(url: &str) {
    let opener = if cfg!(windows) { "explorer" } else { "xdg-open" };
    let _ = std::process::Command::new(opener).arg(url).spawn();
}

/// What the update UI shows (settings/ui/update.slint `Upd`).
#[derive(Clone, Debug, Default)]
pub struct View {
    /// a newer version is known: show the banner
    pub available: bool,
    pub version: String,
    pub notes: String,
    /// About > Updates line
    pub status: String,
    /// the banner / card button: "Update now", "Download", "Updating…"
    pub button: String,
    pub busy: bool,
    /// "Check for updates" makes sense (release build, not Flatpak)
    pub can_check: bool,
}

pub type Sink = Arc<dyn Fn(View) + Send + Sync>;

pub struct Updater {
    sink: Sink,
    busy: AtomicBool,
    status: Mutex<String>,
}

impl Updater {
    fn view(&self) -> View {
        let s = cu::read();
        let busy = self.busy.load(Ordering::Relaxed);
        let available = checks() && s.newer();
        let mut status = self.status.lock().map(|g| g.clone()).unwrap_or_default();
        if status.is_empty() {
            status = if !cu::is_release_build() {
                "This copy of Aimdot was built by hand and doesn't update itself. Get Aimdot from sonela.app/aimdot for updates.".into()
            } else if in_flatpak() {
                "Flatpak keeps Aimdot up to date.".into()
            } else if available {
                format!("Aimdot {} is out (you have {}).", s.version, cu::current_version())
            } else if !s.error.is_empty() {
                format!("Couldn't check for updates: {}", s.error)
            } else if s.checked > 0 {
                format!("You have the newest version ({}).", cu::current_version())
            } else {
                String::new()
            };
        }
        let button = if busy {
            "Updating…"
        } else if can_install(&s) {
            "Update now"
        } else {
            "Download"
        };
        View { available, version: s.version.clone(), notes: s.notes.clone(), status, button: button.into(), busy, can_check: checks() }
    }

    fn show(&self) {
        (self.sink)(self.view());
    }

    fn set_status(&self, s: impl Into<String>) {
        if let Ok(mut g) = self.status.lock() {
            *g = s.into();
        }
        self.show();
    }

    /// Check now (by hand: shows "Checking…" and the result in the About line).
    pub fn check(self: &Arc<Self>, by_hand: bool) {
        if !checks() || self.busy.load(Ordering::Relaxed) {
            self.show();
            return;
        }
        if by_hand {
            self.set_status("Checking…");
        }
        let u = self.clone();
        std::thread::spawn(move || {
            let r = check_and_save();
            match r {
                Ok(s) if s.version.is_empty() && by_hand => u.set_status(format!("You have the newest version ({}).", cu::current_version())),
                Err(e) if by_hand => u.set_status(format!("Couldn't check for updates: {e}")),
                _ => u.set_status(""),
            }
        });
    }

    /// The banner / card button.
    pub fn install(self: &Arc<Self>) {
        let s = cu::read();
        if !s.newer() {
            self.check(true);
            return;
        }
        if !can_install(&s) {
            open_url(cu::DOWNLOAD_PAGE);
            self.set_status(format!("Opened sonela.app/aimdot - download Aimdot {} there.", s.version));
            return;
        }
        if self.busy.swap(true, Ordering::Relaxed) {
            return;
        }
        self.set_status(format!("Downloading Aimdot {}…", s.version));
        let u = self.clone();
        std::thread::spawn(move || {
            if let Err(e) = download_and_install(&s, &u) {
                log(&format!("install failed: {e}"));
                u.busy.store(false, Ordering::Relaxed);
                u.set_status(format!("The update couldn't be installed: {e}. You can download Aimdot from sonela.app/aimdot."));
            }
        });
    }
}

/// Download, verify, install, restart. Returns only on failure (success ends this process).
fn download_and_install(s: &cu::State, u: &Updater) -> Result<(), String> {
    let (code, data) = http_get(&s.url)?;
    if code != 200 {
        return Err(format!("download failed (the server answered {code})"));
    }
    if let Err(e) = verify(&data, &s.signature) {
        log(&format!("signature check failed: {e}"));
        return Err("its signature didn't match, so it was not installed".into());
    }
    log(&format!("{} downloaded + verified ({} bytes)", s.version, data.len()));
    u.set_status(format!("Installing Aimdot {}…", s.version));
    match kind() {
        Kind::Installer => {
            let f = std::env::temp_dir().join(format!("Aimdot-Setup-{}.exe", s.version));
            std::fs::write(&f, &data).map_err(|e| e.to_string())?;
            // Tauri's passive update (like Echo): progress only, /R starts aimdot.exe again when done; the
            // installer stops both programs first (installer/hooks.nsh)
            std::process::Command::new(&f).args(["/P", "/R", "/UPDATE"]).spawn().map_err(|e| format!("the installer couldn't start ({e})"))?;
            log("installer started - exiting");
            std::process::exit(0);
        }
        Kind::AppImage(target) => {
            let tmp = target.with_extension("new");
            std::fs::write(&tmp, &data).map_err(|e| format!("couldn't write next to {} ({e})", target.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
            }
            std::fs::rename(&tmp, &target).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                format!("couldn't replace {} ({e})", target.display())
            })?;
            // the running overlay is the old version: quit it (our own copy still runs from the old mount), then
            // start the new AppImage (overlay + settings window)
            let _ = std::process::Command::new(crosshair_core::paths::sibling_exe(crosshair_core::OVERLAY_EXE)).arg("--quit").status();
            std::thread::sleep(Duration::from_millis(500));
            std::process::Command::new(&target).spawn().map_err(|e| format!("couldn't start the new version ({e})"))?;
            log("new AppImage started - exiting");
            std::process::exit(0);
        }
        Kind::Manual => Err("this copy can't replace itself".into()),
    }
}

/// Starts the updater for the window: shows what update.ini knows, checks now (unless the last check is under 10
/// minutes old) and then every few hours; `--update` installs at once.
pub fn start(sink: Sink) -> Arc<Updater> {
    let u = Arc::new(Updater { sink, busy: AtomicBool::new(false), status: Mutex::new(String::new()) });
    u.show();
    if checks() {
        let w = Arc::downgrade(&u);
        std::thread::spawn(move || {
            let mut first = true;
            loop {
                let Some(u) = w.upgrade() else { return };
                let every = if first { 10 * 60 } else { cu::CHECK_EVERY_SECS };
                if cu::due(every) {
                    let _ = check_and_save();
                    u.set_status("");
                }
                if first && AUTO_INSTALL.swap(false, Ordering::Relaxed) {
                    u.install();
                }
                first = false;
                drop(u);
                std::thread::sleep(Duration::from_secs(5 * 60));
            }
        });
    }
    u
}

/// Connects the `Upd` global (settings/ui/update.slint, exported from app.slint) to the updater. In main.rs, once
/// the window exists: `let _updater = update::wire!(ui);` (keep it alive as long as the window).
macro_rules! wire {
    ($ui:expr) => {{
        let w = $ui.as_weak();
        let sink: $crate::update::Sink = std::sync::Arc::new(move |v: $crate::update::View| {
            let _ = w.upgrade_in_event_loop(move |a| {
                let g = a.global::<Upd>();
                g.set_available(v.available);
                g.set_version(v.version.into());
                g.set_notes(v.notes.into());
                g.set_status(v.status.into());
                g.set_button(v.button.into());
                g.set_busy(v.busy);
                g.set_can_check(v.can_check);
            });
        });
        let u = $crate::update::start(sink);
        {
            let u = u.clone();
            $ui.global::<Upd>().on_check(move || u.check(true));
        }
        {
            let u = u.clone();
            $ui.global::<Upd>().on_install(move || u.install());
        }
        u
    }};
}
#[allow(unused_imports)]
pub(crate) use wire;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_key_means_no_install() {
        // no updater key compiled in (own build) -> never "Update now", always the download page
        if pubkey().is_empty() {
            let s = cu::State { version: "99.0.0".into(), url: "https://x/y".into(), signature: "abc".into(), ..Default::default() };
            assert!(!can_install(&s));
        }
    }

    #[test]
    fn bad_signature_is_rejected() {
        assert!(verify(b"data", "not base64 !!").is_err());
    }
}
