//! The zoom helper: `aimdot --zoom-host <overlay main window> <crosshair window>` (#15 hold to zoom).
//!
//! Why a second process: the Magnification API pulls in Direct3D 9 + the graphics driver (AMD: ~90 MB private,
//! measured 11 Nov 2026) and Windows keeps the driver loaded for the life of the process, even after
//! MagUninitialize. In here it is freed by quitting: the overlay itself never loads magnification.dll and stays at
//! ~3 MB. The overlay starts this helper on a zoom key press and keeps all the zoom logic (key, hold / toggle,
//! only while aiming, wheel); the helper only shows what it is told:
//!
//! - overlay -> helper (PostMessage to the helper's message-only window): [`ZM_LEVEL`], [`ZM_SHOW`], [`ZM_HIDE`],
//!   WM_CLOSE (the overlay quits).
//! - helper -> overlay (PostMessage to the overlay's main window): [`ZM_READY`] once the magnifier is set up,
//!   [`ZM_BYE`] when it quits.
//!
//! It quits by itself [`IDLE_MS`] after the last zoom (test hook `CROSSHAIR_ZOOM_FREE_MS=<ms>`), or at once when the
//! overlay's process ends. It writes no files: what's worth logging goes back to the overlay in the messages.

use super::ffi::*;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicPtr, Ordering};

/// overlay -> helper: zoom factor x100 in wParam (sent before ZM_SHOW, and when the wheel changes it).
pub const ZM_LEVEL: u32 = WM_APP + 20;
/// overlay -> helper: show the lens. wParam = size (px, low 16 bits) | round << 16 | ring 0xRRGGBB << 24,
/// lParam = the crosshair's centre on screen, `pack_xy`.
pub const ZM_SHOW: u32 = WM_APP + 21;
/// overlay -> helper: hide the lens (the idle countdown starts).
pub const ZM_HIDE: u32 = WM_APP + 22;
/// helper -> overlay: set up and listening. wParam = the helper's window, lParam = flags (`READY_*`).
pub const ZM_READY: u32 = WM_APP + 23;
/// helper -> overlay: quitting. wParam = the helper's window, lParam = why (`BYE_*`).
pub const ZM_BYE: u32 = WM_APP + 24;

/// ZM_READY flag: SetWindowDisplayAffinity failed (Windows before 10 2004): the lens shows in captures.
pub const READY_NO_HIDE: isize = 1;
/// ZM_BYE reasons.
pub const BYE_IDLE: isize = 0;
pub const BYE_NO_MAGNIFIER: isize = 1;
pub const BYE_NO_WINDOW: isize = 2;

/// The helper quits this long after the last zoom ended (or after it started, if no zoom came). The next zoom
/// starts it again (a short delay on that one press).
pub const IDLE_MS: u32 = 180_000;
const TIMER_IDLE: usize = 2;
const CLASS_LENS: &str = "AimdotZoom";
const CLASS_CTL: &str = "AimdotZoomHost";

pub fn pack_xy(x: i32, y: i32) -> isize {
    ((x as u32 as u64) | ((y as u32 as u64) << 32)) as isize
}
fn unpack_xy(l: isize) -> (i32, i32) {
    let v = l as u64;
    (v as u32 as i32, (v >> 32) as u32 as i32)
}
pub fn pack_show(size: i32, round: bool, rgb: u32) -> usize {
    (size.clamp(0, 0xFFFF) as u64 | (round as u64) << 16 | ((rgb & 0xFF_FFFF) as u64) << 24) as usize
}

struct Mag {
    source: MagSetWindowSourceFn,
    transform: MagSetWindowTransformFn,
}

struct Host {
    overlay_main: isize,
    ctl: isize,
    lens: isize,
    mag: isize,
    ring: isize,
    api: Mag,
    /// DwmFlush (dwmapi.dll, loaded here only): waits for the next screen refresh (fallback pacing, wait_frame).
    flush: Option<DwmFlushFn>,
    /// The zoom shown now, the newest zoom asked for (ZM_LEVEL; taken on the next frame), and what the magnifier
    /// has (0 = nothing set yet).
    level: f32,
    target: f32,
    applied: f32,
    size: i32,
    centre: (i32, i32),
    shown: bool,
    /// What the ring was last drawn for (size, round, colour).
    ring_key: (i32, bool, u32),
    idle_ms: u32,
}

