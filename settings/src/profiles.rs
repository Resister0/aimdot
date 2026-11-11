//! Profiles page: the saved crosshairs (save, use, rename, order, delete), share codes (copy / paste: Aimdot
//! "AIM1:" codes, CS2 "CSGO-" codes, Valorant profile codes) and per-game switching (which games use which
//! profile; the overlay does the switching - see core/src/games.rs and overlay/src/games.rs).
//! The clipboard is Slint's own (a hidden TextInput in profiles.slint), so no extra crate.

use super::{to_image, with_app, App, AppWindow, Prog, ProfItem, Profs};
use crosshair_core::config::Profile;
use crosshair_core::gfx::Canvas;
use crosshair_core::{games, import, share};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Default)]
struct St {
    ui: Option<slint::Weak<AppWindow>>,
    /// What the list shows; rebuilt only when it changes.
    key: String,
    /// Thumbnails by the profile's settings text.
    thumbs: HashMap<String, slint::Image>,
}

thread_local! {
    static ST: RefCell<St> = RefCell::new(St::default());
}

fn win() -> Option<AppWindow> {
    ST.with(|s| s.borrow().ui.as_ref().and_then(|w| w.upgrade()))
}

fn status(text: impl Into<SharedString>, ok: bool) {
    if let Some(ui) = win() {
        let g = ui.global::<Profs>();
        g.set_status(text.into());
        g.set_status_ok(ok);
    }
}

fn ptext(p: &Profile) -> String {
    let mut s = String::new();
    p.write(&mut s);
    s
}

/// What the per-game switch can see on this system (shown on the page): Linux only, Windows sees every game.
fn games_note() -> &'static str {
    if cfg!(windows) {
        ""
    } else {
        "On Linux, games are detected when they run through Steam / Proton or XWayland."
    }
}

/// Pushes the list into the page (called from App::refresh).
pub fn refresh(a: &mut App) {
    let Some(ui) = a.ui() else { return };
    let g = ui.global::<Profs>();
    let set = &a.m.set;
    let rules = games::parse(&set.games);
    let active = set.active_profile.clone();
    let act = a.m.profiles.iter().find(|p| p.0 == active);
    let changed = act.map_or(false, |p| p.1 != a.m.cur);
    g.set_active(if act.is_some() { active.as_str().into() } else { SharedString::new() });
    g.set_active_changed(changed);
    g.set_games_on(set.games_on);
    let (n, p) = (set.hk_next, set.hk_prev);
    g.set_hk_text(match (n.vk != 0, p.vk != 0) {
        (true, true) => format!("{} / {}", n.name(), p.name()),
        (true, false) => n.name(),
        (false, true) => p.name(),
        _ => String::new(),
    }
    .into());

    let mut key = format!("{active}|{changed}|{}|", set.games);
    let texts: Vec<String> = a.m.profiles.iter().map(|(_, p)| ptext(p)).collect();
    for ((n, _), t) in a.m.profiles.iter().zip(&texts) {
        key.push_str(n);
        key.push('\u{1}');
        key.push_str(t);
    }
    if ST.with(|s| s.borrow().key == key) {
        return;
    }
    let mut thumbs = ST.with(|s| std::mem::take(&mut s.borrow_mut().thumbs));
    let mut keep = HashMap::new();
    let mut items = Vec::new();
    for ((name, p), t) in a.m.profiles.clone().iter().zip(texts) {
        let img = match thumbs.remove(&t) {
            Some(i) => i,
            None => {
                let q = p.clone();
                let mut c = Canvas::new(0, 0);
                a.m.rend.render(&q, 0.0, &mut c);
                to_image(&c)
            }
        };
        keep.insert(t, img.clone());
        let on = *name == active;
        let g: Vec<SharedString> = rules.iter().filter(|r| r.profile == *name).map(|r| r.game.as_str().into()).collect();
        let games_text: Vec<&str> = g.iter().map(|x| x.as_str()).collect();
        let games_text = SharedString::from(games_text.join(", "));
        items.push(ProfItem { name: name.as_str().into(), img, on, changed: on && changed, games: ModelRc::new(VecModel::from(g)), games_text, pixel: p.mode == 1 });
    }
    g.set_items(ModelRc::new(VecModel::from(items)));
    g.set_games_note(games_note().into());
    refresh_progs_used(&g, &rules);
    ST.with(|s| {
        let mut s = s.borrow_mut();
        s.key = key;
        s.thumbs = keep;
    });
}

