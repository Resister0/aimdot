//! Linux overlay main loop: sleeps in poll() on the display connection + a wake pipe. Nothing runs while
//! nothing changes (no timers), except 30 fps while an RGB rainbow crosshair is showing.

mod evdev;
mod hotkeys;
mod keys;
mod tray;
mod wayland;
mod x11;

use crate::ctl::{Ctl, Ev, Out};
use crate::ipc_server::{self, Send, Start};
use crosshair_core::config::{Hotkey, Profile};
use crosshair_core::gfx::Canvas;
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::sync::{mpsc, Arc};
use std::time::Duration;

/// `monitor` value for "follow the game": the compositor picks the screen (Wayland), X11 uses the first one.
pub const FOLLOW: u32 = u32::MAX;

/// What the main loop needs from a display back end (Wayland layer-shell or X11).
trait Display {
    /// Sends queued requests; reads + handles events when `readable` (or when some are already queued).
    fn pump(&mut self, wait: &mut dyn FnMut(Option<BorrowedFd<'_>>) -> bool);
    /// Monitors changed / surface ready since the last call.
    fn take_changed(&mut self) -> bool;
    fn monitors(&self) -> Vec<String>;
    /// Height in physical pixels of monitor `monitor` (the "Monitor" setting's index).
    fn monitor_height(&self, monitor: u32) -> Option<i32>;
    fn present(&mut self, pic: &Canvas, visible: bool, monitor: u32, off: (i32, i32), new_pic: bool);
    fn name(&self) -> &'static str;
    /// Show / hide "Instant": keep the window on screen and only swap a transparent / crosshair buffer, because
    /// KWin's fadingpopups effect fades every new or closed overlay window (no way for an app to opt out).
    /// Off ("Fade"): hidden = no window at all (KWin fades it in / out; nothing over the game while hidden).
    fn set_keep_mapped(&mut self, on: bool);
    /// Grabs the hotkeys itself (X11, only used when there is no shortcuts portal). None = can't (Wayland).
    fn grab_keys(&mut self, _keys: &[Hotkey]) -> Option<Vec<bool>> {
        None
    }
    /// Hotkeys (1..8) pressed since the last call (X11 grabs).
    fn take_hotkeys(&mut self) -> Vec<u32> {
        Vec::new()
    }
}

impl Display for wayland::Wayland {
    fn pump(&mut self, wait: &mut dyn FnMut(Option<BorrowedFd<'_>>) -> bool) {
        let lost = |e: &dyn std::fmt::Display| -> ! {
            // The compositor went away (logout, crash): nothing to draw on any more.
            eprintln!("aimdot: Wayland connection lost: {e}");
            let _ = std::fs::remove_file(crosshair_core::paths::socket_path());
            std::process::exit(1);
        };
        if let Err(wayland_client::backend::WaylandError::Io(e)) = self.conn.flush() {
            if e.kind() != std::io::ErrorKind::WouldBlock {
                lost(&e);
            }
        }
        match self.queue.prepare_read() {
            Some(guard) => {
                if wait(Some(guard.connection_fd())) {
                    match guard.read() {
                        Err(wayland_client::backend::WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(e) => lost(&e),
                        Ok(_) => {}
                    }
                }
            }
            None => {
                wait(None);
            }
        }
        if let Err(e) = self.queue.dispatch_pending(&mut self.st) {
            lost(&e);
        }
        let _ = self.conn.flush();
    }
    fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.st.changed)
    }
    fn monitors(&self) -> Vec<String> {
        self.st.monitor_names()
    }
    fn monitor_height(&self, monitor: u32) -> Option<i32> {
        self.st.monitor_height(monitor)
    }
    fn set_keep_mapped(&mut self, on: bool) {
        self.st.keep_mapped = on;
    }
    fn present(&mut self, pic: &Canvas, visible: bool, monitor: u32, off: (i32, i32), new_pic: bool) {
        let qh = self.queue.handle();
        self.st.present(&qh, pic, visible, monitor, off, new_pic);
        let _ = self.conn.flush();
    }
    fn name(&self) -> &'static str {
        "wayland-layer-shell"
    }
}

fn open_display() -> Result<Box<dyn Display>, String> {
    let force_x11 = std::env::var("CROSSHAIR_X11").map_or(false, |v| v == "1");
    let wl_err = if force_x11 {
        "CROSSHAIR_X11=1".to_string()
    } else {
        match wayland::Wayland::connect() {
            Ok(w) => return Ok(Box::new(w)),
            Err(e) => e,
        }
    };
    match x11::X11::connect() {
        Ok(x) => {
            eprintln!("aimdot: using the X11 window ({wl_err})");
            Ok(Box::new(x))
        }
        Err(x_err) => Err(format!("{wl_err}; X11: {x_err}")),
    }
}

/// Starts watching the right button (evdev), or pretends to under the `CROSSHAIR_FAKE_AIM` test hook.
/// Per-game profiles: the detection thread reports the program in front as Ev::Game.
fn game_report(send: &Send) -> crate::games::Report {
    let s = send.clone();
    Arc::new(move |names| s(Ev::Game(names)))
}

fn aim_start(ctl: &Ctl, send: &Send) -> &'static str {
    if ctl.fake_aim {
        "ok"
    } else {
        match evdev::aim_code(ctl.m.set.aim_button, ctl.m.set.aim_vk) {
            Some(code) => evdev::start(send.clone(), code),
            None => "none",
        }
    }
}

