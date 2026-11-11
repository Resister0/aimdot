//! Windows overlay, written directly against the Win32 API (no crates): a click-through layered topmost window
//! for the crosshair, a hidden window for the tray icon / hotkeys / Raw Input, autostart, and Efficiency mode.
//! Based on the first version's win.rs; the editor window (and GDI+, file dialog, drag & drop) is gone - the separate
//! Slint settings program replaces it and drives this one over the IPC socket (ipc_server.rs).
//!
//! Idle cost: the thread sleeps in GetMessage. It wakes for a 1 s housekeeping tick (save + stay on top),
//! settings-window messages, hotkeys, 30 fps only while an RGB rainbow crosshair shows, and - only while
//! an aim mode ("only while aiming" / "hide while aiming") is on - mouse input (Raw Input).
#![allow(non_snake_case, clippy::upper_case_acronyms)]

mod ffi;
/// The zoom helper process (`aimdot --zoom-host`): owns the magnifier, so this process never loads it.
pub mod zoom_host;

use crate::ctl::{Ctl, Ev, Out};
use crate::gamebar;
use crate::ipc_server::{self, Start};
use crosshair_core::config::Profile;
use crosshair_core::gfx::Canvas;
use crosshair_core::paths;
use ffi::*;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicPtr, AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// Posted (once per batch) when the IPC threads queued events for the main loop.
const WM_EVENTS: u32 = WM_APP + 1;
const WM_TRAY: u32 = WM_APP + 2;
const TIMER_HOUSE: usize = 1;
const TIMER_ANIM: usize = 2;
/// Show / hide "Fade": ~60 steps a second, only while a fade runs.
const TIMER_FADE: usize = 3;
const FADE_IN: Duration = Duration::from_millis(120);
const FADE_OUT: Duration = Duration::from_millis(150);
/// Smart outline (#36): read what's behind the centre 10 times a second, only while the feature is on + shown.
const TIMER_CONTRAST: usize = 4;
/// Zoom (#15): check the key / aim ~60 times a second, only while zoomed (the helper refreshes the lens itself).
const TIMER_ZOOM: usize = 5;
/// A started zoom helper that hasn't said "ready" after this long is given up (the next press starts another).
const ZOOM_SPAWN_WAIT: Duration = Duration::from_secs(5);
const CLASS_MAIN: &str = "AimdotMain";
const CLASS_OVERLAY: &str = "AimdotOverlay";
const AUTOSTART_NAME: &str = "Aimdot";
/// The autostart value's name before the app was called Aimdot (removed if it is there).
const AUTOSTART_OLD: &str = "Crosshair";
/// Right-button polling while the mouse moves (see "aim modes" below): 250 checks a second, so the
/// crosshair hides at most 4 ms after the press - within one frame at 240 Hz.
const AIM_POLL_MS: i32 = 4;
/// How long a burst of mouse input keeps the polling on before Raw Input is armed again.
const AIM_POLL_FOR: Duration = Duration::from_millis(400);
/// A "released" read this soon after Raw Input reported the press can't be a real click: the polling is blind.
const AIM_GUARD: Duration = Duration::from_millis(12);
/// Then Raw Input for every report for this long; the next press tests the polling again (invisibly).
const AIM_DISTRUST_FOR: Duration = Duration::from_secs(60);

struct State {
    ctl: Ctl,
    rx: mpsc::Receiver<Ev>,
    hwnd: isize,
    overlay: isize,
    icon: isize,
    pic: Canvas,
    last: Option<Profile>,
    mem_dc: isize,
    dib: isize,
    dib_bits: *mut u32,
    dib_size: (i32, i32),
    shown: bool,
    /// Top-left of the overlay window on screen.
    pos: (i32, i32),
    /// Monitor rectangles, primary first, then left-to-right (the order the settings window shows).
    monitors: Vec<RECT>,
    anim: bool,
    /// What was last applied (None = not yet).
    autostart: Option<bool>,
    eco: Option<bool>,
    /// An aim mode is on (Raw Input armed, or polling).
    aim_on: bool,
    /// "Aim with" as applied (aim_button, aim_vk).
    aim_with: (u32, u32),
    /// The keyboard is in Raw Input right now: only while "Aim with" is a key, or a zoom key is set.
    kbd_raw: bool,
    /// The zoom key is physically down (Raw Input repeats the press while it's held: those are ignored).
    zoom_key_down: bool,
    /// Zoom changed by the wheel while zoomed: saved when the zoom ends.
    zoom_dirty: bool,
    /// Mouse Raw Input is registered right now.
    raw_input: bool,
    /// High-resolution waitable timer for the right-button polling (0 = not available: Raw Input per event).
    aim_timer: isize,
    polling: bool,
    poll_until: Instant,
    /// When Raw Input last reported the right button going down (None after it went up).
    raw_down_at: Option<Instant>,
    /// GetAsyncKeyState was caught not seeing the button (game as administrator?): Raw Input only until then.
    distrust_until: Option<Instant>,
    /// Game Bar mode: the widget's files.
    gb: gamebar::GameBar,
    /// The widget was last given a crosshair (so leaving Game Bar mode blanks it once).
    gb_active: bool,
    /// Something changed since the widget was last updated.
    gb_dirty: bool,
    /// "Follow the game's monitor": the monitor of the focused window (index into `monitors`).
    follow_mon: Option<usize>,
    /// WinEvent hook for foreground changes (0 = off: only while following is on).
    fg_hook: isize,
    /// WinEvent hook for the cursor showing / hiding (0 = off: only while "hide in menus" is on).
    cursor_hook: isize,
    /// The overlay window's constant alpha (255 = fully there; less only during a Fade).
    alpha: u8,
    /// Last look whether an update check is due (core/update.rs; every 10 min from the 1 s tick).
    update_look: Instant,
    /// Show / hide "Fade" running: (start, alpha at the start, fading in).
    fade: Option<(Instant, u8, bool)>,
    /// Smart outline: the 10/s timer runs; the 32x32 sample (memory DC + DIB, kept).
    contrast: bool,
    smp_dc: isize,
    smp_bits: *mut u32,
    /// Zoom: the helper process' window (0 = no helper ready), when a helper was started that isn't ready yet,
    /// zoomed right now (the key's vk). The magnifier itself lives in the helper (win/zoom_host.rs).
    zoom_helper: isize,
    zoom_spawned: Option<Instant>,
    zooming: Option<u32>,
}

static STATE: AtomicPtr<State> = AtomicPtr::new(null_mut());
/// Explorer's "TaskbarCreated" broadcast id.
static TASKBAR_MSG: AtomicU32 = AtomicU32::new(0);
/// The hidden main window (the IPC threads post WM_EVENTS to it).
static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
/// A WM_EVENTS is already queued: don't post another one.
static POSTED: AtomicBool = AtomicBool::new(false);

/// The UI thread's state. Callers must not hold the reference across calls that pump messages (the tray
/// menu), since the window procedure re-enters.
unsafe fn state<'a>() -> Option<&'a mut State> {
    STATE.load(Ordering::Relaxed).as_mut()
}

// ---------------------------------------------------------------- small helpers

/// The Aimdot icon in aimdot.exe (RT_GROUP_ICON 1, put there by overlay/build.rs from assets/icons/aimdot.ico).
const ICON_ID: usize = 1;

/// The app icon at `size` px from the exe's resources (Windows picks the best picture of the .ico). 0 = not
/// there (a build without the resource, e.g. windows-gnu).
unsafe fn app_icon(hinst: isize, size: i32) -> isize {
    LoadImageW(hinst, ICON_ID as *const u16, IMAGE_ICON, size, size, 0)
}

