//! Linux global hotkeys read straight from the keyboards in `/dev/input` (evdev), like Echo's hotkey_linux.rs
//! evdev mode. Used whenever a keyboard there is readable (normal desktops: the udev / logind ACL gives the
//! logged-in user read access); only when none is (Flatpak, no ACL) does mod.rs fall back to the desktop's
//! shortcuts portal (hotkeys.rs). In this mode the portal is never called: no KDE dialog, nothing registered.
//!
//! Read-only and never grabbed: the game and every other app still get every key. The kernel filters for us
//! (EVIOCSMASK): only key events for the 8 modifier keys and the keys of Aimdot's own hotkeys reach the thread,
//! so normal typing wakes nothing. Key presses are never logged or stored - only "hotkey: toggle" style lines.
//!
//! One thread, asleep in poll() on all keyboards + one inotify watch on /dev/input (a keyboard plugged in later)
//! + a wake pipe (the keys changed in the settings). No timer.

use crate::ctl::Ev;
use crate::ipc_server::Send;
use crosshair_core::config::{Hotkey, MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_WIN};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const N: usize = 8;
const NAMES: [&str; N] = ["toggle", "next profile", "previous profile", "settings", "nudge up", "nudge down", "nudge left", "nudge right"];
const DIR: &str = "/dev/input";

const EV_KEY: u16 = 1;
/// struct input_event on 64-bit Linux: timeval (16) + type (2) + code (2) + value (4)
const EV_SIZE: usize = 24;
const EV_CNT: usize = 0x20;
const KEY_CNT: usize = 0x300;

/// The 8 modifier keys (linux/input-event-codes.h) and the Aimdot flag each one sets.
const MODS: [(u16, u32); 8] = [
    (29, MOD_CTRL),   // KEY_LEFTCTRL
    (97, MOD_CTRL),   // KEY_RIGHTCTRL
    (56, MOD_ALT),    // KEY_LEFTALT
    (100, MOD_ALT),   // KEY_RIGHTALT (AltGr)
    (42, MOD_SHIFT),  // KEY_LEFTSHIFT
    (54, MOD_SHIFT),  // KEY_RIGHTSHIFT
    (125, MOD_WIN),   // KEY_LEFTMETA
    (126, MOD_WIN),   // KEY_RIGHTMETA
];

/// Windows virtual-key code (Aimdot's setting) -> Linux evdev key code (position on a US layout, like the
/// shortcuts KDE / Windows show for letters on most layouts).
pub fn key_code(vk: u32) -> Option<u16> {
    const LETTERS: [u16; 26] = [30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44];
    Some(match vk {
        0x41..=0x5A => LETTERS[(vk - 0x41) as usize],
        0x30 => 11,
        0x31..=0x39 => (vk - 0x31 + 2) as u16,
        0x70..=0x79 => (vk - 0x70 + 59) as u16, // F1-F10
        0x7A => 87,                             // F11
        0x7B => 88,                             // F12
        0x7C..=0x87 => (vk - 0x7C + 183) as u16, // F13-F24
        0x60..=0x69 => [82u16, 79, 80, 81, 75, 76, 77, 71, 72, 73][(vk - 0x60) as usize],
        0x08 => 14,  // Backspace
        0x09 => 15,  // Tab
        0x0D => 28,  // Enter
        0x13 => 119, // Pause
        0x1B => 1,   // Escape
        0x20 => 57,  // Space
        0x21 => 104, // Page Up
        0x22 => 109, // Page Down
        0x23 => 107, // End
        0x24 => 102, // Home
        0x25 => 105, // Left
        0x26 => 103, // Up
        0x27 => 106, // Right
        0x28 => 108, // Down
        0x2C => 99,  // Print Screen
        0x2D => 110, // Insert
        0x2E => 111, // Delete
        0x6A => 55,  // Numpad *
        0x6B => 78,  // Numpad +
        0x6D => 74,  // Numpad -
        0x6E => 83,  // Numpad .
        0x6F => 98,  // Numpad /
        0x91 => 70,  // Scroll Lock
        0xBA => 39,  // ;
        0xBB => 13,  // =
        0xBC => 51,  // ,
        0xBD => 12,  // -
        0xBE => 52,  // .
        0xBF => 53,  // /
        0xC0 => 41,  // `
        0xDB => 26,  // [
        0xDC => 43,  // backslash
        0xDD => 27,  // ]
        0xDE => 40,  // '
        _ => return None,
    })
}

