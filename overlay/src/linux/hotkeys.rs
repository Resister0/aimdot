//! Linux global hotkeys through the XDG GlobalShortcuts portal (Wayland gives apps no key hooks; KDE Plasma,
//! GNOME 48+, Hyprland have the portal). Same approach as Echo's hotkey_linux.rs (portal mode):
//! CreateSession -> BindShortcuts (our 4 ids + the keys from Aimdot's settings as *preferred* triggers) ->
//! `Activated` signals -> `Ev::Hotkey(1..4)`, handled by ctl.rs exactly like Windows' RegisterHotKey.
//!
//! The desktop owns the keys: KDE asks once, the user can pick other keys (or none) in the dialog or later in
//! System Settings > Shortcuts > Aimdot, and KDE keeps the key from the first confirmation - changing it in Aimdot
//! later doesn't change it there. So the keys KDE really uses (`trigger_description`, from the BindShortcuts
//! answer and `ShortcutsChanged`) go back to the settings window (`Ev::KdeKeys` -> info `hk_kde`).
//!
//! One thread, blocking libdbus (the `dbus` crate - no tokio / zbus). Idle = asleep in poll() on the D-Bus socket
//! + a wake pipe: no timer, nothing runs until KDE sends a signal or the keys change in the settings.

use crate::ctl::Ev;
use crate::ipc_server::Send;
use crosshair_core::config::{Hotkey, MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_WIN};
use dbus::arg::{PropMap, RefArg, Variant};
use dbus::blocking::Connection;
use dbus::message::MatchRule;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
/// The settings window's Wayland app id / .desktop name: KDE lists the shortcuts under that app ("Aimdot").
const APP_ID: &str = "link.sonela.Aimdot";
/// Shortcut ids = Ev::Hotkey(1..8) (ctl.rs): show / hide, next profile, previous profile, open settings, nudge
/// the centre 1 px up / down / left / right. Never rename them: KDE remembers the user's keys by id.
const IDS: [&str; N] = ["toggle", "next", "prev", "settings", "nudge_up", "nudge_down", "nudge_left", "nudge_right"];
const WHAT: [&str; N] = [
    "Show / hide the crosshair",
    "Next profile",
    "Previous profile",
    "Open settings",
    "Move the crosshair 1 px up",
    "Move the crosshair 1 px down",
    "Move the crosshair 1 px left",
    "Move the crosshair 1 px right",
];
const N: usize = 8;

struct Shared {
    keys: [Hotkey; N],
    /// Bind again (keys changed in the settings, or the first start).
    dirty: bool,
}

pub struct Hotkeys {
    shared: Arc<Mutex<Shared>>,
    wake: UnixStream,
}

impl Hotkeys {
    /// Starts the portal thread. It reports `Ev::KdeKeys(Some(keys))` once KDE answered, `Ev::KdeKeys(None)` if
    /// there is no shortcuts portal (then it waits for a key change and tries again - no retry timer).
    pub fn start(send: Send, keys: [Hotkey; N]) -> Hotkeys {
        let shared = Arc::new(Mutex::new(Shared { keys, dirty: true }));
        let (wake_r, wake) = UnixStream::pair().expect("socketpair");
        let _ = wake_r.set_nonblocking(true);
        let _ = wake.set_nonblocking(true);
        let sh = shared.clone();
        let _ = std::thread::Builder::new().name("hotkeys".into()).stack_size(256 * 1024).spawn(move || loop {
            if let Err(e) = run(&send, &sh, &wake_r) {
                eprintln!("aimdot: hotkeys: {e}");
                send(Ev::KdeKeys(None));
                // Try again only when the keys change (a portal started later, ...).
                loop {
                    let _ = wait(&[wake_r.as_raw_fd()]);
                    drain(&wake_r);
                    if sh.lock().unwrap().dirty {
                        break;
                    }
                }
            }
        });
        Hotkeys { shared, wake }
    }

    /// The keys in the settings changed: offer them to KDE again (only when they really differ).
    pub fn set(&self, keys: [Hotkey; N]) {
        let mut s = self.shared.lock().unwrap();
        if s.keys != keys {
            s.keys = keys;
            s.dirty = true;
            let _ = (&self.wake).write(&[1]);
        }
    }
}

/// Sleeps until one of `fds` is readable. False = the first one hung up (the D-Bus connection is gone).
fn wait(fds: &[i32]) -> bool {
    let mut p: Vec<libc::pollfd> = fds.iter().map(|&fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 }).collect();
    unsafe { libc::poll(p.as_mut_ptr(), p.len() as libc::nfds_t, -1) };
    p[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) == 0
}

fn drain(r: &UnixStream) {
    let mut b = [0u8; 64];
    while (&*r).read(&mut b).map_or(false, |n| n > 0) {}
}