unsafe fn make_icon(c: &Canvas) -> isize {
    let mut bits: *mut c_void = null_mut();
    let color = CreateDIBSection(0, &bmi(c.w, c.h), 0, &mut bits, 0, 0);
    if color == 0 || bits.is_null() {
        return 0;
    }
    let dst = std::slice::from_raw_parts_mut(bits as *mut u32, (c.w * c.h) as usize);
    for (d, &s) in dst.iter_mut().zip(&c.px) {
        // Hard alpha edge keeps the icon correct whether Windows treats it as straight or premultiplied.
        let a = s >> 24;
        *d = if a < 128 {
            0
        } else {
            let un = |v: u32| (v * 255 / a).min(255);
            0xFF00_0000 | (un((s >> 16) & 255) << 16) | (un((s >> 8) & 255) << 8) | un(s & 255)
        };
    }
    let mask_bits = vec![0u8; (((c.w + 15) / 16) * 2 * c.h) as usize];
    let mask = CreateBitmap(c.w, c.h, 1, 1, mask_bits.as_ptr() as *const c_void);
    let ii = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
    let icon = CreateIconIndirect(&ii);
    DeleteObject(color);
    DeleteObject(mask);
    icon
}

/// NIM_ADD (0) / NIM_MODIFY (1) / NIM_DELETE (2).
unsafe fn tray(st: &State, msg: u32) {
    let mut n: NOTIFYICONDATAW = std::mem::zeroed();
    n.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    n.hWnd = st.hwnd;
    n.uID = 1;
    n.uFlags = 1 | 2 | 4; // message | icon | tip
    n.uCallbackMessage = WM_TRAY;
    n.hIcon = st.icon;
    for (d, s) in n.szTip.iter_mut().take(127).zip(crosshair_core::APP_NAME.encode_utf16()) {
        *d = s;
    }
    Shell_NotifyIconW(msg, &n);
}

unsafe fn tray_menu(hwnd: isize, overlay_on: bool) -> i32 {
    let menu = CreatePopupMenu();
    // "Update to Aimdot X" (release builds, when an update is known - core/update.rs) on top.
    if let Some(label) = crosshair_core::update::tray_label() {
        AppendMenuW(menu, 0, 4, wide(&label).as_ptr());
        AppendMenuW(menu, 0x800, 0, null()); // separator
    }
    AppendMenuW(menu, 0, 1, wide("Open settings").as_ptr());
    AppendMenuW(menu, 0, 2, wide(if overlay_on { "Hide crosshair" } else { "Show crosshair" }).as_ptr());
    AppendMenuW(menu, 0x800, 0, null()); // separator
    AppendMenuW(menu, 0, 3, wide("Quit").as_ptr());
    let mut p = POINT::default();
    GetCursorPos(&mut p);
    SetForegroundWindow(hwnd);
    // TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY
    let cmd = TrackPopupMenu(menu, 0x100 | 0x2 | 0x80, p.x, p.y, 0, hwnd, null());
    PostMessageW(hwnd, 0, 0, 0);
    DestroyMenu(menu);
    cmd
}

unsafe extern "system" fn monitor_cb(mon: isize, _dc: isize, _r: *mut RECT, data: isize) -> i32 {
    let v = &mut *(data as *mut Vec<(bool, RECT)>);
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(mon, &mut mi) != 0 {
        v.push((mi.dwFlags & 1 != 0, mi.rcMonitor));
    }
    1
}

/// Monitor rectangles (physical pixels: the process is per-monitor DPI aware), primary first, then left-to-right.
unsafe fn enum_monitors() -> Vec<RECT> {
    let mut v: Vec<(bool, RECT)> = Vec::new();
    EnumDisplayMonitors(0, null(), monitor_cb, &mut v as *mut _ as isize);
    v.sort_by_key(|(primary, r)| (!*primary, r.left, r.top));
    let mut out: Vec<RECT> = v.into_iter().map(|m| m.1).collect();
    if out.is_empty() {
        out.push(RECT { left: 0, top: 0, right: 1920, bottom: 1080 });
    }
    out
}

fn monitor_names(mons: &[RECT]) -> Vec<String> {
    mons.iter()
        .enumerate()
        .map(|(i, r)| {
            let main = if i == 0 { ", main" } else { "" };
            format!("Monitor {} ({}x{}{main})", i + 1, r.right - r.left, r.bottom - r.top)
        })
        .collect()
}

unsafe fn set_autostart(on: bool) {
    // A test copy (CROSSHAIR_HOME) never touches the real Run value: with autostart off in its scratch config it
    // would delete the installed Aimdot's entry, with it on point it at the test exe (like Linux's autostart.rs).
    if std::env::var_os("CROSSHAIR_HOME").is_some_and(|v| !v.is_empty()) {
        return;
    }
    let mut key: isize = 0;
    let hkcu = 0x8000_0001u32 as i32 as isize;
    let sub = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    if RegCreateKeyExW(hkcu, sub.as_ptr(), 0, null(), 0, 0x2, null(), &mut key, null_mut()) != 0 {
        return;
    }
    RegDeleteValueW(key, wide(AUTOSTART_OLD).as_ptr());
    let name = wide(AUTOSTART_NAME);
    if on {
        if let Ok(exe) = std::env::current_exe() {
            let val = wide(&format!("\"{}\" --autostart", exe.to_string_lossy()));
            RegSetValueExW(key, name.as_ptr(), 0, 1, val.as_ptr() as *const u8, (val.len() * 2) as u32);
        }
    } else {
        RegDeleteValueW(key, name.as_ptr());
    }
    RegCloseKey(key);
}