static HOST: AtomicPtr<Host> = AtomicPtr::new(null_mut());

unsafe fn host<'a>() -> Option<&'a mut Host> {
    HOST.load(Ordering::Relaxed).as_mut()
}

unsafe fn sym<T>(module: isize, name: &[u8]) -> Option<T> {
    let p = GetProcAddress(module, name.as_ptr());
    (!p.is_null()).then(|| std::mem::transmute_copy::<*const c_void, T>(&p))
}

unsafe extern "system" fn lens_proc(h: isize, msg: u32, wp: usize, lp: isize) -> isize {
    if msg == WM_NCHITTEST {
        return -1; // HTTRANSPARENT: never intercept the mouse
    }
    DefWindowProcW(h, msg, wp, lp)
}

unsafe extern "system" fn ctl_proc(h: isize, msg: u32, wp: usize, lp: isize) -> isize {
    let Some(st) = host() else { return DefWindowProcW(h, msg, wp, lp) };
    match msg {
        ZM_LEVEL => {
            // Only the goal: the next frame takes it (wheel ticks that come faster than frames count once).
            st.target = (wp as f32 / 100.0).clamp(1.0, 16.0);
            0
        }
        ZM_SHOW => {
            show(st, (wp & 0xFFFF) as i32, (wp >> 16) & 1 == 1, ((wp >> 24) & 0xFF_FFFF) as u32, unpack_xy(lp));
            0
        }
        ZM_HIDE => {
            hide(st);
            0
        }
        WM_TIMER => {
            if wp == TIMER_IDLE {
                quit(st, BYE_IDLE);
            }
            0
        }
        WM_CLOSE => {
            // The overlay is quitting (it doesn't need a ZM_BYE).
            hide(st);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

unsafe fn quit(st: &mut Host, why: isize) {
    hide(st);
    PostMessageW(st.overlay_main, ZM_BYE, st.ctl as usize, why);
    PostQuitMessage(0);
}

unsafe fn show(st: &mut Host, size: i32, round: bool, rgb: u32, centre: (i32, i32)) {
    KillTimer(st.ctl, TIMER_IDLE);
    let size = size.clamp(64, 4000);
    st.size = size;
    st.centre = centre;
    let (x0, y0) = (centre.0 - size / 2, centre.1 - size / 2);
    SetWindowPos(st.lens, HWND_TOPMOST, x0, y0, size, size, SWP_NOACTIVATE);
    SetWindowPos(st.mag, 0, 0, 0, size, size, SWP_NOZORDER | SWP_NOACTIVATE);
    // Round = a window region (the system owns the region afterwards); square = none.
    SetWindowRgn(st.lens, if round { CreateEllipticRgn(0, 0, size + 1, size + 1) } else { 0 }, 1);
    draw_ring(st, size, round, rgb, (x0, y0));
    st.level = st.target;
    apply(st); // the first picture before the lens shows (no flash of an old one)
    // Order: lens, ring, crosshair on top (the overlay's own window, raised from here).
    ShowWindow(st.lens, SW_SHOWNOACTIVATE);
    SetWindowPos(st.ring, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    ShowWindow(st.ring, SW_SHOWNOACTIVATE);
    SetWindowPos(crosshair_window(), HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    st.shown = true; // the loop in run() now redraws it frame by frame (frame + wait_frame)
}

unsafe fn hide(st: &mut Host) {
    if st.shown {
        st.shown = false;
        ShowWindow(st.lens, SW_HIDE);
        ShowWindow(st.ring, SW_HIDE);
    }
    SetTimer(st.ctl, TIMER_IDLE, st.idle_ms, null());
}

/// One frame while the lens shows: the newest zoom goal (wheel ticks that came since the last frame count once),
/// then a new picture. No glide between levels: a zoom change (MagSetWindowTransform) is the expensive part of a
/// frame (measured 11 Nov 2026: 25 ms on average, up to 35 ms, against 12 ms for a normal frame), so it happens
/// once per wheel step, never on every frame of an animation.
unsafe fn frame(st: &mut Host) {
    st.level = st.target;
    apply(st);
}

/// Sets the zoom + the part of the screen to show, and draws it now. The source is rounded UP, so source x zoom
/// always fills the lens (rounded down it left a 1-9 px black edge at the right / bottom that changed with every
/// wheel step). On a zoom change the two calls go in the order that never has less picture than lens in between:
/// zooming in = zoom first (the old, larger source just overflows), zooming out = the larger source first.
unsafe fn apply(st: &mut Host) {
    if st.size == 0 {
        return;
    }
    let l = st.level;
    let w = ((st.size as f32 / l).ceil() as i32).max(1);
    let (cx, cy) = st.centre;
    let src = RECT { left: cx - w / 2, top: cy - w / 2, right: cx - w / 2 + w, bottom: cy - w / 2 + w };
    let t = MAGTRANSFORM { v: [l, 0.0, 0.0, 0.0, l, 0.0, 0.0, 0.0, 1.0] };
    if l != st.applied {
        if l > st.applied {
            (st.api.transform)(st.mag, &t);
            (st.api.source)(st.mag, src);
        } else {
            (st.api.source)(st.mag, src);
            (st.api.transform)(st.mag, &t);
        }
        st.applied = l;
    } else {
        (st.api.source)(st.mag, src);
    }
    InvalidateRect(st.mag, null(), 0);
    UpdateWindow(st.mag); // paint now, not whenever WM_PAINT comes round
}

/// Paces the frame loop. The Magnifier control's paint itself waits for its own present, which runs at 60 Hz
/// (measured on a 240 Hz AMD screen, 11 Nov 2026: UpdateWindow takes 16.6 ms, so the loop draws 60 pictures a
/// second; an extra DwmFlush after it only made frames late: 20.5 ms). The old 16 ms window timer really ticked
/// every ~31 ms (Windows timers step in 15.6 ms, jitter up to 46 ms): ~32 pictures a second, which looked laggy,
/// worst while the zoom changed. Only if a paint came back early (`drawn_ms` < 4: e.g. a driver that doesn't
/// wait), wait for the next screen refresh (DwmFlush), so the loop never spins; no DwmFlush: ~8 ms sleeps.
unsafe fn wait_frame(st: &Host, drawn_ms: f32) {
    if drawn_ms >= 4.0 {
        return;
    }
    if let Some(flush) = st.flush {
        if flush() >= 0 {
            return;
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(8));
}

static CROSSHAIR: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
fn crosshair_window() -> isize {
    CROSSHAIR.load(Ordering::Relaxed)
}

/// The 2 px ring at the lens' edge in its own colour (zoom_ring, white by default), so you see where the lens
/// ends. Drawn only when the size, shape or colour changed (a per-pixel ARGB layered window; the lens itself can't
/// be half transparent).
unsafe fn draw_ring(st: &mut Host, size: i32, round: bool, rgb: u32, at: (i32, i32)) {
    let p = POINT { x: at.0, y: at.1 };
    let sz = SIZE { cx: size, cy: size };
    let blend = BLENDFUNCTION { op: 0, flags: 0, alpha: 255, format: 1 };
    if st.ring_key == (size, round, rgb) {
        UpdateLayeredWindow(st.ring, 0, &p, &sz, 0, null(), 0, &blend, ULW_ALPHA);
        return;
    }
    let dc = CreateCompatibleDC(0);
    let mut bits: *mut c_void = null_mut();
    let dib = CreateDIBSection(0, &bmi(size, size), 0, &mut bits, 0, 0);
    if dc == 0 || dib == 0 || bits.is_null() {
        return;
    }
    let old = SelectObject(dc, dib);
    let px = std::slice::from_raw_parts_mut(bits as *mut u32, (size * size) as usize);
    let (r, g, b) = (rgb >> 16 & 0xFF, rgb >> 8 & 0xFF, rgb & 0xFF);
    let (m, thick) = (size as f32 / 2.0, 2.0f32);
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Distance inside the edge: round = from the circle, square = from the nearest side.
            let inside = if round { m - ((fx - m).powi(2) + (fy - m).powi(2)).sqrt() } else { fx.min(fy).min(size as f32 - fx).min(size as f32 - fy) };
            let a = if inside < 0.0 { 0.0 } else { (thick + 0.5 - inside).clamp(0.0, 1.0) * inside.min(1.0) };
            let a8 = (a * 255.0).round() as u32;
            // Premultiplied BGRA
            px[(y * size + x) as usize] = a8 << 24 | (r * a8 / 255) << 16 | (g * a8 / 255) << 8 | (b * a8 / 255);
        }
    }
    let zero = POINT::default();
    UpdateLayeredWindow(st.ring, 0, &p, &sz, dc, &zero, 0, &blend, ULW_ALPHA);
    SelectObject(dc, old);
    DeleteObject(dib);
    DeleteDC(dc);
    st.ring_key = (size, round, rgb);
}

/// `aimdot --zoom-host <overlay main window> <crosshair window>` (decimal handles).
pub fn run(args: &[String]) {
    let num = |i: usize| args.get(i).and_then(|v| v.parse::<isize>().ok()).unwrap_or(0);
    let (overlay_main, crosshair) = (num(0), num(1));
    unsafe {
        if overlay_main == 0 || IsWindow(overlay_main) == 0 {
            return; // started by hand, or the overlay is already gone
        }
        CROSSHAIR.store(crosshair, Ordering::Relaxed);
        SetProcessDpiAwarenessContext(-4); // per-monitor v2: the overlay's positions are real pixels
        // Watch the overlay: when its process ends, so does this one (the wait wakes the loop below).
        let mut pid = 0u32;
        GetWindowThreadProcessId(overlay_main, &mut pid);
        let parent = if pid != 0 { OpenProcess(SYNCHRONIZE, 0, pid) } else { 0 };

        let hinst = GetModuleHandleW(null());
        let cls_ctl = wide(CLASS_CTL);
        let cls_lens = wide(CLASS_LENS);
        let mut wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: ctl_proc,
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: 0,
            hCursor: 0,
            hbrBackground: 0,
            lpszMenuName: null(),
            lpszClassName: cls_ctl.as_ptr(),
            hIconSm: 0,
        };
        RegisterClassExW(&wc);
        wc.lpfnWndProc = lens_proc;
        wc.lpszClassName = cls_lens.as_ptr();
        RegisterClassExW(&wc);
        let ctl = CreateWindowExW(0, cls_ctl.as_ptr(), null(), 0, 0, 0, 0, 0, HWND_MESSAGE, 0, hinst, null_mut());
        let bye = |why: isize| {
            PostMessageW(overlay_main, ZM_BYE, ctl as usize, why);
        };
        if ctl == 0 {
            bye(BYE_NO_WINDOW);
            return;
        }

        // magnification.dll only in here (see the top of the file).
        let lib = LoadLibraryW(wide("magnification.dll").as_ptr());
        let api = (|| {
            let init: MagInitializeFn = sym(lib, b"MagInitialize\0")?;
            let filter: MagSetWindowFilterListFn = sym(lib, b"MagSetWindowFilterList\0")?;
            let source: MagSetWindowSourceFn = sym(lib, b"MagSetWindowSource\0")?;
            let transform: MagSetWindowTransformFn = sym(lib, b"MagSetWindowTransform\0")?;
            (init() != 0).then_some((filter, Mag { source, transform }))
        })();
        let Some((filter, api)) = (lib != 0).then_some(api).flatten() else {
            bye(BYE_NO_MAGNIFIER);
            return;
        };

        let title = wide(crosshair_core::APP_NAME);
        let lens = CreateWindowExW(WS_EX_OVERLAY, cls_lens.as_ptr(), title.as_ptr(), WS_POPUP, 0, 0, 64, 64, 0, 0, hinst, null_mut());
        let ring = CreateWindowExW(WS_EX_OVERLAY, cls_lens.as_ptr(), title.as_ptr(), WS_POPUP, 0, 0, 64, 64, 0, 0, hinst, null_mut());
        let mag = if lens != 0 { CreateWindowExW(0, wide("Magnifier").as_ptr(), null(), WS_CHILD | WS_VISIBLE, 0, 0, 64, 64, lens, 0, hinst, null_mut()) } else { 0 };
        if lens == 0 || ring == 0 || mag == 0 {
            bye(BYE_NO_WINDOW);
            return;
        }
        SetLayeredWindowAttributes(lens, 0, 255, LWA_ALPHA);
        // Don't magnify the crosshair (it stays sharp on top; a window of the overlay's process works here), the
        // ring or the magnifier itself.
        let skip = [crosshair, lens, ring];
        filter(mag, MW_FILTERMODE_EXCLUDE, 3, skip.as_ptr());
        // The lens + ring never show in recordings / clips / Discord screen shares / screenshots (only on your
        // screen), like Scope X (user, 11 Nov 2026). Older Windows (before 10 2004) doesn't know it: shown as before.
        // test hook: CROSSHAIR_ZOOM_CAPTURE=1 leaves them in captures (a test screenshot can see the lens).
        let mut flags = 0;
        if std::env::var_os("CROSSHAIR_ZOOM_CAPTURE").is_none() {
            for w in [lens, ring] {
                if SetWindowDisplayAffinity(w, WDA_EXCLUDEFROMCAPTURE) == 0 {
                    flags |= READY_NO_HIDE;
                }
            }
        }
        // test hook: CROSSHAIR_ZOOM_FREE_MS=<ms> (a test copy can check the quit in seconds)
        let idle_ms = std::env::var("CROSSHAIR_ZOOM_FREE_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(IDLE_MS);
        let st = Box::into_raw(Box::new(Host {
            overlay_main,
            ctl,
            lens,
            mag,
            ring,
            api,
            // dwmapi.dll only in here too (the overlay itself doesn't load it).
            flush: sym(LoadLibraryW(wide("dwmapi.dll").as_ptr()), b"DwmFlush\0"),
            level: 2.0,
            target: 2.0,
            applied: 0.0,
            size: 0,
            centre: (0, 0),
            shown: false,
            ring_key: (0, false, 0),
            idle_ms,
        }));
        HOST.store(st, Ordering::Relaxed);
        // Quits by itself if no zoom comes (the key was let go before this was ready).
        SetTimer(ctl, TIMER_IDLE, idle_ms, null());
        if PostMessageW(overlay_main, ZM_READY, ctl as usize, flags) == 0 {
            return; // the overlay went away meanwhile
        }

        let n = (parent != 0) as u32;
        let mut msg: MSG = std::mem::zeroed();
        'run: loop {
            // Lens hidden: sleep until a message (or the overlay's end). Shown: only look, then draw a frame below.
            let shown = host().is_some_and(|st| st.shown);
            let wait = if shown { 0 } else { INFINITE };
            let r = MsgWaitForMultipleObjectsEx(n, &parent, wait, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            if n == 1 && r == WAIT_OBJECT_0 {
                if let Some(st) = host() {
                    hide(st);
                }
                break; // the overlay's process ended (crash, killed): nobody to zoom for
            }
            while PeekMessageW(&mut msg, 0, 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    break 'run;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            // One picture per frame while the lens shows (messages wait at most one frame, ~17 ms).
            if let Some(st) = host().filter(|st| st.shown) {
                let t = std::time::Instant::now();
                frame(st);
                wait_frame(st, t.elapsed().as_secs_f32() * 1000.0);
            }
        }
        // Process exit frees the windows, the magnifier and the driver; no MagUninitialize needed.
    }
}