/// Windows virtual-key code (Aimdot's setting) -> xkb key name for the portal's preferred trigger.
fn xkb_key(vk: u32) -> Option<String> {
    Some(match vk {
        0x41..=0x5A => ((vk as u8) as char).to_ascii_lowercase().to_string(),
        0x30..=0x39 => ((vk as u8) as char).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        0x60..=0x69 => format!("KP_{}", vk - 0x60),
        _ => match vk {
            0x08 => "BackSpace",
            0x09 => "Tab",
            0x0D => "Return",
            0x13 => "Pause",
            0x1B => "Escape",
            0x20 => "space",
            0x21 => "Prior",
            0x22 => "Next",
            0x23 => "End",
            0x24 => "Home",
            0x25 => "Left",
            0x26 => "Up",
            0x27 => "Right",
            0x28 => "Down",
            0x2C => "Print",
            0x2D => "Insert",
            0x2E => "Delete",
            0x6A => "KP_Multiply",
            0x6B => "KP_Add",
            0x6D => "KP_Subtract",
            0x6E => "KP_Decimal",
            0x6F => "KP_Divide",
            0x91 => "Scroll_Lock",
            0xBA => "semicolon",
            0xBB => "equal",
            0xBC => "comma",
            0xBD => "minus",
            0xBE => "period",
            0xBF => "slash",
            0xC0 => "grave",
            0xDB => "bracketleft",
            0xDC => "backslash",
            0xDD => "bracketright",
            0xDE => "apostrophe",
            _ => return None,
        }
        .to_string(),
    })
}

/// Aimdot's key setting -> preferred trigger ("CTRL+ALT+x", the shortcuts spec's format). None = no key.
pub fn trigger(h: Hotkey) -> Option<String> {
    if h.vk == 0 {
        return None;
    }
    let key = xkb_key(h.vk)?;
    let mut s = String::new();
    for (m, n) in [(MOD_CTRL, "CTRL+"), (MOD_ALT, "ALT+"), (MOD_SHIFT, "SHIFT+"), (MOD_WIN, "LOGO+")] {
        if h.mods & m != 0 {
            s.push_str(n);
        }
    }
    Some(s + &key)
}

/// a(sa{sv}) from the BindShortcuts answer / ShortcutsChanged -> the trigger text per id ("" = no key in KDE).
fn triggers_of(v: &dyn RefArg, into: &mut [String; N]) -> bool {
    let mut any = false;
    let Some(items) = v.as_iter() else { return false };
    for item in items {
        let Some(mut f) = item.as_iter() else { continue };
        let (Some(id), Some(dict)) = (f.next().and_then(|x| x.as_str().map(String::from)), f.next()) else { continue };
        let Some(n) = IDS.iter().position(|&i| i == id) else { continue };
        let mut trig = String::new();
        if let Some(mut d) = dict.as_iter() {
            while let (Some(k), Some(val)) = (d.next(), d.next()) {
                if k.as_str() == Some("trigger_description") {
                    trig = val
                        .as_str()
                        .map(String::from)
                        .or_else(|| val.as_iter().and_then(|mut i| i.next().and_then(|x| x.as_str().map(String::from))))
                        .unwrap_or_default();
                }
            }
        }
        into[n] = trig;
        any = true;
    }
    any
}

fn var(v: impl RefArg + 'static) -> Variant<Box<dyn RefArg>> {
    Variant(Box::new(v))
}