/// Efficiency mode: EcoQoS (Windows 11 runs the process on efficient cores at low clocks) + idle priority.
/// Task Manager shows the leaf icon. Off = Windows decides again + normal priority.
unsafe fn set_eco(on: bool) {
    let s = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: if on { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
        StateMask: if on { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
    };
    let p = GetCurrentProcess();
    SetProcessInformation(p, PROCESS_POWER_THROTTLING, &s as *const _ as *const c_void, std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32);
    SetPriorityClass(p, if on { IDLE_PRIORITY_CLASS } else { NORMAL_PRIORITY_CLASS });
}

// ---------------------------------------------------------------- aim modes: only / hide while aiming (Raw Input + polling)
//
// Raw Input can't deliver button changes only: every mouse report arrives as a WM_INPUT, so an 8 kHz gaming
// mouse would wake the overlay 8000 times a second. So Raw Input is only the alarm: the first report of a
// burst turns Raw Input off and starts checking the right button every 4 ms (high-resolution waitable timer,
// 250 wake-ups a second however fast the mouse is). 400 ms later Raw Input is armed again; if the mouse is
// still moving, the next report starts the next 400 ms right away. A mouse at rest costs nothing.
// Without the high-resolution timer (Windows 10 before 1803) it stays on Raw Input for every report.
// Reaction to the press: while Raw Input is armed, at once (the WM_INPUT handler shows / hides the crosshair
// before it switches to polling); while polling, at most 4 ms (one frame at 240 Hz).

fn raw_dev(hwnd: isize, usage: u16, on: bool) -> RAWINPUTDEVICE {
    RAWINPUTDEVICE {
        usUsagePage: 0x01, // generic desktop
        usUsage: usage,    // 2 mouse, 6 keyboard
        dwFlags: if on { RIDEV_INPUTSINK } else { RIDEV_REMOVE },
        hwndTarget: if on { hwnd } else { 0 },
    }
}

/// The keyboard should be in Raw Input: "Aim with" is a key (while mouse Raw Input is `mouse_on`), or a zoom key
/// is set (always - unless the settings window is catching a new hotkey). Keys are never logged or kept.
fn kbd_wanted(st: &State, mouse_on: bool) -> bool {
    (mouse_on && st.aim_on && st.aim_with.0 == 4) || (st.ctl.m.set.hk_zoom.vk != 0 && !st.ctl.capture)
}

/// Mouse Raw Input for the hidden window, also while other apps (the game) have the focus; the keyboard too when
/// kbd_wanted.
unsafe fn set_raw_input(st: &mut State, on: bool) -> bool {
    let kbd = kbd_wanted(st, on);
    let mut devs = vec![raw_dev(st.hwnd, 0x02, on)];
    if kbd || st.kbd_raw {
        devs.push(raw_dev(st.hwnd, 0x06, kbd));
    }
    let ok = RegisterRawInputDevices(devs.as_ptr(), devs.len() as u32, std::mem::size_of::<RAWINPUTDEVICE>() as u32) != 0;
    st.raw_input = on && ok;
    st.kbd_raw = kbd && ok;
    ok
}

/// Puts the keyboard in or out of Raw Input when the zoom key was set / removed (the mouse stays as it is).
/// The zoom key comes this way instead of RegisterHotKey, which would take the key away from every other app
/// (the game too: E as the zoom key couldn't be typed or used anywhere). Raw Input only watches.
unsafe fn apply_kbd(st: &mut State) {
    let want = kbd_wanted(st, st.raw_input);
    if want == st.kbd_raw {
        return;
    }
    let d = [raw_dev(st.hwnd, 0x06, want)];
    if RegisterRawInputDevices(d.as_ptr(), 1, std::mem::size_of::<RAWINPUTDEVICE>() as u32) != 0 {
        st.kbd_raw = want;
    }
    st.zoom_key_down = false;
}

/// What one WM_INPUT packet means for us.
#[derive(Default)]
struct RawEv {
    /// The aim button / key went down (true) / up (false).
    aim: Option<bool>,
    wheel: i16,
    /// The zoom key went down (true, also repeats while held) / up (false).
    zoom: Option<bool>,
    /// A keyboard packet (not mouse).
    kbd: bool,
}

unsafe fn raw_event(st: &State, lp: isize) -> RawEv {
    let mut ri: RAWINPUT_MOUSE = std::mem::zeroed();
    let mut size = std::mem::size_of::<RAWINPUT_MOUSE>() as u32;
    let n = GetRawInputData(lp, RID_INPUT, &mut ri as *mut _ as *mut c_void, &mut size, std::mem::size_of::<RAWINPUTHEADER>() as u32);
    if n == u32::MAX {
        return RawEv::default();
    }
    let (button, vk) = st.aim_with;
    if ri.header.dwType == RIM_TYPEKEYBOARD {
        let k: RAWKEYBOARD = std::ptr::read_unaligned(&ri.mouse as *const RAWMOUSE as *const RAWKEYBOARD);
        let (key, down) = (k.VKey as u32, k.Flags & RI_KEY_BREAK == 0);
        let zk = st.ctl.m.set.hk_zoom.vk;
        return RawEv {
            aim: (st.aim_on && button == 4 && key == vk).then_some(down),
            zoom: (zk != 0 && key == zk).then_some(down),
            kbd: true,
            ..Default::default()
        };
    }
    if ri.header.dwType != RIM_TYPEMOUSE {
        return RawEv::default();
    }
    let f = ri.mouse.usButtonFlags;
    let wheel = if f & RI_MOUSE_WHEEL != 0 { ri.mouse.usButtonData as i16 } else { 0 };
    // (down, up) flags: right, middle, X1 (mouse 4), X2 (mouse 5)
    let (down, up) = match button {
        1 => (0x0010, 0x0020),
        2 => (0x0040, 0x0080),
        3 => (0x0100, 0x0200),
        4 => return RawEv { wheel, ..Default::default() },
        _ => (RI_MOUSE_RIGHT_BUTTON_DOWN, RI_MOUSE_RIGHT_BUTTON_UP),
    };
    let held = if f & down != 0 {
        Some(true)
    } else if f & up != 0 {
        Some(false)
    } else {
        None
    };
    RawEv { aim: held, wheel, ..Default::default() }
}

/// Safety net for a lost button-up (e.g. the secure desktop / UAC prompt took the input): the aim button / key
/// as Windows sees it now. Raw Input reports physical buttons, so with swapped buttons "right" is the left one.
unsafe fn aim_down(st: &State) -> bool {
    let set = &st.ctl.m.set;
    let vk = match (set.aim_button, set.aim_vk) {
        (1, _) => VK_MBUTTON,
        (2, _) => VK_XBUTTON1,
        (3, _) => VK_XBUTTON2,
        (4, vk) => vk as i32,
        _ => {
            if GetSystemMetrics(SM_SWAPBUTTON) != 0 {
                VK_LBUTTON
            } else {
                VK_RBUTTON
            }
        }
    };
    GetAsyncKeyState(vk) < 0
}

/// Compares the aim button with what the controller knows, and tells it when it changed.
unsafe fn aim_check(st: &mut State) {
    let held = aim_down(st);
    if !held && st.ctl.aim_held && st.raw_down_at.map(|t| t.elapsed() < AIM_GUARD).unwrap_or(false) {
        // Raw Input just said "pressed" but GetAsyncKeyState already says "up": Windows doesn't show us this
        // game's button state (e.g. the game runs as administrator). Don't believe the polling: keep the
        // press, go back to Raw Input for every report for a while (costs CPU, but always right).
        st.distrust_until = Some(Instant::now() + AIM_DISTRUST_FOR);
        stop_polling(st, false);
        set_raw_input(st, true);
        return;
    }
    if held != st.ctl.aim_held {
        handle(st, Ev::Aim(held));
    }
}

/// Polling can be used (high-resolution timer there, and it hasn't been caught lying lately).
fn poll_trusted(st: &State) -> bool {
    st.aim_timer != 0 && st.distrust_until.map(|t| Instant::now() >= t).unwrap_or(true)
}

/// Mouse input arrived while Raw Input was armed: switch to polling for a while.
unsafe fn start_polling(st: &mut State) {
    if !poll_trusted(st) || st.polling {
        return;
    }
    let due: i64 = -(AIM_POLL_MS as i64) * 10_000; // relative, in 100 ns units
    if SetWaitableTimer(st.aim_timer, &due, AIM_POLL_MS, null(), null(), 0) == 0 {
        return;
    }
    set_raw_input(st, false);
    st.polling = true;
    st.poll_until = Instant::now() + AIM_POLL_FOR;
}

/// Stops polling; `rearm` = back to Raw Input (and one last look, so a press in between isn't missed).
unsafe fn stop_polling(st: &mut State, rearm: bool) {
    if st.polling {
        CancelWaitableTimer(st.aim_timer);
        st.polling = false;
    }
    if rearm {
        set_raw_input(st, true);
        aim_check(st);
    }
}

/// The 4 ms timer fired.
unsafe fn aim_poll(st: &mut State) {
    if !st.polling {
        return;
    }
    aim_check(st);
    if Instant::now() >= st.poll_until {
        stop_polling(st, true);
    }
}

unsafe fn apply_aim(st: &mut State) {
    let want = st.ctl.aim_needed();
    let with = (st.ctl.m.set.aim_button, st.ctl.m.set.aim_vk);
    if want == st.aim_on && (!want || with == st.aim_with) {
        return;
    }
    if want && st.aim_on {
        // "Aim with" changed: start over with the new button / key (and the keyboard in or out of Raw Input).
        stop_polling(st, false);
        if st.raw_input {
            set_raw_input(st, false);
        }
        st.raw_down_at = None;
        if st.ctl.aim_held {
            handle(st, Ev::Aim(false));
        }
    }
    st.aim_on = want;
    st.aim_with = with;
    if want {
        let ok = set_raw_input(st, true);
        st.ctl.aim_status = if ok { "ok" } else { "none" }.into();
    } else {
        stop_polling(st, false);
        if st.raw_input {
            set_raw_input(st, false);
        }
        st.ctl.aim_status = "off".into();
        st.raw_down_at = None;
        if st.ctl.aim_held {
            st.ctl.handle(Ev::Aim(false));
        }
    }
    st.ctl.send_info();
}

// ---------------------------------------------------------------- drawing

/// Puts the crosshair on screen (or hides it). Only touches the window when something changed.
unsafe fn present(st: &mut State, force_pic: bool) {
    let fade = st.ctl.m.set.show_fade;
    if !st.ctl.visible() {
        if st.shown {
            if fade {
                // Fade out from where it is (also halfway through a fade in), then hide (fade_step).
                if st.fade.map_or(true, |f| f.2) {
                    start_fade(st, false);
                }
            } else {
                stop_fade(st);
                ShowWindow(st.overlay, SW_HIDE);
                st.shown = false;
            }
        }
        return;
    }
    if st.shown && st.fade.is_some_and(|f| !f.2) {
        start_fade(st, true); // shown again while fading out: back in from the current alpha
    }
    st.ctl.now();
    let eff = st.ctl.sized(monitor_height(st));
    let new_pic = force_pic || eff.animated() || st.last.as_ref() != Some(&eff);
    if new_pic {
        st.ctl.m.rend.render(&eff, st.ctl.m.time, &mut st.pic);
        st.last = Some(eff);
    }
    let (w, h) = (st.pic.w, st.pic.h);
    if w <= 0 || h <= 0 {
        return;
    }
    let set = &st.ctl.m.set;
    let mon = st.monitors[mon_index(st)];
    let (cx, cy) = ((mon.left + mon.right) / 2 + set.offset_x as i32, (mon.top + mon.bottom) / 2 + set.offset_y as i32);
    let pos = (cx - w / 2, cy - h / 2);
    let mut copy = new_pic;
    if st.dib_size != (w, h) || st.dib == 0 {
        if st.mem_dc == 0 {
            st.mem_dc = CreateCompatibleDC(0);
        }
        let mut bits: *mut c_void = null_mut();
        let dib = CreateDIBSection(0, &bmi(w, h), 0, &mut bits, 0, 0);
        if dib == 0 || bits.is_null() {
            return;
        }
        SelectObject(st.mem_dc, dib);
        if st.dib != 0 {
            DeleteObject(st.dib);
        }
        st.dib = dib;
        st.dib_bits = bits as *mut u32;
        st.dib_size = (w, h);
        copy = true;
    }
    if !copy && pos == st.pos && st.shown {
        return;
    }
    if copy {
        // Canvas pixels are premultiplied BGRA, exactly what UpdateLayeredWindow wants.
        std::ptr::copy_nonoverlapping(st.pic.px.as_ptr(), st.dib_bits, (w * h) as usize);
    }
    let p = POINT { x: pos.0, y: pos.1 };
    let size = SIZE { cx: w, cy: h };
    let zero = POINT::default();
    if !st.shown {
        if fade {
            st.alpha = 0;
            start_fade(st, true);
        } else {
            stop_fade(st);
        }
    }
    let blend = BLENDFUNCTION { op: 0, flags: 0, alpha: st.alpha, format: 1 }; // AC_SRC_ALPHA
    UpdateLayeredWindow(st.overlay, 0, &p, &size, st.mem_dc, &zero, 0, &blend, ULW_ALPHA);
    st.pos = pos;
    if !st.shown {
        ShowWindow(st.overlay, SW_SHOWNOACTIVATE);
        SetWindowPos(st.overlay, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        st.shown = true;
    }
}

// ---------------------------------------------------------------- show / hide "Fade"
// Windows doesn't fade a layered window by itself; KDE does (KWin's fadingpopups effect). So with "Fade" the
// overlay animates its constant alpha: in 120 ms, out 150 ms. "Instant" (default): shown / hidden at once.

unsafe fn start_fade(st: &mut State, fade_in: bool) {
    st.fade = Some((Instant::now(), st.alpha, fade_in));
    SetTimer(st.hwnd, TIMER_FADE, 15, null());
}

unsafe fn stop_fade(st: &mut State) {
    if st.fade.take().is_some() {
        KillTimer(st.hwnd, TIMER_FADE);
    }
    if st.alpha != 255 {
        st.alpha = 255;
        if st.shown {
            set_alpha(st);
        }
    }
}

/// Only the constant alpha changes (no source DC = the picture stays as it is).
unsafe fn set_alpha(st: &State) {
    let blend = BLENDFUNCTION { op: 0, flags: 0, alpha: st.alpha, format: 1 };
    UpdateLayeredWindow(st.overlay, 0, null(), null(), 0, null(), 0, &blend, ULW_ALPHA);
}

unsafe fn fade_step(st: &mut State) {
    let Some((t0, from, fade_in)) = st.fade else {
        KillTimer(st.hwnd, TIMER_FADE);
        return;
    };
    let (dur, to) = if fade_in { (FADE_IN, 255u8) } else { (FADE_OUT, 0u8) };
    // Scaled by how far it has to go, so a reversal halfway takes half the time.
    let span = (to as f32 - from as f32).abs() / 255.0;
    let k = (t0.elapsed().as_secs_f32() / (dur.as_secs_f32() * span).max(0.001)).min(1.0);
    st.alpha = (from as f32 + (to as f32 - from as f32) * k).round() as u8;
    set_alpha(st);
    if k >= 1.0 {
        st.fade = None;
        KillTimer(st.hwnd, TIMER_FADE);
        if !fade_in {
            ShowWindow(st.overlay, SW_HIDE);
            st.shown = false;
            st.alpha = 255;
        }
    }
}

/// Physical height of the monitor the crosshair is on ("size as % of the screen height").
fn monitor_height(st: &State) -> Option<i32> {
    let m = st.monitors.get(mon_index(st))?;
    Some(m.bottom - m.top)
}

/// The monitor the crosshair is on: the "Monitor" setting, or the focused window's with "follow the game".
fn mon_index(st: &State) -> usize {
    let s = &st.ctl.m.set;
    let i = if s.monitor_follow { st.follow_mon.unwrap_or(s.monitor as usize) } else { s.monitor as usize };
    i.min(st.monitors.len().max(1) - 1)
}

// ---------------------------------------------------------------- follow the game's monitor (WinEvents)
//
// No polling: Windows calls `win_event` (through our own message loop, WINEVENT_OUTOFCONTEXT) when the foreground
// window changes; the 1 s housekeeping tick catches a game window moved to another monitor.

extern "system" fn win_event(_hook: isize, event: u32, _hwnd: isize, obj: i32, _child: i32, _thread: u32, _time: u32) {
    unsafe {
        if let Some(st) = state() {
            if event == EVENT_SYSTEM_FOREGROUND {
                follow_check(st);
            } else if obj == OBJID_CURSOR {
                cursor_check(st);
            }
        }
    }
}

/// "Hide in menus": a game hides the mouse cursor while you play and shows it in menus (and on the desktop).
/// Called on the cursor's show / hide WinEvents (no polling) and from the 1 s tick as a safety net.
unsafe fn cursor_check(st: &mut State) {
    if !st.ctl.m.set.hide_cursor {
        return;
    }
    let mut ci: CURSORINFO = std::mem::zeroed();
    ci.cbSize = std::mem::size_of::<CURSORINFO>() as u32;
    if GetCursorInfo(&mut ci) == 0 {
        return;
    }
    let shown = ci.flags & CURSOR_SHOWING != 0;
    if shown != st.ctl.cursor_shown {
        st.ctl.cursor_shown = shown;
        present(st, false);
        schedule(st);
    }
}

/// The desktop, the taskbar, Alt+Tab: not a game - the crosshair stays where it is.
unsafe fn is_shell(h: isize) -> bool {
    if h == 0 || h == GetShellWindow() {
        return true;
    }
    let mut buf = [0u16; 64];
    let n = GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
    let class = String::from_utf16_lossy(&buf[..n]);
    matches!(class.as_str(), "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "Progman" | "WorkerW" | "MultitaskingViewFrame" | "XamlExplorerHostIslandWindow")
}

unsafe fn follow_check(st: &mut State) {
    if !st.ctl.m.set.monitor_follow {
        return;
    }
    let h = GetForegroundWindow();
    if is_shell(h) {
        return;
    }
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), &mut mi) == 0 {
        return;
    }
    let r = mi.rcMonitor;
    let at = st.monitors.iter().position(|m| (m.left, m.top, m.right, m.bottom) == (r.left, r.top, r.right, r.bottom));
    if at.is_some() && at != st.follow_mon {
        st.follow_mon = at;
        present(st, false);
        st.gb_dirty = true;
    }
}

/// Per-game profiles (games.rs, on this UI thread from its WinEvent hook): the program in front changed.
fn on_game(names: Vec<String>) {
    unsafe {
        if let Some(st) = state() {
            if handle(st, Ev::Game(names)) {
                PostMessageW(st.hwnd, WM_CLOSE, 0, 0);
            }
        }
    }
}

/// Hooks only while a feature needs them.
unsafe fn apply_hooks(st: &mut State) {
    let want = st.ctl.m.set.monitor_follow;
    if want && st.fg_hook == 0 {
        st.fg_hook = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, 0, win_event, 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS);
        follow_check(st);
    } else if !want && st.fg_hook != 0 {
        UnhookWinEvent(st.fg_hook);
        st.fg_hook = 0;
        st.follow_mon = None;
    }
    let want = st.ctl.m.set.hide_cursor;
    if want && st.cursor_hook == 0 {
        // EVENT_OBJECT_SHOW / HIDE: windows too, but only changes (cheap); win_event keeps OBJID_CURSOR.
        st.cursor_hook = SetWinEventHook(EVENT_OBJECT_SHOW, EVENT_OBJECT_HIDE, 0, win_event, 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS);
        cursor_check(st);
    } else if !want && st.cursor_hook != 0 {
        UnhookWinEvent(st.cursor_hook);
        st.cursor_hook = 0;
        st.ctl.cursor_shown = false;
    }
}

