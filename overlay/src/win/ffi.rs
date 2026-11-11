//! The Win32 calls the overlay uses, declared by hand (no crates; `raw-dylib` = no import libraries and no C
//! toolchain needed). From the first version's win.rs, minus the editor / GDI+ / file dialog / drag & drop parts, plus
//! Raw Input and the Efficiency-mode calls.
#![allow(non_snake_case, non_camel_case_types, clippy::upper_case_acronyms, dead_code)]

use std::ffi::c_void;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct POINT {
    pub x: i32,
    pub y: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SIZE {
    pub cx: i32,
    pub cy: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq)]
pub struct RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
#[repr(C)]
pub struct MSG {
    pub hwnd: isize,
    pub message: u32,
    pub wParam: usize,
    pub lParam: isize,
    pub time: u32,
    pub pt: POINT,
    pub lPrivate: u32,
}
#[repr(C)]
pub struct WNDCLASSEXW {
    pub cbSize: u32,
    pub style: u32,
    pub lpfnWndProc: unsafe extern "system" fn(isize, u32, usize, isize) -> isize,
    pub cbClsExtra: i32,
    pub cbWndExtra: i32,
    pub hInstance: isize,
    pub hIcon: isize,
    pub hCursor: isize,
    pub hbrBackground: isize,
    pub lpszMenuName: *const u16,
    pub lpszClassName: *const u16,
    pub hIconSm: isize,
}
#[repr(C)]
pub struct BITMAPINFOHEADER {
    pub biSize: u32,
    pub biWidth: i32,
    pub biHeight: i32,
    pub biPlanes: u16,
    pub biBitCount: u16,
    pub biCompression: u32,
    pub biSizeImage: u32,
    pub biXPelsPerMeter: i32,
    pub biYPelsPerMeter: i32,
    pub biClrUsed: u32,
    pub biClrImportant: u32,
}
#[repr(C)]
pub struct BITMAPINFO {
    pub h: BITMAPINFOHEADER,
    pub colors: [u32; 3],
}
#[repr(C)]
pub struct BLENDFUNCTION {
    pub op: u8,
    pub flags: u8,
    pub alpha: u8,
    pub format: u8,
}
#[repr(C)]
pub struct CURSORINFO {
    pub cbSize: u32,
    pub flags: u32,
    pub hCursor: isize,
    pub ptScreenPos: POINT,
}
pub type WINEVENTPROC = extern "system" fn(hook: isize, event: u32, hwnd: isize, id_object: i32, id_child: i32, thread: u32, time: u32);
#[repr(C)]
pub struct MONITORINFO {
    pub cbSize: u32,
    pub rcMonitor: RECT,
    pub rcWork: RECT,
    pub dwFlags: u32,
}
#[repr(C)]
pub struct ICONINFO {
    pub fIcon: i32,
    pub xHotspot: u32,
    pub yHotspot: u32,
    pub hbmMask: isize,
    pub hbmColor: isize,
}
#[repr(C)]
pub struct NOTIFYICONDATAW {
    pub cbSize: u32,
    pub hWnd: isize,
    pub uID: u32,
    pub uFlags: u32,
    pub uCallbackMessage: u32,
    pub hIcon: isize,
    pub szTip: [u16; 128],
    pub dwState: u32,
    pub dwStateMask: u32,
    pub szInfo: [u16; 256],
    pub uVersion: u32,
    pub szInfoTitle: [u16; 64],
    pub dwInfoFlags: u32,
    pub guidItem: [u32; 4],
    pub hBalloonIcon: isize,
}