fn run(send: &Send, shared: &Arc<Mutex<Shared>>, wake_r: &UnixStream) -> Result<(), String> {
    let mut ch = dbus::channel::Channel::get_private(dbus::channel::BusType::Session).map_err(|e| format!("no D-Bus session: {e}"))?;
    // Needed for watch() below (our own poll() on the D-Bus socket instead of a timeout loop).
    ch.set_watch_enabled(true);
    let conn = Connection::from(ch);
    let me = conn.unique_name().trim_start_matches(':').replace('.', "_");
    let proxy = conn.with_proxy(PORTAL, PORTAL_PATH, Duration::from_secs(30));
    // A host (non-Flatpak) app tells the portal who it is before anything else, so KDE shows "Aimdot" with its
    // icon. Older portals don't have it - fine, KDE then guesses from the process.
    let reg: Result<(), _> = proxy.method_call("org.freedesktop.host.portal.Registry", "Register", (APP_ID, PropMap::new()));
    if let Err(e) = reg {
        eprintln!("aimdot: hotkeys: portal app id not registered ({e})");
    }
    let session: Arc<Mutex<String>> = Arc::default();
    let keys: Arc<Mutex<[String; N]>> = Arc::default();

    // Key pressed: Activated(session, id, timestamp, options) - only for our current session.
    {
        let (send, session) = (send.clone(), session.clone());
        conn.add_match(MatchRule::new_signal(IFACE, "Activated"), move |_: (), _, msg| {
            let (s, id, _, opt) = msg.get4::<dbus::Path, String, u64, PropMap>();
            if s.as_ref().map(|p| p.to_string()) == Some(session.lock().unwrap().clone()) {
                if let Some(n) = id.and_then(|id| IDS.iter().position(|&i| i == id)) {
                    // Open settings: KWin brings the new window to the front only with the portal's token.
                    if let Some(t) = opt.as_ref().and_then(|o| o.get("activation_token")).and_then(|v| v.0.as_str()) {
                        if !t.is_empty() {
                            send(Ev::Activation(t.to_string()));
                        }
                    }
                    send(Ev::Hotkey(n as u32 + 1));
                }
            }
            true
        })
        .map_err(|e| e.to_string())?;
    }
    // The answer to BindShortcuts (the user confirmed / changed / cancelled KDE's dialog): the keys KDE uses.
    {
        let (send, keys, me) = (send.clone(), keys.clone(), me.clone());
        conn.add_match(MatchRule::new_signal("org.freedesktop.portal.Request", "Response"), move |_: (), _, msg| {
            let ours = msg.path().is_some_and(|p| p.contains(&format!("/request/{me}/aimdot")) && p.ends_with('b'));
            if ours {
                if let (Some(code), Some(res)) = msg.get2::<u32, PropMap>() {
                    let mut k = keys.lock().unwrap();
                    if code == 0 {
                        if let Some(v) = res.get("shortcuts") {
                            triggers_of(&v.0, &mut k);
                        }
                    }
                    send(Ev::KdeKeys(Some(k.to_vec())));
                }
            }
            true
        })
        .map_err(|e| e.to_string())?;
    }
    // Keys changed in KDE's System Settings while Aimdot runs.
    {
        let (send, keys, session) = (send.clone(), keys.clone(), session.clone());
        conn.add_match(MatchRule::new_signal(IFACE, "ShortcutsChanged"), move |_: (), _, msg| {
            let mut it = msg.iter_init();
            let s: Option<dbus::Path> = it.read().ok();
            if s.map(|p| p.to_string()) == Some(session.lock().unwrap().clone()) {
                if let Some(v) = it.get_refarg() {
                    let mut k = keys.lock().unwrap();
                    if triggers_of(&*v, &mut k) {
                        send(Ev::KdeKeys(Some(k.to_vec())));
                    }
                }
            }
            true
        })
        .map_err(|e| e.to_string())?;
    }

    let mut n = 0u32;
    loop {
        let bind = {
            let mut s = shared.lock().unwrap();
            std::mem::take(&mut s.dirty).then_some(s.keys)
        };
        if let Some(want) = bind {
            // A session's shortcuts can be bound only once: close the old one, make a new one.
            let old = std::mem::take(&mut *session.lock().unwrap());
            if !old.is_empty() {
                let sp = conn.with_proxy(PORTAL, old, Duration::from_secs(5));
                let _: Result<(), _> = sp.method_call("org.freedesktop.portal.Session", "Close", ());
            }
            n += 1;
            let tok = format!("aimdot{}_{n}", std::process::id());
            let mut opt = PropMap::new();
            opt.insert("handle_token".into(), var(format!("{tok}r")));
            opt.insert("session_handle_token".into(), var(tok.clone()));
            let _: (dbus::Path,) = proxy.method_call(IFACE, "CreateSession", (opt,)).map_err(|e| format!("no shortcuts portal ({e})"))?;
            let sp = format!("{PORTAL_PATH}/session/{me}/{tok}");
            *session.lock().unwrap() = sp.clone();
            let list: Vec<(String, PropMap)> = (0..N)
                .map(|i| {
                    let mut m = PropMap::new();
                    m.insert("description".into(), var(format!("Aimdot: {}", WHAT[i])));
                    if let Some(t) = trigger(want[i]) {
                        m.insert("preferred_trigger".into(), var(t));
                    }
                    (IDS[i].to_string(), m)
                })
                .collect();
            let mut opt = PropMap::new();
            opt.insert("handle_token".into(), var(format!("{tok}b")));
            let _: (dbus::Path,) = proxy
                .method_call(IFACE, "BindShortcuts", (dbus::Path::from(sp), list, "", opt))
                .map_err(|e| format!("BindShortcuts: {e}"))?;
            eprintln!("aimdot: hotkeys: offered to the desktop's shortcuts portal");
        }
        // Everything already read (also signals that came in during the calls above), then sleep.
        while conn.process(Duration::ZERO).map_err(|e| e.to_string())? {}
        if !wait(&[conn.channel().watch().fd, wake_r.as_raw_fd()]) {
            return Err("D-Bus connection lost".into());
        }
        drain(wake_r);
        conn.process(Duration::ZERO).map_err(|e| format!("D-Bus connection lost: {e}"))?;
    }
}