/// The running-programs chips show which profile each one already belongs to.
fn refresh_progs_used(g: &Profs, rules: &[games::Rule]) {
    use slint::Model as _;
    let progs = g.get_progs();
    if progs.row_count() == 0 {
        return;
    }
    let v: Vec<Prog> = progs
        .iter()
        .map(|mut p| {
            p.used = rules.iter().find(|r| r.game == p.key.as_str()).map_or(SharedString::new(), |r| r.profile.as_str().into());
            p
        })
        .collect();
    g.set_progs(ModelRc::new(VecModel::from(v)));
}

/// The code of a profile (an image one carries its picture). Err = message for the user.
pub fn code_for(a: &mut App, name: &str, p: &Profile) -> Result<String, String> {
    if p.mode == 1 {
        let img = a.m.rend.image(&p.image).ok_or_else(|| "The picture of this crosshair could not be read".to_string())?;
        // an animated picture ("Name (anim 4x8).png", all frames side by side) travels with its frames + speed
        let anim = crosshair_core::paint::anim_of(&p.image).map(|(f, fps)| (f as u32, fps.round() as u32));
        share::encode_anim(name, p, Some(&img), anim).map_err(|e| e.to_string())
    } else {
        share::encode_profile(name, p).map_err(|e| e.to_string())
    }
}

/// "stem (anim 4x8).png", or "stem (2) (anim 4x8).png" ... when taken.
fn free_anim_name(dir: &std::path::Path, stem: &str, frames: usize, fps: f32) -> String {
    let stem = super::images::clean_stem(stem);
    let mut name = crosshair_core::paint::anim_name(&stem, frames, fps);
    let mut n = 2;
    while dir.join(&name).exists() {
        name = crosshair_core::paint::anim_name(&format!("{stem} ({n})"), frames, fps);
        n += 1;
    }
    name
}

/// Reads a pasted code (Aimdot, CS2 or Valorant). Pictures are saved into the image library.
/// Returns the name and the crosshair, plus a note on what didn't carry over (CS2 / Valorant).
pub fn read_code(a: &mut App, text: &str) -> Result<(String, Profile, String), String> {
    match share::decode(text) {
        Ok(s) => {
            let mut p = s.profile;
            if let Some(img) = s.pixels {
                let dir = a.m.rend.img_dir.clone();
                let _ = std::fs::create_dir_all(&dir);
                let stem = if s.name.is_empty() { "Shared".to_string() } else { s.name.clone() };
                let file = match s.anim {
                    Some((f, fps)) => free_anim_name(&dir, &stem, f as usize, fps as f32),
                    None => super::images::free_name(&dir, &stem),
                };
                let png = super::images::encode_png(&img)?;
                super::images::write_atomic(&dir.join(&file), &png)?;
                a.m.rend.forget(&file);
                a.m.scan_images();
                p.image = file;
            }
            Ok((s.name, p, String::new()))
        }
        Err(share::Error::NoCode) => match import::read(text, screen_h()) {
            Some(Ok(im)) => Ok((im.name, im.profile, im.note)),
            Some(Err(e)) => Err(e),
            None => Err("No code found. Aimdot codes start with AIM1:, CS2 codes with CS, Valorant codes with 0;".into()),
        },
        Err(e) => Err(e.to_string()),
    }
}

