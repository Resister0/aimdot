//! Per-game profiles, the overlay's part: notice which program is in front and report its names
//! (`Ev::Game(names)` -> `Model::game_focus` in core switches the profile). Runs only while a game rule exists
//! (`Model::games_needed`); with no rules nothing here is started - no thread, no hook, no polling.
//!
//! - **Windows**: `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` - Windows calls us when the window in front changes
//!   (out of context, on the overlay's own message loop). Names: the program file of that window's process.
//! - **Linux**: XWayland's root window property `_NET_ACTIVE_WINDOW` (KWin, GNOME and wlroots WMs set it for X11
//!   windows and clear it when a Wayland window is in front). One thread blocks on its own X connection and wakes
//!   only when the active window changes. Names: `/proc/<pid>/exe`, argv[0] (a Proton / Wine game's own
//!   `Game.exe`), and the window's WM_CLASS (Proton: `steam_app_<id>`). Games in a native Wayland window aren't
//!   seen (no simple standard way for a normal program to ask the compositor) - they count as "no game".

#[cfg(target_os = "linux")]
pub use linux::{set_needed, start, Report};

#[cfg(any(windows, feature = "wincheck"))]
#[cfg_attr(not(windows), allow(dead_code, unused_imports))]
pub use win::set_needed as win_set_needed;

/// Program names for a pid (Linux): exe file name, argv[0] file name (Wine: "C:\...\Game.exe").
#[cfg(target_os = "linux")]
pub fn names_of_pid(pid: u32) -> Vec<String> {
    let dir = std::path::PathBuf::from(format!("/proc/{pid}"));
    let mut v = Vec::new();
    if let Ok(p) = std::fs::read_link(dir.join("exe")) {
        if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
            v.push(n.to_string());
        }
    }
    if let Ok(cmd) = std::fs::read(dir.join("cmdline")) {
        let a0 = String::from_utf8_lossy(cmd.split(|&b| b == 0).next().unwrap_or(&[])).into_owned();
        let base = a0.rsplit(['/', '\\']).next().unwrap_or("").to_string();
        if !base.is_empty() && !v.contains(&base) {
            v.push(base);
        }
    }
    v
}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, EventMask};
    use x11rb::protocol::Event;

    pub type Report = Arc<dyn Fn(Vec<String>) + Send + Sync>;

    static RUNNING: AtomicBool = AtomicBool::new(false);
    static NEEDED: AtomicBool = AtomicBool::new(false);

    /// Starts watching (once; later calls only switch it back on). Returns what works: "x11" (watching XWayland),
    /// "no-x11" (no XWayland / DISPLAY: nothing can be seen).
    pub fn start(report: Report) -> &'static str {
        NEEDED.store(true, Ordering::SeqCst);
        if RUNNING.load(Ordering::SeqCst) {
            return "x11";
        }
        let Ok((conn, screen)) = x11rb::connect(None) else { return "no-x11" };
        RUNNING.store(true, Ordering::SeqCst);
        let _ = std::thread::Builder::new().name("aimdot-games".into()).stack_size(256 << 10).spawn(move || {
            let r = watch(&conn, screen, &report);
            if let Err(e) = r {
                eprintln!("aimdot: game detection stopped ({e})");
            }
            RUNNING.store(false, Ordering::SeqCst);
        });
        "x11"
    }

    /// No rules any more: the thread stops reporting (it stays asleep on the X socket - no cost - and ends at its
    /// next wake-up). `start` switches it on again.
    pub fn set_needed(on: bool) {
        NEEDED.store(on, Ordering::SeqCst);
    }

    fn watch(conn: &impl Connection, screen: usize, report: &Report) -> Result<(), String> {
        let e = |e: &dyn std::fmt::Display| e.to_string();
        let root = conn.setup().roots[screen].root;
        let atom = |name: &[u8]| -> Result<u32, String> { Ok(conn.intern_atom(false, name).map_err(|x| e(&x))?.reply().map_err(|x| e(&x))?.atom) };
        let active = atom(b"_NET_ACTIVE_WINDOW")?;
        let pid_atom = atom(b"_NET_WM_PID")?;
        conn.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE)).map_err(|x| e(&x))?;
        conn.flush().map_err(|x| e(&x))?;
        let mut last: Option<Vec<String>> = None;
        let check = |last: &mut Option<Vec<String>>| -> Result<(), String> {
            let names = names_of_active(conn, root, active, pid_atom);
            if last.as_ref() != Some(&names) {
                *last = Some(names.clone());
                report(names);
            }
            Ok(())
        };
        check(&mut last)?;
        loop {
            let ev = conn.wait_for_event().map_err(|x| e(&x))?;
            if !NEEDED.load(Ordering::SeqCst) {
                return Ok(());
            }
            if let Event::PropertyNotify(p) = ev {
                if p.atom == active && p.window == root {
                    check(&mut last)?;
                }
            }
        }
    }

    /// The names of the X11 window in front; empty when none (a Wayland window, the desktop).
    fn names_of_active(conn: &impl Connection, root: u32, active: u32, pid_atom: u32) -> Vec<String> {
        let win = conn
            .get_property(false, root, active, AtomEnum::WINDOW, 0, 1)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().and_then(|mut v| v.next()))
            .unwrap_or(0);
        if win == 0 {
            return Vec::new();
        }
        let mut names = Vec::new();
        let pid = conn.get_property(false, win, pid_atom, AtomEnum::CARDINAL, 0, 1).ok().and_then(|c| c.reply().ok()).and_then(|r| r.value32().and_then(|mut v| v.next()));
        if let Some(pid) = pid.filter(|&p| p > 0) {
            names.extend(super::names_of_pid(pid));
        }
        // WM_CLASS: "instance\0class\0" (Proton: steam_app_730)
        if let Some(r) = conn.get_property(false, win, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 64).ok().and_then(|c| c.reply().ok()) {
            for part in r.value.split(|&b| b == 0) {
                let s = String::from_utf8_lossy(part).trim().to_string();
                if !s.is_empty() && !names.contains(&s) {
                    names.push(s);
                }
            }
        }
        // old plain-Xlib programs set neither: their title
        if names.is_empty() {
            if let Some(r) = conn.get_property(false, win, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 64).ok().and_then(|c| c.reply().ok()) {
                let t = String::from_utf8_lossy(&r.value).trim().to_string();
                if !t.is_empty() {
                    names.push(t);
                }
            }
        }
        names
    }
}