/// 30 fps timer only while a rainbow crosshair is showing.
unsafe fn schedule(st: &mut State) {
    let want = st.ctl.animated();
    if want != st.anim {
        st.anim = want;
        if want {
            SetTimer(st.hwnd, TIMER_ANIM, 33, null());
        } else {
            KillTimer(st.hwnd, TIMER_ANIM);
        }
    }
    let want = st.ctl.m.set.smart_outline && st.shown;
    if want != st.contrast {
        st.contrast = want;
        if want {
            SetTimer(st.hwnd, TIMER_CONTRAST, 100, null());
            contrast_step(st);
        } else {
            KillTimer(st.hwnd, TIMER_CONTRAST);
            if st.ctl.bright_bg.take().is_some() {
                present(st, false); // back to the profile's own outline
            }
        }
    }
}

// ---------------------------------------------------------------- smart outline (#36)
// GDI BitBlt of 32x32 screen pixels around the centre - without CAPTUREBLT, so layered windows (the crosshair
// itself) aren't read. 10 times a second, only while the feature is on and the crosshair shows. Exclusive
// fullscreen games aren't readable this way (the normal overlay doesn't show there either). ⚠ Cost on a GPU-bound
// game not measured yet (reading the desktop makes DWM copy a few pixels back from the GPU).