/// The height of the screen this window is on (CS2 codes are scaled to it); 1080 when unknown.
fn screen_h() -> u32 {
    use slint::winit_030::WinitWindowAccessor;
    win().and_then(|u| u.window().with_winit_window(|w| w.current_monitor().map(|m| m.size().height)).flatten()).filter(|&h| h >= 240).unwrap_or(1080)
}

/// Running programs a game rule could be made for. Likely games first.
pub fn running() -> Vec<String> {
    let mut v = list_programs();
    let me = [crosshair_core::OVERLAY_EXE, crosshair_core::SETTINGS_EXE];
    v.retain(|(n, _)| !me.contains(&games::exe_key(n).as_str()) && !games::exe_key(n).is_empty());
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    let mut seen = Vec::new();
    v.into_iter()
        .filter(|(n, _)| {
            let k = games::exe_key(n);
            let new = !seen.contains(&k);
            seen.push(k);
            new
        })
        .map(|(n, _)| n)
        .take(45)
        .collect()
}

/// Programs that are never games (Proton / Wine helpers, the desktop).
const NOT_GAMES: [&str; 34] = [
    "wineserver", "wine", "wine64", "wine-preloader", "wine64-preloader", "services", "winedevice", "explorer", "plugplay",
    "svchost", "rpcss", "conhost", "steam", "steamwebhelper", "tabtip", "start", "winedbg", "rundll32", "pv-bwrap",
    "pressure-vessel-wrap", "srt-bwrap", "reaper", "steam-runtime-launcher-service", "gameoverlayui", "applicationframehost",
    "textinputhost", "systemsettings", "shellexperiencehost", "searchhost", "startmenuexperiencehost", "lockapp",
    "searchapp", "crashpad_handler", "secur32",
];

/// (name, likely a game). Linux: the programs with an X11 / XWayland window (what the per-game switch can see)
/// plus this user's Wine / Proton programs; Windows: programs with a visible window.
#[cfg(not(windows))]
fn list_programs() -> Vec<(String, bool)> {
    let mut out = x11_programs();
    for (name, _) in wine_programs() {
        out.push((name, true));
    }
    out.retain(|(n, _)| !NOT_GAMES.contains(&games::exe_key(n).as_str()));
    out
}

/// What a pid runs: (shown name, likely a game). Wine / Proton: argv[0] is the Windows program ("C:\...\Game.exe").
#[cfg(not(windows))]
fn pid_program(pid: u32) -> Option<(String, bool)> {
    let dir = std::path::PathBuf::from(format!("/proc/{pid}"));
    let exe = std::fs::read_link(dir.join("exe")).map(|p| p.to_string_lossy().trim_end_matches(" (deleted)").to_string()).unwrap_or_default();
    let cmd = std::fs::read(dir.join("cmdline")).unwrap_or_default();
    let argv0 = String::from_utf8_lossy(cmd.split(|&b| b == 0).next().unwrap_or(&[])).into_owned();
    if argv0.to_ascii_lowercase().ends_with(".exe") {
        return Some((argv0.rsplit(['/', '\\']).next().unwrap_or(&argv0).to_string(), true));
    }
    if exe.is_empty() {
        return None;
    }
    let low = exe.to_lowercase();
    let game = low.contains("/steamapps/") || low.contains("/games/") || low.contains("/heroic/") || low.contains("/lutris/");
    Some((exe.rsplit('/').next().unwrap_or(&exe).to_string(), game))
}