/// The combo logic, without any device: modifier state per keyboard (a modifier on one keyboard + the key on
/// another still counts, like Echo) -> which hotkey (1..8) a key event fires.
#[derive(Default)]
pub struct Matcher {
    /// keyboard id -> bit i set = MODS[i] held on that keyboard
    held: HashMap<u32, u8>,
}

impl Matcher {
    fn mods(&self) -> u32 {
        let all = self.held.values().fold(0u8, |a, b| a | b);
        MODS.iter().enumerate().filter(|(i, _)| all & (1 << i) != 0).fold(0, |m, (_, (_, f))| m | f)
    }

    /// One EV_KEY event (`val`: 1 down, 0 up, 2 auto-repeat). Returns the hotkey to fire: on the press of the key
    /// with exactly the modifiers set in Aimdot; auto-repeat fires again only for the nudge keys (5..8).
    pub fn event(&mut self, dev: u32, code: u16, val: i32, keys: &[Hotkey; N]) -> Option<u32> {
        if let Some(i) = MODS.iter().position(|&(c, _)| c == code) {
            let h = self.held.entry(dev).or_default();
            match val {
                1 => *h |= 1 << i,
                0 => *h &= !(1 << i),
                _ => {}
            }
            // A modifier can be a hotkey's own key only without modifiers (rare); not supported, like Windows.
            return None;
        }
        if val == 0 {
            return None;
        }
        let mods = self.mods();
        let n = keys.iter().position(|h| h.vk != 0 && key_code(h.vk) == Some(code) && h.mods == mods)?;
        (val == 1 || n >= 4).then_some(n as u32 + 1)
    }

    /// Modifiers held when a keyboard was opened (EVIOCGKEY), or a keyboard gone (`bits` = 0).
    pub fn set_held(&mut self, dev: u32, bits: u8) {
        if bits == 0 {
            self.held.remove(&dev);
        } else {
            self.held.insert(dev, bits);
        }
    }
}

/// The key codes the kernel lets through: the modifiers + the keys of the hotkeys.
fn wanted(keys: &[Hotkey; N]) -> Vec<u16> {
    let mut v: Vec<u16> = MODS.iter().map(|m| m.0).collect();
    v.extend(keys.iter().filter(|h| h.vk != 0).filter_map(|h| key_code(h.vk)));
    v
}

// ---------------- devices ----------------

const fn ioc(dir: u64, nr: u64, size: usize) -> libc::c_ulong {
    ((dir << 30) | ((size as u64) << 16) | ((b'E' as u64) << 8) | nr) as libc::c_ulong
}
const IOC_READ: u64 = 2;
const IOC_WRITE: u64 = 1;

/// struct input_mask (linux/input.h)
#[repr(C)]
struct InputMask {
    ty: u32,
    codes_size: u32,
    codes_ptr: u64,
}

fn set_bit(bits: &mut [u64], n: usize) {
    bits[n / 64] |= 1 << (n % 64);
}
fn bit(bits: &[u64], n: usize) -> bool {
    bits[n / 64] & (1 << (n % 64)) != 0
}

/// A keyboard = it has EV_KEY and the letter keys (mice, game pads, power buttons don't).
fn is_keyboard(fd: i32) -> bool {
    let mut types = [0u64; EV_CNT.div_ceil(64)];
    let mut keys = [0u64; KEY_CNT.div_ceil(64)];
    unsafe {
        if libc::ioctl(fd, ioc(IOC_READ, 0x20, std::mem::size_of_val(&types)) as _, types.as_mut_ptr()) < 0
            || libc::ioctl(fd, ioc(IOC_READ, 0x20 + EV_KEY as u64, std::mem::size_of_val(&keys)) as _, keys.as_mut_ptr()) < 0
        {
            return false;
        }
    }
    bit(&types, EV_KEY as usize) && [16, 30, 44, 50, 57].iter().all(|&k| bit(&keys, k)) // Q A Z M Space
}

/// Modifiers already held right now (EVIOCGKEY), as Matcher bits.
fn held_now(fd: i32) -> u8 {
    let mut keys = [0u64; KEY_CNT.div_ceil(64)];
    if unsafe { libc::ioctl(fd, ioc(IOC_READ, 0x18, std::mem::size_of_val(&keys)) as _, keys.as_mut_ptr()) } < 0 {
        return 0;
    }
    MODS.iter().enumerate().filter(|(_, m)| bit(&keys, m.0 as usize)).fold(0, |a, (i, _)| a | (1 << i))
}

