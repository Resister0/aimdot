//! Linux tray icon: a StatusNotifierItem (KDE Plasma, GNOME with the AppIndicator extension, most panels) with a
//! com.canonical.dbusmenu menu - the same entries and the same ctl.rs events as the Windows tray (win.rs).
//!
//! Hand-written on the light blocking `dbus` crate (no ksni / zbus / tokio). One thread, asleep in poll() on the
//! D-Bus socket + a wake pipe: nothing runs until the panel asks something, the user clicks, or the crosshair is
//! switched on/off elsewhere (hotkey, settings window) and the checkbox has to follow.
//!
//! The panel restarting (plasmashell crash, panel added later) is handled by watching the watcher's name:
//! when org.kde.StatusNotifierWatcher gets a new owner, the item registers again.
//!
//! Updates (release builds, like the Windows tray): this thread also wakes every 10 min to start the 4-hourly
//! `aimdot-settings --check-update` (core/update.rs), and shows "Update to Aimdot X" on top of the menu once one is
//! known (13 Nov 2026 - before, Linux only checked when the overlay started and only the settings window offered it).

use crate::ctl::Ev;
use crate::ipc_server::Send;
use dbus::arg::{PropMap, RefArg, Variant};
use dbus::blocking::Connection;
use dbus::channel::{MatchingReceiver, Sender};
use dbus::message::{MatchRule, MessageType};
use dbus::Message;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const SNI: &str = "org.kde.StatusNotifierItem";
const SNI_PATH: &str = "/StatusNotifierItem";
const MENU: &str = "com.canonical.dbusmenu";
const MENU_PATH: &str = "/MenuBar";
const PROPS: &str = "org.freedesktop.DBus.Properties";
/// Installed icon name (dev install script + core/src/desktop_entry.rs); the pixmaps below cover "not installed".
const ICON_NAME: &str = "link.sonela.Aimdot";

/// Menu item ids. 0 = the root.
const ID_SHOW: i32 = 1;
const ID_OPEN: i32 = 2;
const ID_SEP: i32 = 3;
const ID_QUIT: i32 = 4;
const ID_UPDATE: i32 = 5;
const ID_SEP_UPDATE: i32 = 6;

/// How often the thread wakes to see whether an update check is due (the check itself runs every 4 h; Windows'
/// housekeeping looks every 10 min too).
const UPDATE_LOOK: Duration = Duration::from_secs(600);

/// "Update to Aimdot X" while an update is known (core::update::tray_label), else None.
type UpdLabel = Arc<Mutex<Option<String>>>;

pub struct Tray {
    on: Arc<AtomicBool>,
    wake: UnixStream,
}

impl Tray {
    /// Starts the tray thread. Without a session bus (or a panel) it just stays quiet.
    pub fn start(send: Send, overlay_on: bool) -> Tray {
        let on = Arc::new(AtomicBool::new(overlay_on));
        let (wake_r, wake) = UnixStream::pair().expect("socketpair");
        let _ = wake_r.set_nonblocking(true);
        let _ = wake.set_nonblocking(true);
        let on2 = on.clone();
        let waker = wake.try_clone().ok();
        let _ = std::thread::Builder::new().name("tray".into()).stack_size(256 * 1024).spawn(move || {
            if let Err(e) = run(send, on2, wake_r, waker) {
                eprintln!("aimdot: tray: {e}");
            }
        });
        Tray { on, wake }
    }

    /// The crosshair was switched on/off (any way): the "Show crosshair" checkbox follows. Cheap when unchanged.
    pub fn set_on(&self, on: bool) {
        if self.on.swap(on, Ordering::Relaxed) != on {
            let _ = (&self.wake).write(&[1]);
        }
    }
}

type Var = Variant<Box<dyn RefArg>>;
type Pixmaps = Vec<(i32, i32, Vec<u8>)>;

fn var(v: impl RefArg + 'static) -> Var {
    Variant(Box::new(v))
}

/// The app icon at 24 / 32 / 48 px as SNI pixmaps (ARGB32, network byte order), decoded once at start.
fn pixmaps() -> Pixmaps {
    let pngs: [&[u8]; 3] = [
        include_bytes!("../../../assets/icons/aimdot-24.png"),
        include_bytes!("../../../assets/icons/aimdot-32.png"),
        include_bytes!("../../../assets/icons/aimdot-48.png"),
    ];
    pngs.iter()
        .filter_map(|d| crosshair_core::image::decode_png(d).ok())
        .map(|im| {
            let argb = im.rgba.chunks_exact(4).flat_map(|p| [p[3], p[0], p[1], p[2]]).collect();
            (im.w as i32, im.h as i32, argb)
        })
        .collect()
}