#[cfg(not(windows))]
fn x11_programs() -> Vec<(String, bool)> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};
    let Ok((c, scr)) = x11rb::connect(None) else { return Vec::new() };
    let root = c.setup().roots[scr].root;
    let atom = |n: &[u8]| c.intern_atom(false, n).ok().and_then(|r| r.reply().ok()).map(|r| r.atom);
    let (Some(list), Some(pid_a)) = (atom(b"_NET_CLIENT_LIST"), atom(b"_NET_WM_PID")) else { return Vec::new() };
    let prop = |w: u32, a: u32, t: AtomEnum, len: u32| c.get_property(false, w, a, t, 0, len).ok().and_then(|r| r.reply().ok());
    let wins: Vec<u32> = prop(root, list, AtomEnum::WINDOW, 4096).and_then(|r| r.value32().map(|v| v.collect())).unwrap_or_default();
    let mut out = Vec::new();
    for w in wins {
        let pid = prop(w, pid_a, AtomEnum::CARDINAL, 1).and_then(|r| r.value32().and_then(|mut v| v.next())).unwrap_or(0);
        let class = prop(w, AtomEnum::WM_CLASS.into(), AtomEnum::STRING, 64)
            .map(|r| r.value.split(|&b| b == 0).map(|s| String::from_utf8_lossy(s).into_owned()).collect::<Vec<_>>())
            .unwrap_or_default();
        let steam_app = class.iter().any(|s| s.starts_with("steam_app_"));
        match (pid > 0).then(|| pid_program(pid)).flatten() {
            Some((n, g)) => out.push((n, g || steam_app)),
            None => {
                // no pid: WM_CLASS, else the title (old plain-Xlib programs set neither)
                let title = || prop(w, AtomEnum::WM_NAME.into(), AtomEnum::STRING, 64).map(|r| String::from_utf8_lossy(&r.value).trim().to_string());
                if let Some(c) = class.iter().find(|s| !s.is_empty()).cloned().or_else(title).filter(|s| !s.is_empty()) {
                    out.push((c, steam_app));
                }
            }
        }
    }
    out
}

/// This user's Wine / Proton programs (a game may not have its window yet).
#[cfg(not(windows))]
fn wine_programs() -> Vec<(String, bool)> {
    use std::os::unix::fs::MetadataExt;
    let Ok(me) = std::fs::metadata("/proc/self") else { return Vec::new() };
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/proc") else { return out };
    for e in rd.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        if std::fs::metadata(e.path()).map_or(true, |m| m.uid() != me.uid()) {
            continue;
        }
        if let Some((n, true)) = pid_program(pid).filter(|(n, _)| n.to_ascii_lowercase().ends_with(".exe")) {
            out.push((n, true));
        }
    }
    out
}

#[cfg(windows)]
fn list_programs() -> Vec<(String, bool)> {
    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(cb: extern "system" fn(isize, isize) -> i32, lp: isize) -> i32;
        fn IsWindowVisible(h: isize) -> i32;
        fn GetWindowThreadProcessId(h: isize, pid: *mut u32) -> u32;
        fn GetWindowTextLengthW(h: isize) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn QueryFullProcessImageNameW(h: isize, flags: u32, buf: *mut u16, size: *mut u32) -> i32;
        fn CloseHandle(h: isize) -> i32;
    }
    extern "system" fn each(h: isize, lp: isize) -> i32 {
        let pids = unsafe { &mut *(lp as *mut Vec<u32>) };
        unsafe {
            if IsWindowVisible(h) != 0 && GetWindowTextLengthW(h) > 0 {
                let mut pid = 0u32;
                GetWindowThreadProcessId(h, &mut pid);
                if pid != 0 && !pids.contains(&pid) {
                    pids.push(pid);
                }
            }
        }
        1
    }
    let mut pids: Vec<u32> = Vec::new();
    unsafe { EnumWindows(each, &mut pids as *mut Vec<u32> as isize) };
    let mut out = Vec::new();
    for pid in pids {
        let h = unsafe { OpenProcess(0x1000, 0, pid) }; // PROCESS_QUERY_LIMITED_INFORMATION
        if h == 0 {
            continue;
        }
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let ok = unsafe { QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut n) } != 0;
        unsafe { CloseHandle(h) };
        if !ok {
            continue;
        }
        let path = String::from_utf16_lossy(&buf[..n as usize]);
        let low = path.to_lowercase();
        if low.starts_with(r"c:\windows\") {
            continue;
        }
        let name = path.rsplit('\\').next().unwrap_or(&path).to_string();
        if NOT_GAMES.contains(&games::exe_key(&name).as_str()) {
            continue;
        }
        let game = low.contains(r"\steamapps\") || low.contains("games") || low.contains("riot") || low.contains("epic");
        out.push((name, game));
    }
    out
}