// Raw Input (mouse only)
#[repr(C)]
pub struct RAWINPUTDEVICE {
    pub usUsagePage: u16,
    pub usUsage: u16,
    pub dwFlags: u32,
    pub hwndTarget: isize,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RAWINPUTHEADER {
    pub dwType: u32,
    pub dwSize: u32,
    pub hDevice: isize,
    pub wParam: usize,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RAWMOUSE {
    pub usFlags: u16,
    // union { ULONG ulButtons; struct { USHORT usButtonFlags; USHORT usButtonData; } } - 4-byte aligned,
    // so there are 2 bytes of padding after usFlags.
    pub _pad: u16,
    pub usButtonFlags: u16,
    pub usButtonData: u16,
    pub ulRawButtons: u32,
    pub lLastX: i32,
    pub lLastY: i32,
    pub ulExtraInformation: u32,
}
/// RAWINPUT with the mouse member of the union, padded to the size of the biggest member (RAWHID is
/// variable-length, but a mouse packet is always header + RAWMOUSE).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RAWINPUT_MOUSE {
    pub header: RAWINPUTHEADER,
    pub mouse: RAWMOUSE,
    pub _pad: [u32; 4],
}

// Efficiency mode
#[repr(C)]
pub struct PROCESS_POWER_THROTTLING_STATE {
    pub Version: u32,
    pub ControlMask: u32,
    pub StateMask: u32,
}

#[cfg_attr(windows, link(name = "user32", kind = "raw-dylib"))]
extern "system" {
    pub fn SetWindowDisplayAffinity(h: isize, affinity: u32) -> i32;
    pub fn RegisterClassExW(c: *const WNDCLASSEXW) -> u16;
    pub fn CreateWindowExW(ex: u32, class: *const u16, title: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: isize, menu: isize, inst: isize, param: *mut c_void) -> isize;
    pub fn DefWindowProcW(h: isize, m: u32, w: usize, l: isize) -> isize;
    pub fn GetMessageW(m: *mut MSG, h: isize, min: u32, max: u32) -> i32;
    pub fn PeekMessageW(m: *mut MSG, h: isize, min: u32, max: u32, remove: u32) -> i32;
    pub fn MsgWaitForMultipleObjectsEx(n: u32, handles: *const isize, ms: u32, wake_mask: u32, flags: u32) -> u32;
    pub fn TranslateMessage(m: *const MSG) -> i32;
    pub fn DispatchMessageW(m: *const MSG) -> isize;
    pub fn PostQuitMessage(code: i32);
    pub fn PostMessageW(h: isize, m: u32, w: usize, l: isize) -> i32;
    pub fn ShowWindow(h: isize, cmd: i32) -> i32;
    pub fn DestroyWindow(h: isize) -> i32;
    pub fn SetWindowPos(h: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
    pub fn SetTimer(h: isize, id: usize, ms: u32, f: *const c_void) -> usize;
    pub fn KillTimer(h: isize, id: usize) -> i32;
    pub fn UpdateLayeredWindow(h: isize, dst: isize, ppt: *const POINT, size: *const SIZE, src: isize, psrc: *const POINT, key: u32, blend: *const BLENDFUNCTION, flags: u32) -> i32;
    pub fn RegisterHotKey(h: isize, id: i32, mods: u32, vk: u32) -> i32;
    pub fn UnregisterHotKey(h: isize, id: i32) -> i32;
    pub fn GetAsyncKeyState(vk: i32) -> i16;
    pub fn GetSystemMetrics(i: i32) -> i32;
    pub fn EnumDisplayMonitors(hdc: isize, clip: *const RECT, f: unsafe extern "system" fn(isize, isize, *mut RECT, isize) -> i32, data: isize) -> i32;
    pub fn GetMonitorInfoW(mon: isize, mi: *mut MONITORINFO) -> i32;
    pub fn MonitorFromWindow(h: isize, flags: u32) -> isize;
    pub fn GetForegroundWindow() -> isize;
    pub fn GetShellWindow() -> isize;
    pub fn GetClassNameW(h: isize, buf: *mut u16, n: i32) -> i32;
    pub fn SetWinEventHook(min: u32, max: u32, hmod: isize, f: WINEVENTPROC, pid: u32, tid: u32, flags: u32) -> isize;
    pub fn UnhookWinEvent(h: isize) -> i32;
    pub fn GetCursorInfo(ci: *mut CURSORINFO) -> i32;
    pub fn CreatePopupMenu() -> isize;
    pub fn AppendMenuW(menu: isize, flags: u32, id: usize, text: *const u16) -> i32;
    pub fn TrackPopupMenu(menu: isize, flags: u32, x: i32, y: i32, res: i32, h: isize, r: *const RECT) -> i32;
    pub fn DestroyMenu(menu: isize) -> i32;
    pub fn GetCursorPos(p: *mut POINT) -> i32;
    pub fn SetForegroundWindow(h: isize) -> i32;
    pub fn CreateIconIndirect(ii: *const ICONINFO) -> isize;
    pub fn LoadImageW(inst: isize, name: *const u16, ty: u32, cx: i32, cy: i32, flags: u32) -> isize;
    pub fn DestroyIcon(h: isize) -> i32;
    pub fn MessageBoxW(h: isize, text: *const u16, cap: *const u16, ty: u32) -> i32;
    pub fn RegisterWindowMessageW(s: *const u16) -> u32;
    pub fn SetProcessDpiAwarenessContext(ctx: isize) -> i32;
    pub fn RegisterRawInputDevices(devs: *const RAWINPUTDEVICE, n: u32, size: u32) -> i32;
    pub fn GetRawInputData(h: isize, cmd: u32, data: *mut c_void, size: *mut u32, header_size: u32) -> u32;
}

#[cfg_attr(windows, link(name = "gdi32", kind = "raw-dylib"))]
extern "system" {
    pub fn CreateCompatibleDC(hdc: isize) -> isize;
    pub fn CreateDIBSection(hdc: isize, bmi: *const BITMAPINFO, usage: u32, bits: *mut *mut c_void, section: isize, offset: u32) -> isize;
    pub fn SelectObject(dc: isize, obj: isize) -> isize;
    pub fn DeleteObject(obj: isize) -> i32;
    pub fn DeleteDC(dc: isize) -> i32;
    pub fn CreateBitmap(w: i32, h: i32, planes: u32, bpp: u32, bits: *const c_void) -> isize;
}

#[cfg_attr(windows, link(name = "shell32", kind = "raw-dylib"))]
extern "system" {
    pub fn Shell_NotifyIconW(msg: u32, data: *const NOTIFYICONDATAW) -> i32;
}

#[cfg_attr(windows, link(name = "kernel32", kind = "raw-dylib"))]
extern "system" {
    pub fn GetModuleHandleW(name: *const u16) -> isize;
    pub fn CreateMutexW(attr: *const c_void, owner: i32, name: *const u16) -> isize;
    pub fn GetLastError() -> u32;
    pub fn GetCurrentProcess() -> isize;
    pub fn SetPriorityClass(h: isize, class: u32) -> i32;
    pub fn SetProcessInformation(h: isize, class: i32, info: *const c_void, size: u32) -> i32;
    pub fn CreateWaitableTimerExW(attr: *const c_void, name: *const u16, flags: u32, access: u32) -> isize;
    pub fn SetWaitableTimer(h: isize, due: *const i64, period_ms: i32, cb: *const c_void, arg: *const c_void, resume: i32) -> i32;
    pub fn CancelWaitableTimer(h: isize) -> i32;
    pub fn CloseHandle(h: isize) -> i32;
}

#[cfg_attr(windows, link(name = "advapi32", kind = "raw-dylib"))]
extern "system" {
    pub fn RegCreateKeyExW(key: isize, sub: *const u16, reserved: u32, class: *const u16, opts: u32, sam: u32, sec: *const c_void, out: *mut isize, disp: *mut u32) -> i32;
    pub fn RegSetValueExW(key: isize, name: *const u16, reserved: u32, ty: u32, data: *const u8, cb: u32) -> i32;
    pub fn RegDeleteValueW(key: isize, name: *const u16) -> i32;
    pub fn RegCloseKey(key: isize) -> i32;
}

pub const WM_DESTROY: u32 = 0x0002;
pub const WM_CLOSE: u32 = 0x0010;
pub const WM_QUERYENDSESSION: u32 = 0x0011;
pub const WM_ENDSESSION: u32 = 0x0016;
pub const WM_DISPLAYCHANGE: u32 = 0x007E;
pub const WM_NCHITTEST: u32 = 0x0084;
pub const WM_INPUT: u32 = 0x00FF;
pub const WM_TIMER: u32 = 0x0113;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_RBUTTONUP: u32 = 0x0205;
pub const WM_HOTKEY: u32 = 0x0312;
pub const WM_APP: u32 = 0x8000;

pub const WS_POPUP: u32 = 0x8000_0000;
/// layered | transparent (click-through) | topmost | toolwindow (no taskbar button) | noactivate
pub const WS_EX_OVERLAY: u32 = 0x0008_0000 | 0x20 | 0x8 | 0x80 | 0x0800_0000;
pub const WS_EX_TOOLWINDOW: u32 = 0x80;
pub const SW_HIDE: i32 = 0;
pub const SW_SHOWNOACTIVATE: i32 = 4;
pub const SWP_NOSIZE: u32 = 1;
pub const SWP_NOMOVE: u32 = 2;
pub const SWP_NOACTIVATE: u32 = 0x10;
pub const HWND_TOPMOST: isize = -1;
pub const ULW_ALPHA: u32 = 2;
/// SetWindowDisplayAffinity: the window is left out of screen captures (Windows 10 2004+)
pub const WDA_EXCLUDEFROMCAPTURE: u32 = 0x11;
pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;
pub const MOD_NOREPEAT: u32 = 0x4000;
pub const ERROR_ALREADY_EXISTS: u32 = 183;
pub const SM_SWAPBUTTON: i32 = 23;
pub const SM_CXSMICON: i32 = 49;
pub const SM_CXICON: i32 = 11;
pub const IMAGE_ICON: u32 = 1;
pub const VK_LBUTTON: i32 = 1;
pub const VK_RBUTTON: i32 = 2;

pub const RIDEV_REMOVE: u32 = 0x0000_0001;
pub const RIDEV_INPUTSINK: u32 = 0x0000_0100;
pub const RID_INPUT: u32 = 0x1000_0003;
pub const RIM_TYPEMOUSE: u32 = 0;
pub const RI_MOUSE_RIGHT_BUTTON_DOWN: u16 = 0x0004;
pub const RI_MOUSE_RIGHT_BUTTON_UP: u16 = 0x0008;
pub const RI_MOUSE_WHEEL: u16 = 0x0400;
pub const RIM_TYPEKEYBOARD: u32 = 1;
/// RAWKEYBOARD.Flags: the key went up.
pub const RI_KEY_BREAK: u16 = 1;
pub const VK_MBUTTON: i32 = 4;
pub const VK_XBUTTON1: i32 = 5;
pub const VK_XBUTTON2: i32 = 6;
/// RAWKEYBOARD (after the RAWINPUTHEADER in a WM_INPUT packet).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RAWKEYBOARD {
    pub MakeCode: u16,
    pub Flags: u16,
    pub Reserved: u16,
    pub VKey: u16,
    pub Message: u32,
    pub ExtraInformation: u32,
}

pub const WM_QUIT: u32 = 0x0012;
pub const PM_REMOVE: u32 = 1;
pub const QS_ALLINPUT: u32 = 0x04FF;
pub const MWMO_INPUTAVAILABLE: u32 = 0x0004;
pub const WAIT_OBJECT_0: u32 = 0;
pub const INFINITE: u32 = 0xFFFF_FFFF;
/// Windows 10 1803+: a waitable timer with ~0.5 ms precision (normal timers tick every 15.6 ms).
pub const CREATE_WAITABLE_TIMER_HIGH_RESOLUTION: u32 = 0x0000_0002;
pub const TIMER_ALL_ACCESS: u32 = 0x001F_0003;

pub const NORMAL_PRIORITY_CLASS: u32 = 0x0000_0020;
pub const IDLE_PRIORITY_CLASS: u32 = 0x0000_0040;
/// PROCESS_INFORMATION_CLASS::ProcessPowerThrottling
pub const PROCESS_POWER_THROTTLING: i32 = 4;
// WinEvents (follow the focused window's monitor, hide while the cursor shows)
pub const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
pub const EVENT_OBJECT_SHOW: u32 = 0x8002;
pub const EVENT_OBJECT_HIDE: u32 = 0x8003;
pub const OBJID_CURSOR: i32 = -9;
pub const WINEVENT_OUTOFCONTEXT: u32 = 0x0000;
pub const WINEVENT_SKIPOWNPROCESS: u32 = 0x0002;
pub const MONITOR_DEFAULTTONEAREST: u32 = 2;
pub const CURSOR_SHOWING: u32 = 1;
pub const PROCESS_POWER_THROTTLING_CURRENT_VERSION: u32 = 1;
pub const PROCESS_POWER_THROTTLING_EXECUTION_SPEED: u32 = 0x1;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 32-bit top-down DIB header.
pub fn bmi(w: i32, h: i32) -> BITMAPINFO {
    BITMAPINFO {
        h: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        colors: [0; 3],
    }
}

// Windows version for the log
#[repr(C)]
pub struct OSVERSIONINFOW {
    pub size: u32,
    pub major: u32,
    pub minor: u32,
    pub build: u32,
    pub platform: u32,
    pub csd: [u16; 128],
}
#[cfg_attr(windows, link(name = "ntdll", kind = "raw-dylib"))]
extern "system" {
    pub fn RtlGetVersion(v: *mut OSVERSIONINFOW) -> i32;
}

// Smart outline (#36): read a tiny screen region; zoom (#15): Magnification API
#[cfg_attr(windows, link(name = "user32", kind = "raw-dylib"))]
extern "system" {
    pub fn GetDC(h: isize) -> isize;
    pub fn ReleaseDC(h: isize, dc: isize) -> i32;
    pub fn SetLayeredWindowAttributes(h: isize, key: u32, alpha: u8, flags: u32) -> i32;
    pub fn InvalidateRect(h: isize, r: *const RECT, erase: i32) -> i32;
}
#[cfg_attr(windows, link(name = "gdi32", kind = "raw-dylib"))]
extern "system" {
    pub fn BitBlt(dst: isize, x: i32, y: i32, w: i32, h: i32, src: isize, sx: i32, sy: i32, rop: u32) -> i32;
}
#[repr(C)]
pub struct MAGTRANSFORM {
    pub v: [f32; 9],
}
// The Magnification API is NOT linked: magnification.dll pulls in Direct3D 9 + the graphics driver (~90 MB on AMD)
// for the rest of the process' life. Only the zoom helper (`aimdot --zoom-host`, win/zoom_host.rs) loads it, with
// LoadLibraryW + GetProcAddress, so aimdot.exe's import table doesn't name it.
pub type MagInitializeFn = unsafe extern "system" fn() -> i32;
pub type MagSetWindowSourceFn = unsafe extern "system" fn(h: isize, r: RECT) -> i32;
pub type MagSetWindowTransformFn = unsafe extern "system" fn(h: isize, t: *const MAGTRANSFORM) -> i32;
pub type MagSetWindowFilterListFn = unsafe extern "system" fn(h: isize, mode: u32, count: i32, list: *const isize) -> i32;
/// dwmapi's DwmFlush (HRESULT): waits for the next screen refresh. Also loaded by the zoom helper only.
pub type DwmFlushFn = unsafe extern "system" fn() -> i32;
#[cfg_attr(windows, link(name = "kernel32", kind = "raw-dylib"))]
extern "system" {
    pub fn LoadLibraryW(name: *const u16) -> isize;
    pub fn GetProcAddress(module: isize, name: *const u8) -> *const c_void;
    pub fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
}
#[cfg_attr(windows, link(name = "user32", kind = "raw-dylib"))]
extern "system" {
    pub fn GetWindowThreadProcessId(h: isize, pid: *mut u32) -> u32;
    pub fn IsWindow(h: isize) -> i32;
    pub fn UpdateWindow(h: isize) -> i32;
}
pub const SYNCHRONIZE: u32 = 0x0010_0000;
/// CreateWindowExW parent for a message-only window (no z-order, never shown, but PostMessage reaches it).
pub const HWND_MESSAGE: isize = -3;
pub const SRCCOPY: u32 = 0x00CC_0020;
pub const LWA_ALPHA: u32 = 2;
pub const WS_CHILD: u32 = 0x4000_0000;
pub const WS_VISIBLE: u32 = 0x1000_0000;
pub const MW_FILTERMODE_EXCLUDE: u32 = 0;

// Zoom lens shape
#[cfg_attr(windows, link(name = "user32", kind = "raw-dylib"))]
extern "system" {
    pub fn SetWindowRgn(h: isize, rgn: isize, redraw: i32) -> i32;
}
#[cfg_attr(windows, link(name = "gdi32", kind = "raw-dylib"))]
extern "system" {
    pub fn CreateEllipticRgn(l: i32, t: i32, r: i32, b: i32) -> isize;
}
pub const SWP_NOZORDER: u32 = 0x4;
