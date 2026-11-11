//! Echo's own title bar (Echo's prototype-native/src/titlebar.rs, trimmed): no system frame, the empty top-bar
//! space drags the window (native move, so Windows snap still works), double-click maximizes, own minimize /
//! maximize / close, 5 px invisible edges resize. CROSSHAIR_NATIVE_FRAME=1 brings the normal frame back.
//! No extra crates: the two Windows calls are declared here by hand.
use crate::AppWindow;
use slint::winit_030::{winit, WinitWindowAccessor};
use slint::ComponentHandle;

/// Linux (Wayland/X11): after drag_window / drag_resize_window the compositor runs the move and the button release
/// never reaches the app, so Slint kept the pressed TouchArea grabbed (Echo, 9 Nov 2026). Hand Slint the release.
fn end_grab(_a: &AppWindow) {
    #[cfg(not(windows))]
    {
        let w = _a.as_weak();
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            if let Some(a) = w.upgrade() {
                a.window().dispatch_event(slint::platform::WindowEvent::PointerReleased {
                    position: slint::LogicalPosition::new(1.0, 1.0),
                    button: slint::platform::PointerEventButton::Left,
                });
            }
        });
    }
}

pub fn wire(app: &AppWindow) {
    let custom = std::env::var_os("CROSSHAIR_NATIVE_FRAME").is_none();
    app.set_custom_frame(custom);
    // Linux: start the drag only after the mouse moved 4 px (a drag on press eats the 2nd click of a double-click)
    app.set_drag_on_move(cfg!(not(windows)));
    #[cfg(windows)]
    win::after_shown(app, custom);
    let w = app.as_weak();
    app.on_win_act(move |what| {
        let Some(a) = w.upgrade() else { return };
        let win = a.window();
        match what.as_str() {
            "min" => win.set_minimized(true),
            "max" => {
                win.set_maximized(!win.is_maximized());
                a.set_win_max(win.is_maximized());
            }
            // the window size changed (maximized by the system: snap, Win+Up, …): keep the button icon right
            "size" => {
                let m = win.is_maximized();
                if a.get_win_max() != m {
                    a.set_win_max(m);
                }
            }
            "close" => {
                let _ = slint::quit_event_loop();
            }
            "drag" => {
                win.with_winit_window(|ww| {
                    let _ = ww.drag_window();
                });
                end_grab(&a);
            }
            r if r.starts_with("resize:") => {
                use winit::window::ResizeDirection as D;
                let d = match &r[7..] {
                    "n" => D::North,
                    "s" => D::South,
                    "e" => D::East,
                    "w" => D::West,
                    "ne" => D::NorthEast,
                    "nw" => D::NorthWest,
                    "se" => D::SouthEast,
                    _ => D::SouthWest,
                };
                win.with_winit_window(|ww| {
                    let _ = ww.drag_resize_window(d);
                });
                end_grab(&a);
            }
            _ => {}
        }
    });
}

/// Windows: a frameless window keeps its invisible 8 px resize border, so MAXIMIZED it hangs 8 px past every screen
/// edge (the bar's top cut, the bottom under the taskbar). Echo's fix: while maximized, WM_NCCALCSIZE makes the
/// client area exactly the monitor's work area. ⚠ untested here (type-checked only); Echo's version is tested.
#[cfg(windows)]
mod win {
    use super::*;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    #[repr(C)]
    struct MonitorInfo {
        size: u32,
        monitor: Rect,
        work: Rect,
        flags: u32,
    }
    #[repr(C)]
    struct NcCalcSizeParams {
        rgrc: [Rect; 3],
        pos: *mut core::ffi::c_void,
    }
    type SubProc = unsafe extern "system" fn(isize, u32, usize, isize, usize, usize) -> isize;
    #[link(name = "user32")]
    extern "system" {
        fn GetWindowLongW(h: isize, i: i32) -> i32;
        fn IsZoomed(h: isize) -> i32;
        fn MonitorFromWindow(h: isize, flags: u32) -> isize;
        fn GetMonitorInfoW(m: isize, mi: *mut MonitorInfo) -> i32;
        fn LoadImageW(inst: isize, name: *const u16, ty: u32, cx: i32, cy: i32, flags: u32) -> isize;
        fn SendMessageW(h: isize, msg: u32, wp: usize, lp: isize) -> isize;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> isize;
    }
    #[link(name = "comctl32")]
    extern "system" {
        fn SetWindowSubclass(h: isize, p: SubProc, id: usize, data: usize) -> i32;
        fn DefSubclassProc(h: isize, msg: u32, wp: usize, lp: isize) -> isize;
    }
    const WM_NCCALCSIZE: u32 = 0x0083;
    const GWL_STYLE: i32 = -16;
    const WS_THICKFRAME: u32 = 0x0004_0000;
    const MONITOR_DEFAULTTONEAREST: u32 = 2;
    const WM_SETICON: u32 = 0x0080;
    const ICON_BIG: usize = 1;
    const IMAGE_ICON: u32 = 1;
    const LR_DEFAULTSIZE: u32 = 0x0040;

    /// winit only sets the window's SMALL icon (Slint's `icon:`); the taskbar uses the BIG one: take it from the
    /// exe's own icon (build.rs embeds aimdot.ico as icon 1) - Echo's big_icon(). ⚠ untested
    unsafe fn big_icon(h: isize) {
        let icon = LoadImageW(GetModuleHandleW(std::ptr::null()), 1 as *const u16, IMAGE_ICON, 0, 0, LR_DEFAULTSIZE);
        if icon != 0 {
            SendMessageW(h, WM_SETICON, ICON_BIG, icon);
        }
    }

    unsafe extern "system" fn proc(h: isize, msg: u32, wp: usize, lp: isize, _id: usize, _data: usize) -> isize {
        let sizable = (GetWindowLongW(h, GWL_STYLE) as u32 & WS_THICKFRAME) != 0;
        if msg == WM_NCCALCSIZE && wp != 0 && sizable && IsZoomed(h) != 0 {
            let mut mi = MonitorInfo { size: std::mem::size_of::<MonitorInfo>() as u32, monitor: Rect { left: 0, top: 0, right: 0, bottom: 0 }, work: Rect { left: 0, top: 0, right: 0, bottom: 0 }, flags: 0 };
            if GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), &mut mi) != 0 {
                let p = lp as *mut NcCalcSizeParams;
                (*p).rgrc[0] = mi.work;
                return 0;
            }
        }
        DefSubclassProc(h, msg, wp, lp)
    }

    fn hwnd(a: &AppWindow) -> Option<isize> {
        let mut out = None;
        a.window().with_winit_window(|ww| {
            if let Ok(RawWindowHandle::Win32(h)) = ww.window_handle().map(|h| h.as_raw()) {
                out = Some(h.hwnd.get());
            }
        });
        out
    }

    thread_local! {
        static HOOK: slint::Timer = slint::Timer::default();
    }

    /// winit makes the native window only once the event loop shows it: try every 100 ms until it exists (max 10 s)
    pub fn after_shown(app: &AppWindow, custom: bool) {
        let w = app.as_weak();
        let tries = std::cell::Cell::new(0u32);
        HOOK.with(|t| {
            t.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(100), move || {
                let Some(a) = w.upgrade() else { return };
                tries.set(tries.get() + 1);
                match hwnd(&a) {
                    Some(h) => unsafe {
                        if custom {
                            SetWindowSubclass(h, proc, 0xA1D0, 0);
                        }
                        big_icon(h);
                    },
                    None if tries.get() < 100 => return,
                    None => {}
                }
                HOOK.with(|t| t.stop());
            });
        });
    }
}