#[cfg(any(windows, feature = "wincheck"))]
#[cfg_attr(not(windows), allow(dead_code, non_snake_case))]
mod win {
    use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

    type Hook = isize;
    type WinEventProc = extern "system" fn(Hook, u32, isize, i32, i32, u32, u32);

    #[cfg(windows)]
    #[link(name = "user32")]
    extern "system" {
        fn SetWinEventHook(min: u32, max: u32, module: isize, f: WinEventProc, pid: u32, tid: u32, flags: u32) -> Hook;
        fn UnhookWinEvent(h: Hook) -> i32;
        fn GetForegroundWindow() -> isize;
        fn GetWindowThreadProcessId(h: isize, pid: *mut u32) -> u32;
    }
    #[cfg(windows)]
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn QueryFullProcessImageNameW(h: isize, flags: u32, buf: *mut u16, size: *mut u32) -> i32;
        fn CloseHandle(h: isize) -> i32;
    }
    // Type-checking on Linux (feature wincheck): stand-ins with the same shapes.
    #[cfg(not(windows))]
    unsafe fn SetWinEventHook(_: u32, _: u32, _: isize, _: WinEventProc, _: u32, _: u32, _: u32) -> Hook {
        0
    }
    #[cfg(not(windows))]
    unsafe fn UnhookWinEvent(_: Hook) -> i32 {
        0
    }
    #[cfg(not(windows))]
    unsafe fn GetForegroundWindow() -> isize {
        0
    }
    #[cfg(not(windows))]
    unsafe fn GetWindowThreadProcessId(_: isize, _: *mut u32) -> u32 {
        0
    }
    #[cfg(not(windows))]
    unsafe fn OpenProcess(_: u32, _: i32, _: u32) -> isize {
        0
    }
    #[cfg(not(windows))]
    unsafe fn QueryFullProcessImageNameW(_: isize, _: u32, _: *mut u16, _: *mut u32) -> i32 {
        0
    }
    #[cfg(not(windows))]
    unsafe fn CloseHandle(_: isize) -> i32 {
        0
    }

    const EVENT_SYSTEM_FOREGROUND: u32 = 3;
    const WINEVENT_OUTOFCONTEXT: u32 = 0;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    static HOOK: AtomicIsize = AtomicIsize::new(0);
    /// The `fn(Vec<String>)` given to `set_needed` (as usize: statics can't hold fn pointers in an atomic).
    static REPORT: AtomicUsize = AtomicUsize::new(0);
    static LAST_PID: AtomicUsize = AtomicUsize::new(usize::MAX);

    /// Call on the overlay's UI thread (the one with the message loop). `on` = rules exist: hook the foreground
    /// change (and report the window in front right away); off = unhook. `report` runs on that same thread.
    pub fn set_needed(on: bool, report: fn(Vec<String>)) {
        REPORT.store(report as usize, Ordering::SeqCst);
        let h = HOOK.load(Ordering::SeqCst);
        if on && h == 0 {
            let h = unsafe { SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, 0, on_event, 0, 0, WINEVENT_OUTOFCONTEXT) };
            HOOK.store(h, Ordering::SeqCst);
            LAST_PID.store(usize::MAX, Ordering::SeqCst);
            front(unsafe { GetForegroundWindow() });
        } else if !on && h != 0 {
            unsafe { UnhookWinEvent(h) };
            HOOK.store(0, Ordering::SeqCst);
        }
    }

    extern "system" fn on_event(_: Hook, _ev: u32, hwnd: isize, _: i32, _: i32, _: u32, _: u32) {
        front(hwnd);
    }

    fn front(hwnd: isize) {
        let mut pid = 0u32;
        if hwnd != 0 {
            unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        }
        // the same program again (another of its windows): nothing to tell
        if LAST_PID.swap(pid as usize, Ordering::SeqCst) == pid as usize {
            return;
        }
        let names = if pid == 0 { Vec::new() } else { exe_name(pid).into_iter().collect() };
        let f = REPORT.load(Ordering::SeqCst);
        if f != 0 {
            let f: fn(Vec<String>) = unsafe { std::mem::transmute::<usize, fn(Vec<String>)>(f) };
            f(names);
        }
    }

    fn exe_name(pid: u32) -> Option<String> {
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if h == 0 {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let ok = unsafe { QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut n) } != 0;
        unsafe { CloseHandle(h) };
        if !ok {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..n as usize]);
        path.rsplit('\\').next().map(str::to_string)
    }
}