/// Only EV_KEY events with these codes reach us (EV_SYN can't be filtered; the kernel drops a SYN_REPORT with
/// nothing before it). Old kernel (< 4.4): unfiltered, still works.
fn filter(fd: i32, codes: &[u16]) {
    let mut types = [0u64; EV_CNT.div_ceil(64)];
    set_bit(&mut types, EV_KEY as usize);
    let mut keys = [0u64; KEY_CNT.div_ceil(64)];
    for &c in codes {
        set_bit(&mut keys, c as usize);
    }
    let set = |ty: u32, bits: &[u64]| {
        let m = InputMask { ty, codes_size: std::mem::size_of_val(bits) as u32, codes_ptr: bits.as_ptr() as u64 };
        unsafe { libc::ioctl(fd, ioc(IOC_WRITE, 0x93, std::mem::size_of::<InputMask>()) as _, &m) == 0 }
    };
    // Codes first, then the type mask (until the type mask is set, nothing is filtered).
    let _ = set(EV_KEY as u32, &keys) && set(0, &types);
}

struct Dev {
    id: u32,
    path: PathBuf,
    f: std::fs::File,
}

/// Opens `p` if it is a readable keyboard.
fn open(p: &Path) -> Option<std::fs::File> {
    if !p.file_name()?.to_string_lossy().starts_with("event") {
        return None;
    }
    let f = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC).open(p).ok()?;
    is_keyboard(f.as_raw_fd()).then_some(f)
}

fn inotify() -> Option<OwnedFd> {
    let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let dir = std::ffi::CString::new(DIR).ok()?;
    // IN_ATTRIB: udev / logind add the read ACL a moment after the device node appears.
    let w = unsafe { libc::inotify_add_watch(fd.as_raw_fd(), dir.as_ptr(), libc::IN_CREATE | libc::IN_ATTRIB | libc::IN_DELETE) };
    (w >= 0).then_some(fd)
}

/// Names of the files the inotify events are about (nothing else is in /dev/input's events).
fn inotify_names(fd: i32) -> Vec<String> {
    let mut v = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            return v;
        }
        let mut off = 0usize;
        while off + 16 <= n as usize {
            let len = u32::from_ne_bytes(buf[off + 12..off + 16].try_into().unwrap()) as usize;
            let name = &buf[off + 16..(off + 16 + len).min(n as usize)];
            let name = String::from_utf8_lossy(name.split(|&b| b == 0).next().unwrap_or(&[])).to_string();
            if !name.is_empty() && !v.contains(&name) {
                v.push(name);
            }
            off += 16 + len;
        }
    }
}

/// Where the hotkeys come from: `CROSSHAIR_HOTKEYS=evdev|portal|x11|off` (test hook). Default "auto" = evdev when
/// a keyboard is readable, else the portal. A test copy (`CROSSHAIR_HOME` set) never reads the real keyboards
/// unless it asks for evdev: default "portal" there.
pub fn mode() -> String {
    match std::env::var("CROSSHAIR_HOTKEYS") {
        Ok(m) if ["evdev", "portal", "x11", "off"].contains(&m.as_str()) => m,
        _ if std::env::var_os("CROSSHAIR_HOME").is_some() => "portal".into(),
        _ => "auto".into(),
    }
}

pub struct Keys {
    keys: Arc<Mutex<[Hotkey; N]>>,
    wake: UnixStream,
}

impl Keys {
    /// Opens the keyboards and starts the reader thread. None = no readable keyboard (use the portal instead).
    pub fn start(send: Send, keys: [Hotkey; N]) -> Option<Keys> {
        let mut devs = Vec::new();
        let mut next_id = 0u32;
        let mut paths: Vec<PathBuf> = std::fs::read_dir(DIR).ok()?.flatten().map(|e| e.path()).collect();
        paths.sort();
        for p in paths {
            if let Some(f) = open(&p) {
                devs.push(Dev { id: next_id, path: p, f });
                next_id += 1;
            }
        }
        if devs.is_empty() {
            return None;
        }
        let shared = Arc::new(Mutex::new(keys));
        let (wake_r, wake) = UnixStream::pair().ok()?;
        let _ = wake_r.set_nonblocking(true);
        let _ = wake.set_nonblocking(true);
        let ino = inotify();
        crosshair_core::log::line(
            "hotkey",
            &format!("evdev ({} keyboard{}){}", devs.len(), if devs.len() == 1 { "" } else { "s" }, if ino.is_some() { "" } else { " - no inotify, keyboards plugged in later are missed" }),
        );
        let sh = shared.clone();
        std::thread::Builder::new()
            .name("keys".into())
            .stack_size(128 * 1024)
            .spawn(move || run(send, sh, wake_r, ino, devs, next_id))
            .ok()?;
        Some(Keys { keys: shared, wake })
    }