/// Where the hotkeys come from (chosen once at start).
enum Hk {
    Evdev(keys::Keys),
    Portal(hotkeys::Hotkeys),
    /// None (test hook), or X11 grabs done by the main loop.
    Off,
}
impl Hk {
    fn set(&self, k: [Hotkey; 8]) {
        match self {
            Hk::Evdev(e) => e.set(k),
            Hk::Portal(p) => p.set(k),
            Hk::Off => {}
        }
    }
}

pub fn run(tray: bool, autostart: bool) {
    let (tx, rx) = mpsc::channel::<Ev>();
    let (mut wake_r, wake_w) = UnixStream::pair().expect("socketpair");
    let _ = wake_r.set_nonblocking(true);
    let _ = wake_w.set_nonblocking(true);
    let send: Send = Arc::new(move |ev| {
        let _ = tx.send(ev);
        let _ = (&wake_w).write(&[1]);
    });
    match ipc_server::start(send.clone()) {
        Ok(Start::Running) => {
            // Second start (app menu): show the settings window of the running overlay.
            if !tray {
                ipc_server::tell_running("open");
            }
            return;
        }
        Ok(Start::Listening(_)) => {}
        Err(e) => eprintln!("aimdot: no settings connection ({e})"),
    }
    // A downloaded Aimdot that runs alone (autostart --tray) gets its app menu entry + icons (core/desktop_entry.rs;
    // skips cargo builds, Flatpak and test copies).
    crosshair_core::desktop_entry::ensure();
    let mut disp = match open_display() {
        Ok(d) => d,
        Err(e) => {
            crosshair_core::log::line("error", &format!("can't show an overlay here: {e}"));
            eprintln!("aimdot: can't show an overlay here: {e}");
            let _ = std::fs::remove_file(crosshair_core::paths::socket_path());
            std::process::exit(1);
        }
    };
    let mut ctl = Ctl::new(None);
    ctl.platform = disp.name().into();
    crosshair_core::log::start("overlay", &format!("{} - overlay window: {}", crosshair_core::log::os_guess(), disp.name()));
    // Auto-update check (release builds only, every 4 h at most; a short-lived `aimdot-settings --check-update`).
    crosshair_core::update::spawn_check_if_due();
    if ctl.aim_needed() {
        ctl.aim_status = aim_start(&ctl, &send).into();
    }
    if ctl.m.games_needed() {
        crate::games::start(game_report(&send));
    }
    let keys = |c: &Ctl| crate::ctl::hotkeys(&c.m.set);
    // Hotkeys: read the keyboards directly (evdev) when one is readable; the desktop's shortcuts portal (KDE asks
    // the user) only when none is (Flatpak, no ACL). Never both: in evdev mode the portal is not called at all.
    // Test hook CROSSHAIR_HOTKEYS=evdev|portal|x11|off (keys::mode; test copies default to the portal).
    let mode = keys::mode();
    let ev = if mode == "auto" || mode == "evdev" { keys::Keys::start(send.clone(), keys(&ctl)) } else { None };
    let hk = match ev {
        Some(k) => Hk::Evdev(k),
        None if mode == "off" || mode == "evdev" => {
            crosshair_core::log::line("hotkey", &format!("off (CROSSHAIR_HOTKEYS={mode})"));
            Hk::Off
        }
        None if mode == "x11" => {
            // As if there were no portal: X11 grabs the keys itself (main loop below).
            send(Ev::KdeKeys(None));
            Hk::Off
        }
        None => {
            if mode == "auto" {
                crosshair_core::log::line("hotkey", "no readable keyboard in /dev/input - using the desktop's shortcuts portal");
            }
            Hk::Portal(hotkeys::Hotkeys::start(send.clone(), keys(&ctl)))
        }
    };
    // Tray icon (StatusNotifierItem): always, like the Windows tray - it's the way back to settings / Quit.
    let icon = tray::Tray::start(send.clone(), ctl.m.set.overlay_on);
    // Started with the PC: the window too, unless "Start minimised" (Echo's way, user 11 Nov 2026).
    if ctl.first_run() || !tray || (autostart && !ctl.m.set.start_minimized) {
        ctl.open_settings();
    }

    // X11 without a shortcuts portal: the overlay grabs the keys itself (XGrabKey). Let go while the settings
    // window captures a new key, so the keys reach it.
    let mut x_grab = false;
    let grab = |disp: &mut Box<dyn Display>, ctl: &mut Ctl| {
        let k = if ctl.capture { [Hotkey::default(); 8] } else { keys(ctl) };
        if let Some(ok) = disp.grab_keys(&k) {
            if !ctl.capture {
                ctl.m.hk_ok.copy_from_slice(&ok);
            }
            ctl.send_info();
        }
    };

    let mut pic = Canvas::new(0, 0);
    let mut last: Option<Profile> = None;
    let mut redraw = true;
    let mut quit = false;
    while !quit {
        if disp.take_changed() {
            ctl.monitors = disp.monitors();
            ctl.m.monitors = ctl.monitors.len().max(1) as u32;
            ctl.send_info();
            redraw = true;
        }
        if redraw || ctl.animated() {
            ctl.now();
            let mon = if ctl.m.set.monitor_follow { FOLLOW } else { ctl.m.set.monitor };
            let eff = ctl.sized(disp.monitor_height(mon));
            let new_pic = eff.animated() || last.as_ref() != Some(&eff);
            if new_pic {
                ctl.m.rend.render(&eff, ctl.m.time, &mut pic);
                last = Some(eff);
            }
            let s = &ctl.m.set;
            disp.set_keep_mapped(!s.show_fade);
            disp.present(&pic, ctl.visible(), mon, (s.offset_x as i32, s.offset_y as i32), new_pic);
            redraw = false;
        }
        let mut timeout = ctl.housekeeping();
        if ctl.animated() {
            timeout = Some(timeout.map_or(Duration::from_millis(33), |t| t.min(Duration::from_millis(33))));
        }
        let wake_fd = wake_r.as_fd().as_raw_fd();
        disp.pump(&mut |fd| {
            let ms = timeout.map_or(-1, |t| t.as_millis().min(i32::MAX as u128) as i32);
            let mut fds = [
                libc::pollfd { fd: fd.map_or(-1, |f| f.as_raw_fd()), events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: wake_fd, events: libc::POLLIN, revents: 0 },
            ];
            // Events already queued: don't sleep.
            let ms = if fd.is_none() { 0 } else { ms };
            unsafe { libc::poll(fds.as_mut_ptr(), 2, ms) };
            if fds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 && fds[0].revents & libc::POLLIN == 0 {
                // The display server went away (logout, crash): exit instead of waking up for ever.
                eprintln!("aimdot: display connection closed");
                let _ = std::fs::remove_file(crosshair_core::paths::socket_path());
                std::process::exit(1);
            }
            fds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0
        });
        let mut b = [0u8; 64];
        while wake_r.read(&mut b).map_or(false, |n| n > 0) {}
        let pressed = disp.take_hotkeys().into_iter().map(Ev::Hotkey);
        for ev in pressed.chain(std::iter::from_fn(|| rx.try_recv().ok())) {
            if matches!(ev, Ev::KdeKeys(None)) && disp.name() == "x11" {
                // No portal: grab the keys here instead. `hk_kde` stays off (the overlay owns the keys, like Windows).
                x_grab = true;
                grab(&mut disp, &mut ctl);
                continue;
            }
            let out: Out = ctl.handle(ev);
            redraw |= out.redraw;
            quit |= out.quit;
            if out.hotkeys {
                hk.set(keys(&ctl));
                if x_grab {
                    grab(&mut disp, &mut ctl);
                }
            }
            if out.aim {
                ctl.aim_status = if ctl.aim_needed() { aim_start(&ctl, &send).into() } else { "off".into() };
                ctl.send_info();
                redraw = true; // aim_status decides what `visible()` means
            }
            if out.games {
                if ctl.m.games_needed() {
                    crate::games::start(game_report(&send));
                } else {
                    crate::games::set_needed(false);
                }
            }
        }
        icon.set_on(ctl.m.set.overlay_on);
    }
    ctl.shutdown();
    crosshair_core::log::stop("quit");
    let _ = std::fs::remove_file(crosshair_core::paths::socket_path());
}