unsafe fn contrast_step(st: &mut State) {
    const N: i32 = 32;
    if st.smp_dc == 0 {
        st.smp_dc = CreateCompatibleDC(0);
        let mut bits: *mut c_void = null_mut();
        let dib = CreateDIBSection(0, &bmi(N, N), 0, &mut bits, 0, 0);
        if st.smp_dc == 0 || dib == 0 || bits.is_null() {
            return;
        }
        SelectObject(st.smp_dc, dib); // kept for the process' lifetime (one 4 KB bitmap)
        st.smp_bits = bits as *mut u32;
    }
    let (cx, cy) = (st.pos.0 + st.dib_size.0 / 2, st.pos.1 + st.dib_size.1 / 2);
    let screen = GetDC(0);
    let ok = BitBlt(st.smp_dc, 0, 0, N, N, screen, cx - N / 2, cy - N / 2, SRCCOPY) != 0;
    ReleaseDC(0, screen);
    if !ok {
        return;
    }
    let px = std::slice::from_raw_parts(st.smp_bits, (N * N) as usize);
    let sum: f32 = px
        .iter()
        .map(|&p| (0.0722 * (p & 255) as f32 + 0.7152 * ((p >> 8) & 255) as f32 + 0.2126 * ((p >> 16) & 255) as f32) / 255.0)
        .sum();
    let lum = sum / (N * N) as f32;
    // Hysteresis: a grey background between 0.42 and 0.58 keeps the current colour (no flicker).
    let now = match st.ctl.bright_bg {
        None => Some(lum >= 0.5),
        Some(false) if lum > 0.58 => Some(true),
        Some(true) if lum < 0.42 => Some(false),
        b => b,
    };
    if now != st.ctl.bright_bg {
        st.ctl.bright_bg = now;
        present(st, false);
    }
}

// ---------------------------------------------------------------- hold to zoom (#15)
// Magnification API: a click-through topmost window with a Magnifier control around the centre, while the key is
// held; the crosshair window stays on top of it and isn't magnified (filter list). Works over windowed /
// borderless games (what DWM composes), not exclusive fullscreen.
//
// The magnifier runs in a helper process (`aimdot --zoom-host`, win/zoom_host.rs): it pulls in Direct3D 9 + the
// graphics driver (AMD: ~90 MB private, 11 Nov 2026), which Windows keeps loaded for the life of a process. This
// process keeps the logic (key, hold / toggle, only while aiming, wheel) and stays at ~3 MB; the helper is started
// on a zoom press, shows the lens as soon as it's ready, and quits by itself ~3 min after the last zoom.

/// Starts the zoom helper unless one is running or starting. It posts ZM_READY when the magnifier is set up.
unsafe fn zoom_spawn(st: &mut State) {
    if st.zoom_helper != 0 || st.zoom_spawned.is_some_and(|t| t.elapsed() < ZOOM_SPAWN_WAIT) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let mut cmd = std::process::Command::new(exe);
    // Its environment is this one's: a test copy (CROSSHAIR_HOME) gets its own helper, tied to its own windows.
    cmd.arg("--zoom-host").arg(st.hwnd.to_string()).arg(st.overlay.to_string());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(NORMAL_PRIORITY_CLASS); // not the overlay's idle priority (Efficiency mode): a smooth lens
    }
    match cmd.spawn() {
        Ok(_) => st.zoom_spawned = Some(Instant::now()), // (dropping the Child only closes its handles)
        Err(e) => crosshair_core::log::line("zoom", &format!("can't start the zoom helper: {e}")),
    }
}