/// Wires the page's callbacks. Call once, after the App is in place.
pub fn init(ui: &AppWindow) {
    ST.with(|s| s.borrow_mut().ui = Some(ui.as_weak()));
    let g = ui.global::<Profs>();
    g.set_games_note(games_note().into());
    g.on_save_new(|name| {
        with_app(|a| {
            let clean = share::clean_name(name.as_str());
            if clean.is_empty() {
                return status("Give the profile a name first", false);
            }
            if a.m.profiles.iter().any(|p| p.0.eq_ignore_ascii_case(&clean)) {
                return status(format!("There is already a profile called \"{clean}\" - pick another name"), false);
            }
            a.m.save_profile(&clean);
            if let Some(ui) = a.ui() {
                ui.global::<Profs>().set_new_name(SharedString::new());
            }
            status(format!("Saved \"{clean}\""), true);
            a.changed();
        })
    });
    g.on_update_active(|| {
        with_app(|a| {
            let n = a.m.set.active_profile.clone();
            if a.m.profiles.iter().any(|p| p.0 == n) {
                a.m.save_profile(&n);
                status(format!("Updated \"{n}\""), true);
                a.changed();
            }
        })
    });
    g.on_use(|i| {
        with_app(|a| {
            a.m.load_profile(i as usize);
            a.changed();
        })
    });
    g.on_rename(|i, text| {
        with_app(|a| match a.m.rename_profile(i as usize, text.as_str()) {
            Ok(n) => {
                if let Some(ui) = a.ui() {
                    ui.global::<Profs>().set_renaming(-1);
                }
                status(format!("Renamed to \"{n}\""), true);
                a.changed();
            }
            Err(e) => status(e, false),
        })
    });
    g.on_move(|i, d| {
        with_app(|a| {
            let open = win().map_or(-1, |u| u.global::<Profs>().get_games_open());
            a.m.move_profile(i as usize, d);
            if open == i {
                if let Some(u) = win() {
                    u.global::<Profs>().set_games_open(i + d);
                }
            }
            a.changed();
        })
    });
    g.on_delete(|i| {
        with_app(|a| {
            let name = a.m.profiles.get(i as usize).map(|p| p.0.clone()).unwrap_or_default();
            a.m.delete_profile(i as usize);
            if let Some(u) = win() {
                u.global::<Profs>().set_games_open(-1);
            }
            status(format!("Deleted \"{name}\""), true);
            a.changed();
        })
    });
    g.on_code_of(|i| {
        let mut out = SharedString::new();
        with_app(|a| {
            let Some((name, p)) = a.m.profiles.get(i as usize).cloned() else { return };
            match code_for(a, &name, &p) {
                Ok(c) => {
                    status(format!("Copied the code of \"{name}\" ({} letters) - paste it anywhere", c.len()), true);
                    out = c.into();
                }
                Err(e) => status(e, false),
            }
        });
        out
    });
    g.on_code_of_current(|| {
        let mut out = SharedString::new();
        with_app(|a| {
            let cur = a.m.cur.clone();
            let name = a.m.profiles.iter().find(|p| p.0 == a.m.set.active_profile && p.1 == cur).map(|p| p.0.clone()).unwrap_or_default();
            match code_for(a, &name, &cur) {
                Ok(c) => {
                    status(format!("Copied the code of the crosshair on screen ({} letters)", c.len()), true);
                    out = c.into();
                }
                Err(e) => status(e, false),
            }
        });
        out
    });
    g.on_add_code(|text, use_now| {
        with_app(|a| match read_code(a, text.as_str()) {
            Ok((name, p, note)) => {
                let name = if name.is_empty() { "Shared".to_string() } else { name };
                let Some(i) = a.m.add_profile(&name, p) else { return status("That's 200 profiles - delete some first", false) };
                let shown = a.m.profiles[i].0.clone();
                if use_now {
                    a.m.load_profile(i);
                }
                if let Some(u) = a.ui() {
                    u.global::<Profs>().set_code_text(SharedString::new());
                }
                let what = if use_now { "Added and using" } else { "Added" };
                status(if note.is_empty() { format!("{what} \"{shown}\"") } else { format!("{what} \"{shown}\". {note}") }, true);
                a.changed();
            }
            Err(e) => status(e, false),
        })
    });
    g.on_add_game(|i, text| {
        with_app(|a| {
            let key = games::exe_key(text.as_str());
            let Some(name) = a.m.profiles.get(i as usize).map(|p| p.0.clone()) else { return };
            if key.is_empty() {
                return status("Type the game's program name first (e.g. cs2.exe)", false);
            }
            let before = games::parse(&a.m.set.games).into_iter().find(|r| r.game == key).map(|r| r.profile);
            a.m.set_game_rule(&key, Some(&name));
            if let Some(u) = a.ui() {
                u.global::<Profs>().set_game_text(SharedString::new());
            }
            match before {
                Some(b) if b != name => status(format!("\"{key}\" now uses \"{name}\" (was \"{b}\")"), true),
                _ => status(format!("\"{key}\" uses \"{name}\""), true),
            }
            a.changed();
        })
    });
    g.on_remove_game(|key| {
        with_app(|a| {
            a.m.set_game_rule(key.as_str(), None);
            status(format!("\"{key}\" no longer switches the crosshair"), true);
            a.changed();
        })
    });
    g.on_games_switch(|on| {
        with_app(|a| {
            a.m.set.games_on = on;
            a.changed();
        })
    });
    g.on_list_progs(list_progs);

    // Test hook (screenshots): CROSSHAIR_PROFILES_DEMO=1 opens the second profile's games and lists the running
    // programs, and puts a CS2 code in the paste field.
    if std::env::var_os("CROSSHAIR_PROFILES_DEMO").is_some() {
        std::env::remove_var("CROSSHAIR_PROFILES_DEMO");
        g.set_games_open(1);
        g.set_code_text("CSvbPubOq37zTGqtsPTP5QTrp5CB4xFXiKRLfzJsm49ZRe".into());
        list_progs();
    }
}