fn sni_props(icons: &Pixmaps) -> PropMap {
    let mut m = PropMap::new();
    m.insert("Category".into(), var("ApplicationStatus".to_string()));
    m.insert("Id".into(), var("aimdot".to_string()));
    m.insert("Title".into(), var(crosshair_core::APP_NAME.to_string()));
    m.insert("Status".into(), var("Active".to_string()));
    m.insert("WindowId".into(), var(0i32));
    m.insert("IconName".into(), var(ICON_NAME.to_string()));
    m.insert("IconPixmap".into(), var(icons.clone()));
    m.insert("IconThemePath".into(), var(String::new()));
    m.insert("OverlayIconName".into(), var(String::new()));
    m.insert("AttentionIconName".into(), var(String::new()));
    let tip: (String, Pixmaps, String, String) = (String::new(), Vec::new(), crosshair_core::APP_NAME.to_string(), String::new());
    m.insert("ToolTip".into(), var(tip));
    m.insert("ItemIsMenu".into(), var(false));
    m.insert("Menu".into(), var(dbus::Path::from(MENU_PATH)));
    m
}

fn menu_props() -> PropMap {
    let mut m = PropMap::new();
    m.insert("Version".into(), var(3u32));
    m.insert("TextDirection".into(), var("ltr".to_string()));
    m.insert("Status".into(), var("normal".to_string()));
    m.insert("IconThemePath".into(), var(Vec::<String>::new()));
    m
}

/// The properties of one menu item.
fn item(id: i32, on: bool, upd: &Option<String>) -> PropMap {
    let mut m = PropMap::new();
    match id {
        0 => {
            m.insert("children-display".into(), var("submenu".to_string()));
        }
        ID_SHOW => {
            m.insert("label".into(), var("Show crosshair".to_string()));
            m.insert("toggle-type".into(), var("checkmark".to_string()));
            m.insert("toggle-state".into(), var(on as i32));
        }
        ID_OPEN => {
            m.insert("label".into(), var("Open Aimdot".to_string()));
        }
        ID_SEP => {
            m.insert("type".into(), var("separator".to_string()));
        }
        ID_QUIT => {
            m.insert("label".into(), var("Quit".to_string()));
        }
        ID_UPDATE => {
            m.insert("label".into(), var(upd.clone().unwrap_or_default()));
            m.insert("visible".into(), var(upd.is_some()));
        }
        ID_SEP_UPDATE => {
            m.insert("type".into(), var("separator".to_string()));
            m.insert("visible".into(), var(upd.is_some()));
        }
        _ => {}
    }
    m
}

type Layout = (i32, PropMap, Vec<Var>);

/// The menu's items, top to bottom ("Update to Aimdot X" + a separator on top while an update is known).
fn ids(upd: &Option<String>) -> Vec<i32> {
    let mut v = if upd.is_some() { vec![ID_UPDATE, ID_SEP_UPDATE] } else { Vec::new() };
    v.extend([ID_SHOW, ID_OPEN, ID_SEP, ID_QUIT]);
    v
}

fn layout(parent: i32, on: bool, upd: &Option<String>) -> Layout {
    let kids = if parent == 0 {
        ids(upd).into_iter().map(|id| var((id, item(id, on, upd), Vec::<Var>::new()))).collect()
    } else {
        Vec::new()
    };
    (parent, item(parent, on, upd), kids)
}

/// Asks the panel to show the item (again). Not waited for: a watcher that calls back into us before
/// answering would otherwise stall this thread.
fn register(conn: &Connection, name: &str) {
    if let Ok(m) = Message::new_method_call(WATCHER, "/StatusNotifierWatcher", WATCHER, "RegisterStatusNotifierItem") {
        let _ = conn.send(m.append1(name));
    }
}

fn clicked(send: &Send, id: i32) {
    match id {
        ID_SHOW => send(Ev::ToggleOverlay),
        ID_OPEN => send(Ev::OpenSettings),
        ID_QUIT => send(Ev::Quit),
        // Opens the settings window, which downloads + checks + installs; the AppImage update quits and restarts
        // this overlay itself (settings/src/update.rs).
        ID_UPDATE => crosshair_core::update::spawn_install(),
        _ => {}
    }
}