    /// The keys in the settings changed: filter for the new ones.
    pub fn set(&self, keys: [Hotkey; N]) {
        let mut k = self.keys.lock().unwrap();
        if *k != keys {
            *k = keys;
            let _ = (&self.wake).write(&[1]);
        }
    }
}

fn run(send: Send, shared: Arc<Mutex<[Hotkey; N]>>, wake_r: UnixStream, ino: Option<OwnedFd>, mut devs: Vec<Dev>, mut next_id: u32) {
    let mut m = Matcher::default();
    let mut keys = *shared.lock().unwrap();
    let mut codes = wanted(&keys);
    for d in &devs {
        filter(d.f.as_raw_fd(), &codes);
        m.set_held(d.id, held_now(d.f.as_raw_fd()));
    }
    let mut buf = [0u8; EV_SIZE * 64];
    loop {
        let mut fds = vec![
            libc::pollfd { fd: wake_r.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: ino.as_ref().map_or(-1, |f| f.as_raw_fd()), events: libc::POLLIN, revents: 0 },
        ];
        fds.extend(devs.iter().map(|d| libc::pollfd { fd: d.f.as_raw_fd(), events: libc::POLLIN, revents: 0 }));
        if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) } < 0 {
            continue; // EINTR
        }
        if fds[0].revents != 0 {
            let mut b = [0u8; 64];
            while (&wake_r).read(&mut b).map_or(false, |n| n > 0) {}
            keys = *shared.lock().unwrap();
            codes = wanted(&keys);
            for d in &devs {
                filter(d.f.as_raw_fd(), &codes);
            }
        }
        let mut gone = Vec::new();
        for (i, d) in devs.iter_mut().enumerate() {
            let re = fds[i + 2].revents;
            if re == 0 {
                continue;
            }
            loop {
                match d.f.read(&mut buf) {
                    Ok(0) => {
                        gone.push(i);
                        break;
                    }
                    Ok(n) => {
                        for ev in buf[..n - n % EV_SIZE].chunks_exact(EV_SIZE) {
                            let ty = u16::from_ne_bytes([ev[16], ev[17]]);
                            let code = u16::from_ne_bytes([ev[18], ev[19]]);
                            let val = i32::from_ne_bytes([ev[20], ev[21], ev[22], ev[23]]);
                            if ty != EV_KEY {
                                continue;
                            }
                            if let Some(n) = m.event(d.id, code, val, &keys) {
                                if val == 1 {
                                    crosshair_core::log::line("hotkey", NAMES[n as usize - 1]);
                                }
                                send(Ev::Hotkey(n));
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => {
                        gone.push(i); // ENODEV: unplugged
                        break;
                    }
                }
            }
        }
        for &i in gone.iter().rev() {
            let d = devs.remove(i);
            m.set_held(d.id, 0);
            crosshair_core::log::line("hotkey", "a keyboard was removed");
        }
        if fds[1].revents != 0 {
            if let Some(ino) = &ino {
                for name in inotify_names(ino.as_raw_fd()) {
                    let p = Path::new(DIR).join(&name);
                    if devs.iter().any(|d| d.path == p) {
                        continue;
                    }
                    if let Some(f) = open(&p) {
                        filter(f.as_raw_fd(), &codes);
                        m.set_held(next_id, held_now(f.as_raw_fd()));
                        devs.push(Dev { id: next_id, path: p, f });
                        next_id += 1;
                        crosshair_core::log::line("hotkey", "a keyboard was added");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> [Hotkey; N] {
        // the keys these tests use (fresh installs have none set since 10 Nov 2026)
        use crosshair_core::config::{MOD_ALT as A, MOD_CTRL as C, MOD_SHIFT as S};
        let h = |mods, vk| Hotkey { mods, vk };
        let mut s = crosshair_core::config::Settings::default();
        (s.hk_toggle, s.hk_next, s.hk_prev, s.hk_editor) = (h(C | A, 0x58), h(C | A, 0x27), h(C | A, 0x25), h(C | A, 0x45));
        (s.hk_nudge_up, s.hk_nudge_down, s.hk_nudge_left, s.hk_nudge_right) = (h(C | A | S, 0x26), h(C | A | S, 0x28), h(C | A | S, 0x25), h(C | A | S, 0x27));
        crate::ctl::hotkeys(&s)
    }

    /// Feeds (keyboard, code, value) events, returns the hotkeys fired.
    fn feed(evs: &[(u32, u16, i32)]) -> Vec<u32> {
        let mut m = Matcher::default();
        let k = keys();
        evs.iter().filter_map(|&(d, c, v)| m.event(d, c, v, &k)).collect()
    }

    const CTRL: u16 = 29;
    const ALT: u16 = 56;
    const SHIFT: u16 = 42;
    const X: u16 = 45;
    const RIGHT: u16 = 106;

    #[test]
    fn ctrl_alt_x_toggles_once() {
        // Ctrl+Alt+X held long enough for auto-repeat: fires once.
        let ev = [(0, CTRL, 1), (0, ALT, 1), (0, X, 1), (0, X, 2), (0, X, 2), (0, X, 0), (0, ALT, 0), (0, CTRL, 0)];
        assert_eq!(feed(&ev), vec![1]);
    }

    #[test]
    fn plain_x_or_wrong_mods_do_nothing() {
        assert!(feed(&[(0, X, 1), (0, X, 0)]).is_empty());
        assert!(feed(&[(0, CTRL, 1), (0, X, 1), (0, X, 0), (0, CTRL, 0)]).is_empty());
        // Ctrl+Alt+Shift+X is not Ctrl+Alt+X.
        assert!(feed(&[(0, CTRL, 1), (0, ALT, 1), (0, SHIFT, 1), (0, X, 1)]).is_empty());
        // Released modifier no longer counts.
        assert!(feed(&[(0, CTRL, 1), (0, ALT, 1), (0, ALT, 0), (0, X, 1)]).is_empty());
    }

    #[test]
    fn right_ctrl_and_left_alt_count() {
        assert_eq!(feed(&[(0, 97, 1), (0, ALT, 1), (0, X, 1)]), vec![1]);
        // Left and right Ctrl both held, one released: Ctrl still held.
        assert_eq!(feed(&[(0, CTRL, 1), (0, 97, 1), (0, CTRL, 0), (0, ALT, 1), (0, X, 1)]), vec![1]);
    }

    #[test]
    fn modifier_on_another_keyboard() {
        assert_eq!(feed(&[(0, CTRL, 1), (1, ALT, 1), (1, X, 1)]), vec![1]);
    }

    #[test]
    fn next_vs_nudge_and_nudge_repeats() {
        // Ctrl+Alt+Right = next profile, once even when held.
        assert_eq!(feed(&[(0, CTRL, 1), (0, ALT, 1), (0, RIGHT, 1), (0, RIGHT, 2), (0, RIGHT, 0)]), vec![2]);
        // Ctrl+Alt+Shift+Right = nudge right, repeats while held.
        assert_eq!(feed(&[(0, CTRL, 1), (0, ALT, 1), (0, SHIFT, 1), (0, RIGHT, 1), (0, RIGHT, 2), (0, RIGHT, 2)]), vec![8, 8, 8]);
    }

    #[test]
    fn unplugged_keyboard_drops_its_modifiers() {
        let mut m = Matcher::default();
        let k = keys();
        m.event(1, CTRL, 1, &k);
        m.event(1, ALT, 1, &k);
        m.set_held(1, 0);
        assert_eq!(m.event(0, X, 1, &k), None);
    }

    #[test]
    fn unbound_key_never_fires() {
        let mut k = keys();
        k[0] = Hotkey::default();
        let mut m = Matcher::default();
        assert_eq!(m.event(0, 0, 1, &k), None);
    }

    #[test]
    fn every_portal_key_maps() {
        // All keys the settings window can set have an evdev code (letters, digits, F-keys, arrows, ...).
        for vk in (0x41..=0x5A).chain(0x30..=0x39).chain(0x70..=0x87).chain([0x25, 0x26, 0x27, 0x28, 0x20, 0x2E]) {
            assert!(key_code(vk).is_some(), "vk {vk:#x}");
        }
        assert_eq!(key_code(0x58), Some(45)); // X
        assert_eq!(key_code(0x45), Some(18)); // E
        assert_eq!(key_code(0x7B), Some(88)); // F12
    }

    #[test]
    fn filter_keeps_mods_and_hotkey_keys_only() {
        let w = wanted(&keys());
        assert!(w.contains(&X) && w.contains(&RIGHT) && w.contains(&CTRL));
        assert!(!w.contains(&30)); // A: typing never wakes the thread
    }
}