/// Tells the helper to show the lens: level, size, shape, ring colour, centred on the crosshair.
unsafe fn zoom_show(st: &mut State) {
    let set = &st.ctl.m.set;
    // Lens: % of the monitor's height (22 % = about 6 cm on a 24" screen), centred on the crosshair.
    let size = ((monitor_height(st).unwrap_or(1080) as f32 * set.zoom_size / 100.0).round() as i32).clamp(64, 4000);
    let c = set.zoom_ring;
    let rgb = (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
    let (cx, cy) = (st.pos.0 + st.dib_size.0 / 2, st.pos.1 + st.dib_size.1 / 2);
    let level = (set.zoom_level * 100.0).round() as usize;
    let show = zoom_host::pack_show(size, set.zoom_round, rgb);
    let ok = PostMessageW(st.zoom_helper, zoom_host::ZM_LEVEL, level, 0) != 0
        && PostMessageW(st.zoom_helper, zoom_host::ZM_SHOW, show, zoom_host::pack_xy(cx, cy)) != 0;
    if !ok {
        // The helper just quit (idle): start a new one, it shows the lens when ready.
        st.zoom_helper = 0;
        zoom_spawn(st);
    }
}

unsafe fn zoom_start(st: &mut State) {
    let hk = st.ctl.m.set.hk_zoom;
    if hk.vk == 0 || st.zooming.is_some() || !st.shown {
        return;
    }
    st.zooming = Some(hk.vk);
    if st.zoom_helper != 0 {
        zoom_show(st);
    } else {
        zoom_spawn(st); // shows on ZM_READY (if the zoom is still on by then)
    }
    if st.ctl.m.set.zoom_scroll {
        // The wheel comes as Raw Input (no input hook: the game still gets the wheel too).
        stop_polling(st, false);
        if !st.raw_input {
            set_raw_input(st, true);
        }
    }
    SetTimer(st.hwnd, TIMER_ZOOM, 16, null());
}

/// The helper is set up (its window in `helper`).
unsafe fn zoom_ready(st: &mut State, helper: isize, flags: isize) {
    st.zoom_helper = helper;
    let ms = st.zoom_spawned.take().map_or("?".into(), |t| t.elapsed().as_millis().to_string());
    let hide = if flags & zoom_host::READY_NO_HIDE != 0 { ", can't hide it from screen capture (Windows older than 10 2004?)" } else { "" };
    crosshair_core::log::line("zoom", &format!("magnifier ready in {ms} ms{hide}"));
    if st.zooming.is_some() {
        zoom_show(st);
    }
}

/// The helper quit (`why` = zoom_host::BYE_*).
unsafe fn zoom_bye(st: &mut State, helper: isize, why: isize) {
    if st.zoom_helper != 0 && st.zoom_helper != helper {
        return; // an older helper (a newer one is already ready)
    }
    st.zoom_helper = 0;
    st.zoom_spawned = None;
    let what = match why {
        zoom_host::BYE_IDLE => "magnifier let go (not used for a while)",
        zoom_host::BYE_NO_MAGNIFIER => "can't start the magnifier (MagInitialize failed)",
        _ => "can't make the zoom windows",
    };
    crosshair_core::log::line("zoom", what);
    if why == zoom_host::BYE_IDLE && st.zooming.is_some() {
        zoom_spawn(st); // it quit just as a zoom started (that show was lost): a new one shows it
    }
}

unsafe fn zoom_step(st: &mut State) {
    let Some(vk) = st.zooming else { return };
    let set = &st.ctl.m.set;
    // Hold: ends when the zoom key is let go. Toggle: only the next press ends it. "Only while aiming": letting
    // go of the aim button always ends it.
    let key_up = !set.zoom_toggle && GetAsyncKeyState(vk as i32) >= 0;
    if key_up || (set.zoom_aim_only && !aim_down(st)) || !st.shown {
        zoom_stop(st);
    }
}

unsafe fn zoom_stop(st: &mut State) {
    if st.zooming.take().is_some() {
        KillTimer(st.hwnd, TIMER_ZOOM);
        if st.zoom_helper != 0 {
            PostMessageW(st.zoom_helper, zoom_host::ZM_HIDE, 0, 0); // its idle countdown starts
        }
        if st.raw_input && !st.aim_on {
            set_raw_input(st, false);
        }
        if std::mem::take(&mut st.zoom_dirty) {
            st.ctl.changed_here(); // remember the zoom for next time (+ the settings window shows it)
        }
    }
}

/// The zoom key went down / up (Raw Input: the key still reaches the game and every other app).
unsafe fn zoom_key(st: &mut State, down: bool) {
    if !down {
        st.zoom_key_down = false;
        if st.zooming.is_some() && !st.ctl.m.set.zoom_toggle {
            zoom_stop(st); // Hold: ends on release (zoom_step also checks, in case a release got lost)
        }
        return;
    }
    if std::mem::replace(&mut st.zoom_key_down, true) || st.ctl.capture {
        return; // a repeat while held, or the settings window is catching a new hotkey
    }
    // The zoom key's own modifiers must be held too (extra ones don't matter: Shift to run + E still zooms).
    let mods = st.ctl.m.set.hk_zoom.mods;
    let held = |vk: i32| GetAsyncKeyState(vk) < 0;
    let need = [(MOD_ALT, held(0x12)), (MOD_CONTROL, held(0x11)), (MOD_SHIFT, held(0x10)), (MOD_WIN, held(0x5B) || held(0x5C))];
    if need.iter().any(|&(m, down)| mods & m != 0 && !down) {
        return;
    }
    let set = &st.ctl.m.set;
    if st.zooming.is_some() {
        if set.zoom_toggle {
            zoom_stop(st); // Toggle: the second press ends it
        }
    } else if !set.zoom_aim_only || aim_down(st) {
        // "Only while aiming": nothing unless the aim button is held. Aiming alone never zooms.
        zoom_start(st);
    }
}

/// "Scroll to change zoom": 0.5x per wheel notch, 1.5x - 6x.
unsafe fn zoom_wheel(st: &mut State, delta: i16) {
    let s = &mut st.ctl.m.set;
    let z = (s.zoom_level + 0.5 * (delta as f32 / 120.0)).clamp(1.5, 6.0);
    let z = (z * 2.0).round() / 2.0;
    if z != s.zoom_level {
        s.zoom_level = z;
        st.zoom_dirty = true;
        if st.zoom_helper != 0 {
            PostMessageW(st.zoom_helper, zoom_host::ZM_LEVEL, (z * 100.0).round() as usize, 0);
        }
    }
}

unsafe fn register_hotkeys(st: &mut State) {
    let keys = crate::ctl::hotkeys(&st.ctl.m.set);
    for (i, hk) in keys.iter().enumerate() {
        UnregisterHotKey(st.hwnd, i as i32 + 1);
        st.ctl.m.hk_ok[i] = true;
        // While the settings window captures a new hotkey, ours are let go so the keys reach it.
        if hk.vk != 0 && !st.ctl.capture {
            // The nudge keys (5..8) repeat while held; the others fire once per press.
            let rep = if i >= 4 { 0 } else { MOD_NOREPEAT };
            st.ctl.m.hk_ok[i] = RegisterHotKey(st.hwnd, i as i32 + 1, hk.mods | rep, hk.vk) != 0;
        }
    }
    // Hold to zoom (#15): not a RegisterHotKey (that takes the key from every app) - Raw Input watches it (zoom_key).
    apply_kbd(st);
}

/// Hotkeys, autostart and Efficiency mode (ctl reports a change to any of them as `out.hotkeys`).
unsafe fn apply_system(st: &mut State) {
    register_hotkeys(st);
    let s = &st.ctl.m.set;
    let (autostart, eco) = (s.autostart, s.eco);
    if st.autostart != Some(autostart) {
        st.autostart = Some(autostart);
        set_autostart(autostart);
    }
    if st.eco != Some(eco) {
        st.eco = Some(eco);
        set_eco(eco);
    }
    st.ctl.send_info();
}

/// Feeds one event to the shared controller and does what it says. Returns true when the overlay should quit.
unsafe fn handle(st: &mut State, ev: Ev) -> bool {
    let out: Out = st.ctl.handle(ev);
    if out.hotkeys {
        apply_system(st);
    }
    if out.aim {
        apply_aim(st);
    }
    if out.games {
        // May call on_game right away (it reports the window in front): `st` isn't used across the call.
        let need = st.ctl.m.games_needed();
        crate::games::win_set_needed(need, on_game);
    }
    if out.redraw {
        apply_hooks(st);
        present(st, false);
        st.gb_dirty = true;
    }
    schedule(st);
    out.quit
}

// ---------------------------------------------------------------- Game Bar mode

/// Hands the crosshair to the Game Bar widget in Game Bar mode (display_mode 1); blanks the widget once when
/// the mode is left. Called after a batch of changes (not per settings message: a slider drag sends ~60 a
/// second) and from the 1 s tick, which also picks up a widget installed meanwhile. Cheap when nothing changed.
/// Hide/show while aiming isn't passed on: a file + Game Bar's reaction are too slow for that.
fn gamebar_sync(st: &mut State) {
    st.gb_dirty = false;
    let eff = st.ctl.sized(monitor_height(st));
    let m = &mut st.ctl.m;
    if m.set.display_mode == 1 {
        let _ = st.gb.update(&mut m.rend, &eff, &m.set, m.set.overlay_on);
        st.gb_active = true;
    } else if st.gb_active {
        let _ = st.gb.update(&mut m.rend, &m.cur, &m.set, false);
        st.gb_active = false;
    }
}

fn gamebar_flush(st: &mut State) {
    if st.gb_dirty {
        gamebar_sync(st);
    }
}

unsafe fn drain(st: &mut State) -> bool {
    POSTED.store(false, Ordering::Release);
    let mut quit = false;
    while let Ok(ev) = st.rx.try_recv() {
        quit |= handle(st, ev);
    }
    gamebar_flush(st);
    quit
}

unsafe fn housekeeping(st: &mut State) {
    st.ctl.housekeeping();
    follow_check(st); // a game window moved to another monitor (no WinEvent for that)
    // Auto-update check (release builds only; runs `aimdot-settings --check-update` when 4 h have passed).
    if st.update_look.elapsed() >= Duration::from_secs(600) {
        st.update_look = Instant::now();
        crosshair_core::update::spawn_check_if_due();
    }
    cursor_check(st); // safety net for a missed cursor event
    let gb = gamebar::status();
    if st.ctl.gamebar != gb {
        st.ctl.gamebar = gb.into();
        st.ctl.send_info();
    }
    gamebar_sync(st);
    st.gb.log_tick(st.ctl.m.set.display_mode == 1);
    // Lost button-up safety net - only while GetAsyncKeyState is trusted (else it would undo real presses).
    if st.ctl.aim_held && !st.polling && poll_trusted(st) && !aim_down(st) {
        handle(st, Ev::Aim(false));
    }
    if st.shown {
        // Games like to re-assert their own z-order; stay on top.
        SetWindowPos(st.overlay, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

unsafe fn shutdown(st: &mut State) {
    st.ctl.shutdown();
    if st.gb_active {
        // The widget goes blank when the overlay isn't running.
        let m = &mut st.ctl.m;
        let _ = st.gb.update(&mut m.rend, &m.cur, &m.set, false);
        st.gb_active = false;
    }
    tray(st, 2);
    stop_polling(st, false);
    if st.raw_input {
        set_raw_input(st, false);
    }
    if st.zoom_helper != 0 {
        PostMessageW(st.zoom_helper, WM_CLOSE, 0, 0); // (it also quits by itself when this process ends)
        st.zoom_helper = 0;
    }
    #[cfg(windows)]
    let _ = std::fs::remove_file(paths::port_file());
    crosshair_core::log::stop("quit");
}

/// "10.0.26100" for the log (ntdll's RtlGetVersion tells the truth; GetVersionEx lies without a manifest).
fn windows_version() -> String {
    unsafe {
        let mut v: OSVERSIONINFOW = std::mem::zeroed();
        v.size = std::mem::size_of::<OSVERSIONINFOW>() as u32;
        if RtlGetVersion(&mut v) == 0 {
            format!("{}.{}.{}", v.major, v.minor, v.build)
        } else {
            "?".into()
        }
    }
}

// ---------------------------------------------------------------- window procedures

unsafe extern "system" fn overlay_proc(h: isize, msg: u32, wp: usize, lp: isize) -> isize {
    if msg == WM_NCHITTEST {
        return -1; // HTTRANSPARENT: never intercept the mouse
    }
    DefWindowProcW(h, msg, wp, lp)
}

unsafe extern "system" fn main_proc(hwnd: isize, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_INPUT => {
            // A button change goes to the controller at once; any report (also plain movement) switches to
            // polling, so Raw Input stops waking us for every report of a fast mouse.
            if let Some(st) = state() {
                let ev = raw_event(st, lp);
                if ev.wheel != 0 && st.zooming.is_some() && st.ctl.m.set.zoom_scroll {
                    zoom_wheel(st, ev.wheel);
                }
                if let Some(down) = ev.zoom {
                    zoom_key(st, down);
                }
                if let Some(held) = ev.aim {
                    st.raw_down_at = if held { Some(Instant::now()) } else { None };
                    if held != st.ctl.aim_held {
                        handle(st, Ev::Aim(held));
                    }
                }
                // (Not while zooming with the wheel: the wheel needs Raw Input. Not for typing either, when the
                // keyboard is only here for the zoom key.)
                let aim_packet = !ev.kbd || st.aim_with.0 == 4;
                if st.aim_on && aim_packet && !st.polling && !(st.zooming.is_some() && st.ctl.m.set.zoom_scroll) {
                    start_polling(st);
                }
            }
            // Falls through to DefWindowProc, which must see WM_INPUT so Windows frees the packet.
        }
        WM_EVENTS => {
            if let Some(st) = state() {
                if drain(st) {
                    PostMessageW(hwnd, WM_CLOSE, 0, 0);
                }
            }
            return 0;
        }
        WM_TIMER => {
            if let Some(st) = state() {
                match wp {
                    TIMER_ANIM => {
                        present(st, false);
                        schedule(st);
                    }
                    TIMER_HOUSE => housekeeping(st),
                    TIMER_FADE => fade_step(st),
                    TIMER_CONTRAST => contrast_step(st),
                    TIMER_ZOOM => zoom_step(st),
                    _ => {}
                }
            }
            return 0;
        }
        WM_HOTKEY => {
            if let Some(st) = state() {
                if handle(st, Ev::Hotkey(wp as u32)) {
                    PostMessageW(hwnd, WM_CLOSE, 0, 0);
                }
                gamebar_flush(st);
            }
            return 0;
        }
        zoom_host::ZM_READY => {
            if let Some(st) = state() {
                zoom_ready(st, wp as isize, lp);
            }
            return 0;
        }
        zoom_host::ZM_BYE => {
            if let Some(st) = state() {
                zoom_bye(st, wp as isize, lp);
            }
            return 0;
        }
        WM_TRAY => {
            let ev = (lp & 0xFFFF) as u32;
            if ev == WM_LBUTTONUP {
                if let Some(st) = state() {
                    handle(st, Ev::OpenSettings);
                }
            } else if ev == WM_RBUTTONUP {
                // No state reference is held while the menu pumps messages.
                let on = state().map(|s| s.ctl.m.set.overlay_on).unwrap_or(true);
                let cmd = tray_menu(hwnd, on);
                if let Some(st) = state() {
                    let ev = match cmd {
                        1 => Some(Ev::OpenSettings),
                        // A hotkey may have toggled it while the menu was open: only toggle if it still reads the same.
                        2 if st.ctl.m.set.overlay_on == on => Some(Ev::ToggleOverlay),
                        3 => Some(Ev::Quit),
                        4 => {
                            // Opens the settings window, which downloads + checks + runs the installer; the installer
                            // quits and restarts this overlay itself.
                            crosshair_core::update::spawn_install();
                            None
                        }
                        _ => None,
                    };
                    if let Some(ev) = ev {
                        if handle(st, ev) {
                            PostMessageW(hwnd, WM_CLOSE, 0, 0);
                        }
                        gamebar_flush(st);
                    }
                }
            }
            return 0;
        }
        WM_DISPLAYCHANGE => {
            if let Some(st) = state() {
                st.monitors = enum_monitors();
                st.ctl.monitors = monitor_names(&st.monitors);
                st.ctl.m.monitors = st.monitors.len() as u32;
                st.ctl.send_info();
                present(st, false);
            }
        }
        WM_QUERYENDSESSION => return 1,
        WM_ENDSESSION => {
            // Windows is shutting down / logging off: save now, the process may end without a WM_CLOSE.
            if wp != 0 {
                if let Some(st) = state() {
                    shutdown(st);
                }
            }
            return 0;
        }
        WM_CLOSE => {
            if let Some(st) = state() {
                shutdown(st);
                DestroyWindow(st.overlay);
            }
            DestroyWindow(hwnd);
            return 0;
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            return 0;
        }
        _ => {
            if msg >= 0xC000 && msg == TASKBAR_MSG.load(Ordering::Relaxed) {
                if let Some(st) = state() {
                    tray(st, 0); // Explorer restarted: put the icon back
                }
            }
        }
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

// ---------------------------------------------------------------- entry point

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        crosshair_core::log::line("error", &format!("crash: {info}"));
        let msg = format!("{} hit an unexpected error and has to close.\n\n{info}", crosshair_core::APP_NAME);
        let dir = paths::config_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("crash.log"), &msg);
        unsafe {
            MessageBoxW(0, wide(&msg).as_ptr(), wide(crosshair_core::APP_NAME).as_ptr(), 0x10);
        }
    }));
}