fn list_progs() {
    {
        if let Some(u) = win() {
            u.global::<Profs>().set_progs_busy(true);
        }
        std::thread::spawn(|| {
            let names = running();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(u) = win() else { return };
                let g = u.global::<Profs>();
                g.set_progs_busy(false);
                if names.is_empty() {
                    status("No programs found - type the game's program name instead", false);
                }
                let v: Vec<Prog> = names.iter().map(|n| Prog { name: n.as_str().into(), key: games::exe_key(n).into(), used: SharedString::new() }).collect();
                g.set_prog_rows(v.len().div_ceil(3) as i32);
                g.set_progs(ModelRc::new(VecModel::from(v)));
                with_app(|a| refresh_progs_used(&g, &games::parse(&a.m.set.games)));
            });
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn animated_names_are_free_and_readable() {
        let dir = std::env::temp_dir().join(format!("aimdot-prof-anim-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let a = super::free_anim_name(&dir, "Pulse", 4, 8.0);
        assert_eq!(a, "Pulse (anim 4x8).png");
        std::fs::write(dir.join(&a), b"x").unwrap();
        let b = super::free_anim_name(&dir, "Pulse", 4, 8.0);
        assert_eq!(b, "Pulse (2) (anim 4x8).png");
        assert_eq!(crosshair_core::paint::anim_of(&b), Some((4, 8.0)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn running_programs_are_listed_without_aimdot() {
        // this test process itself is a program of this user: something must come back, and never aimdot
        let v = super::running();
        assert!(v.len() <= 45);
        assert!(v.iter().all(|n| !n.to_lowercase().starts_with("aimdot")));
    }
}