const INTROSPECT: &str = r#"<!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd">
<node>
 <interface name="org.kde.StatusNotifierItem">
  <method name="Activate"><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
  <method name="SecondaryActivate"><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
  <method name="ContextMenu"><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
  <method name="Scroll"><arg type="i" direction="in"/><arg type="s" direction="in"/></method>
  <method name="ProvideXdgActivationToken"><arg type="s" direction="in"/></method>
 </interface>
 <interface name="com.canonical.dbusmenu">
  <method name="GetLayout"><arg type="i" direction="in"/><arg type="i" direction="in"/><arg type="as" direction="in"/><arg type="u" direction="out"/><arg type="(ia{sv}av)" direction="out"/></method>
  <method name="Event"><arg type="i" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="in"/><arg type="u" direction="in"/></method>
  <signal name="LayoutUpdated"><arg type="u"/><arg type="i"/></signal>
 </interface>
</node>"#;

/// Answers one method call. None = not ours (an error reply goes back).
fn answer(msg: &Message, send: &Send, on: &AtomicBool, upd: &UpdLabel, rev: u32, icons: &Pixmaps) -> Option<Message> {
    let upd = upd.lock().map(|u| u.clone()).unwrap_or_default();
    let path = msg.path()?.to_string();
    let iface = msg.interface().map(|i| i.to_string()).unwrap_or_default();
    let member = msg.member()?.to_string();
    let is_on = on.load(Ordering::Relaxed);
    let all = || if path == SNI_PATH { Some(sni_props(icons)) } else if path == MENU_PATH { Some(menu_props()) } else { None };
    let r = msg.method_return();
    Some(match (iface.as_str(), member.as_str()) {
        ("org.freedesktop.DBus.Introspectable", "Introspect") => r.append1(INTROSPECT),
        (PROPS, "GetAll") => r.append1(all()?),
        (PROPS, "Get") => {
            let (_, name) = msg.read2::<String, String>().ok()?;
            let v = all()?.remove(&name)?;
            r.append1(v)
        }
        (SNI, "Activate") => {
            send(Ev::OpenSettings);
            r
        }
        (SNI, "ProvideXdgActivationToken") => {
            if let Ok(t) = msg.read1::<String>() {
                if !t.is_empty() {
                    send(Ev::Activation(t));
                }
            }
            r
        }
        (SNI, "SecondaryActivate" | "ContextMenu" | "Scroll") => r,
        (MENU, "GetLayout") => {
            let parent = msg.read1::<i32>().unwrap_or(0);
            r.append2(rev, layout(parent, is_on, &upd))
        }
        (MENU, "GetGroupProperties") => {
            let ids = msg.read1::<Vec<i32>>().unwrap_or_default();
            let ids = if ids.is_empty() { [vec![0], self::ids(&upd)].concat() } else { ids };
            let v: Vec<(i32, PropMap)> = ids.into_iter().map(|id| (id, item(id, is_on, &upd))).collect();
            r.append1(v)
        }
        (MENU, "GetProperty") => {
            let (id, name) = msg.read2::<i32, String>().ok()?;
            r.append1(item(id, is_on, &upd).remove(&name)?)
        }
        (MENU, "Event") => {
            let (id, kind) = msg.read2::<i32, String>().ok()?;
            if kind == "clicked" {
                clicked(send, id);
            }
            r
        }
        (MENU, "EventGroup") => {
            let evs = msg.read1::<Vec<(i32, String, Var, u32)>>().unwrap_or_default();
            for (id, kind, _, _) in evs {
                if kind == "clicked" {
                    clicked(send, id);
                }
            }
            r.append1(Vec::<i32>::new())
        }
        (MENU, "AboutToShow") => r.append1(false),
        (MENU, "AboutToShowGroup") => r.append2(Vec::<i32>::new(), Vec::<i32>::new()),
        _ => return None,
    })
}

/// Sleeps until a fd is readable or `timeout` passed. false = the D-Bus connection (fds[0]) is gone.
fn wait(fds: &[i32], timeout: Duration) -> bool {
    let mut p: Vec<libc::pollfd> = fds.iter().map(|&fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 }).collect();
    let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    unsafe { libc::poll(p.as_mut_ptr(), p.len() as libc::nfds_t, ms) };
    p[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) == 0
}