/// One overlay per user session (and per CROSSHAIR_HOME test folder, so a test copy can run next to the real one).
fn mutex_name() -> String {
    match std::env::var_os("CROSSHAIR_HOME").filter(|v| !v.is_empty()) {
        Some(h) => {
            // FNV-1a of the folder: mutex names can't contain more backslashes.
            let mut x: u64 = 0xcbf2_9ce4_8422_2325;
            for b in h.to_string_lossy().bytes() {
                x = (x ^ b as u64).wrapping_mul(0x100_0000_01b3);
            }
            format!("Local\\Aimdot.Overlay.{x:016x}")
        }
        None => "Local\\Aimdot.Overlay".into(),
    }
}

pub fn run(tray_only: bool, autostart: bool) {
    install_panic_hook();
    unsafe {
        SetProcessDpiAwarenessContext(-4); // per-monitor v2: monitor rectangles in real pixels

        // Single instance: a second start shows the running overlay's settings window (nothing with --tray).
        CreateMutexW(null(), 0, wide(&mutex_name()).as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if !tray_only {
                ipc_server::tell_running("open");
            }
            return;
        }

        let hinst = GetModuleHandleW(null());
        let cls_main = wide(CLASS_MAIN);
        let cls_overlay = wide(CLASS_OVERLAY);
        let mut wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: main_proc,
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: app_icon(hinst, GetSystemMetrics(SM_CXICON)),
            hCursor: 0,
            hbrBackground: 0,
            lpszMenuName: null(),
            lpszClassName: cls_main.as_ptr(),
            hIconSm: app_icon(hinst, GetSystemMetrics(SM_CXSMICON)),
        };
        RegisterClassExW(&wc);
        wc.lpfnWndProc = overlay_proc;
        wc.lpszClassName = cls_overlay.as_ptr();
        RegisterClassExW(&wc);

        let title = wide(crosshair_core::APP_NAME);
        // Hidden top-level window (never shown): tray, hotkeys, Raw Input. Top-level (not message-only) so it
        // also gets the TaskbarCreated and display-change broadcasts.
        let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, cls_main.as_ptr(), title.as_ptr(), WS_POPUP, 0, 0, 0, 0, 0, 0, hinst, null_mut());
        let overlay = CreateWindowExW(WS_EX_OVERLAY, cls_overlay.as_ptr(), title.as_ptr(), WS_POPUP, 0, 0, 8, 8, 0, 0, hinst, null_mut());
        if hwnd == 0 || overlay == 0 {
            crosshair_core::log::line("error", "could not create the crosshair window");
            MessageBoxW(0, wide("Could not create the crosshair window.").as_ptr(), title.as_ptr(), 0x10);
            return;
        }
        MAIN_HWND.store(hwnd, Ordering::Relaxed);

        // Settings-window connection: the IPC threads queue events and wake this thread with one message.
        let (tx, rx) = mpsc::channel::<Ev>();
        let send: ipc_server::Send = Arc::new(move |ev| {
            let _ = tx.send(ev);
            if !POSTED.swap(true, Ordering::AcqRel) {
                PostMessageW(MAIN_HWND.load(Ordering::Relaxed), WM_EVENTS, 0, 0);
            }
        });
        let token = match ipc_server::start(send) {
            Ok(Start::Listening(t)) => t,
            Ok(Start::Running) => return,
            Err(_) => None, // no settings connection; the crosshair still works from config.ini + hotkeys
        };

        let mut ctl = Ctl::new(token);
        ctl.platform = "win32".into(); // the settings window shows it as "Windows overlay window"
        ctl.gamebar = gamebar::status().into();
        crosshair_core::log::start("overlay", &format!("Windows {}, overlay window ok, Game Bar widget {}", windows_version(), ctl.gamebar));
        crosshair_core::update::spawn_check_if_due();
        let monitors = enum_monitors();
        ctl.monitors = monitor_names(&monitors);
        ctl.m.monitors = monitors.len() as u32;
        let icon_px = GetSystemMetrics(SM_CXSMICON).clamp(16, 64);
        // Tray: the Aimdot app icon; the drawn crosshair tile only if the exe has no icon resource.
        let icon = match app_icon(hinst, icon_px) {
            0 => make_icon(&crosshair_core::model::icon(icon_px, ctl.m.set.accent)),
            i => i,
        };

        let st = Box::into_raw(Box::new(State {
            ctl,
            rx,
            hwnd,
            overlay,
            icon,
            pic: Canvas::new(0, 0),
            last: None,
            mem_dc: 0,
            dib: 0,
            dib_bits: null_mut(),
            dib_size: (0, 0),
            shown: false,
            pos: (i32::MIN, i32::MIN),
            monitors,
            anim: false,
            autostart: None,
            eco: None,
            aim_on: false,
            aim_with: (0, 0),
            kbd_raw: false,
            zoom_key_down: false,
            zoom_dirty: false,
            raw_input: false,
            aim_timer: CreateWaitableTimerExW(null(), null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS),
            polling: false,
            poll_until: Instant::now(),
            raw_down_at: None,
            distrust_until: None,
            gb: gamebar::GameBar::new(),
            // True at start: in normal mode the first sync blanks a widget left showing by an overlay that crashed.
            gb_active: true,
            gb_dirty: true,
            follow_mon: None,
            fg_hook: 0,
            cursor_hook: 0,
            alpha: 255,
            fade: None,
            update_look: Instant::now(),
            contrast: false,
            smp_dc: 0,
            smp_bits: null_mut(),
            zoom_helper: 0,
            zoom_spawned: None,
            zooming: None,
        }));
        STATE.store(st, Ordering::Relaxed);
        TASKBAR_MSG.store(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()), Ordering::Relaxed);
        let st = &mut *st;

        apply_system(st); // Efficiency mode first thing, then hotkeys + autostart
        apply_aim(st);
        apply_hooks(st);
        crate::games::win_set_needed(st.ctl.m.games_needed(), on_game);
        tray(st, 0);
        present(st, true);
        schedule(st);
        SetTimer(hwnd, TIMER_HOUSE, 1000, null());
        // Started with the PC: the window too, unless "Start minimised" (Echo's way, user 11 Nov 2026).
        if st.ctl.first_run() || !tray_only || (autostart && !st.ctl.m.set.start_minimized) {
            st.ctl.open_settings();
        }
        // Events that arrived before the state existed.
        if drain(st) {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }

        // Like GetMessage, but also wakes for the aim-polling timer (only ticking while the mouse moves).
        let timer = st.aim_timer;
        let n = (timer != 0) as u32;
        let mut msg: MSG = std::mem::zeroed();
        'run: loop {
            let r = MsgWaitForMultipleObjectsEx(n, &timer, INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            if n == 1 && r == WAIT_OBJECT_0 {
                if let Some(st) = state() {
                    aim_poll(st);
                }
            }
            while PeekMessageW(&mut msg, 0, 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    break 'run;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let st = STATE.swap(null_mut(), Ordering::Relaxed);
        if !st.is_null() {
            let st = Box::from_raw(st);
            if st.aim_timer != 0 {
                CloseHandle(st.aim_timer);
            }
            if st.dib != 0 {
                DeleteObject(st.dib);
            }
            if st.mem_dc != 0 {
                DeleteDC(st.mem_dc);
            }
            if st.icon != 0 {
                DestroyIcon(st.icon);
            }
        }
    }
}