fn run(send: Send, on: Arc<AtomicBool>, wake_r: UnixStream, waker: Option<UnixStream>) -> Result<(), String> {
    let mut ch = dbus::channel::Channel::get_private(dbus::channel::BusType::Session).map_err(|e| format!("no D-Bus session: {e}"))?;
    ch.set_watch_enabled(true);
    let conn = Connection::from(ch);
    let name = format!("{SNI}-{}-1", std::process::id());
    conn.request_name(name.as_str(), false, true, true).map_err(|e| format!("bus name: {e}"))?;

    let icons = pixmaps();
    let rev = Arc::new(std::sync::atomic::AtomicU32::new(1));
    let upd: UpdLabel = Arc::new(Mutex::new(crosshair_core::update::tray_label()));
    {
        let (send, on, upd, rev) = (send.clone(), on.clone(), upd.clone(), rev.clone());
        conn.start_receive(
            MatchRule::new_method_call(),
            Box::new(move |msg: Message, c: &Connection| {
                if msg.msg_type() == MessageType::MethodCall {
                    let reply = answer(&msg, &send, &on, &upd, rev.load(Ordering::Relaxed), &icons).unwrap_or_else(|| {
                        let e = dbus::strings::ErrorName::from("org.freedesktop.DBus.Error.UnknownMethod");
                        msg.error(&e, &std::ffi::CString::new("not supported").unwrap())
                    });
                    if !msg.get_no_reply() {
                        let _ = c.send(reply);
                    }
                }
                true
            }),
        );
    }
    // The panel (re)started: register again.
    {
        let name = name.clone();
        let rule = MatchRule::new_signal("org.freedesktop.DBus", "NameOwnerChanged").with_sender("org.freedesktop.DBus");
        conn.add_match(rule, move |(who, _, new): (String, String, String), c, _| {
            if who == WATCHER && !new.is_empty() {
                register(c, &name);
            }
            true
        })
        .map_err(|e| e.to_string())?;
    }
    register(&conn, &name);
    eprintln!("aimdot: tray: icon offered to the panel");

    let mut shown = on.load(Ordering::Relaxed);
    let mut looked: Option<Instant> = None;
    loop {
        while conn.process(Duration::ZERO).map_err(|e| e.to_string())? {}
        // Update check due? (release builds, every 4 h; reads one small file). When it has exited, the waker
        // wakes this loop so the menu shows its result right away.
        if looked.map_or(true, |t| t.elapsed() >= UPDATE_LOOK) {
            looked = Some(Instant::now());
            let w = waker.as_ref().and_then(|w| w.try_clone().ok());
            crosshair_core::update::spawn_check_if_due_then(move || {
                if let Some(w) = w {
                    let _ = (&w).write(&[2]);
                }
            });
        }
        let left = UPDATE_LOOK.saturating_sub(looked.map_or(Duration::ZERO, |t| t.elapsed()));
        if !wait(&[conn.channel().watch().fd, wake_r.as_raw_fd()], left.max(Duration::from_secs(1))) {
            return Err("D-Bus connection lost".into());
        }
        let mut b = [0u8; 64];
        while (&wake_r).read(&mut b).map_or(false, |n| n > 0) {}
        // An update became known (or was installed / is gone): the menu gets / loses its first item.
        let label = crosshair_core::update::tray_label();
        let label_changed = upd.lock().map(|mut u| if *u != label { *u = label; true } else { false }).unwrap_or(false);
        if label_changed {
            let r = rev.fetch_add(1, Ordering::Relaxed) + 1;
            if let Ok(s) = Message::new_signal(MENU_PATH, MENU, "LayoutUpdated") {
                let _ = conn.send(s.append2(r, 0i32));
            }
        }
        let now = on.load(Ordering::Relaxed);
        if now != shown {
            shown = now;
            let r = rev.fetch_add(1, Ordering::Relaxed) + 1;
            let u = upd.lock().map(|u| u.clone()).unwrap_or_default();
            let changed: Vec<(i32, PropMap)> = vec![(ID_SHOW, item(ID_SHOW, now, &u))];
            if let Ok(s) = Message::new_signal(MENU_PATH, MENU, "ItemsPropertiesUpdated") {
                let _ = conn.send(s.append2(changed, Vec::<(i32, Vec<String>)>::new()));
            }
            if let Ok(s) = Message::new_signal(MENU_PATH, MENU, "LayoutUpdated") {
                let _ = conn.send(s.append2(r, 0i32));
            }
        }
        conn.process(Duration::ZERO).map_err(|e| format!("D-Bus connection lost: {e}"))?;
    }
}
