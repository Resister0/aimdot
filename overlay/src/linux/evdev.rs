//! Hide while aiming on Linux: reads the aim button (right by default; middle, mouse 4 / 5 or a keyboard key -
//! "Aim with" in the settings) straight from the mouse / keyboard devices (`/dev/input`).
//! Opt-in only, and it only works when the user is in the `input` group (the devices are not readable
//! otherwise) - it is never required and nothing is changed for the user. Wayland gives other apps' mouse
//! buttons to nobody, so this is the only way there. Read-only: the game still gets every click.
//! One small thread per device, blocked in read(). The kernel filters for us (EVIOCSMASK, Linux 4.4+): only that
//! button's / key's events reach the thread, so mouse movement (1000-8000 reports a second on a gaming mouse) and
//! other keys wake nothing. Keys are never logged. Keyboards are opened only when the aim button is a key.
//! Nothing is ever written to the device and it is never grabbed: the mouse's DPI / polling rate can't change.

use crate::ctl::Ev;
use crate::ipc_server::Send;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, AtomicU32, Ordering};
use std::sync::Mutex;

const EV_KEY: u16 = 1;
const BTN_RIGHT: u16 = 0x111;
/// struct input_event on 64-bit Linux: timeval (16) + type (2) + code (2) + value (4)
const EV_SIZE: usize = 24;
const EV_CNT: usize = 0x20;
const KEY_CNT: usize = 0x300;

/// struct input_mask (linux/input.h)
#[repr(C)]
struct InputMask {
    ty: u32,
    codes_size: u32,
    codes_ptr: u64,
}
/// _IOW('E', 0x93, struct input_mask)
const EVIOCSMASK: libc::c_ulong = (1 << 30) | ((std::mem::size_of::<InputMask>() as libc::c_ulong) << 16) | ((b'E' as libc::c_ulong) << 8) | 0x93;

/// Lets only EV_KEY / `code` through for this reader (EV_SYN can't be filtered, but the kernel drops a SYN_REPORT
/// with nothing before it, so a movement-only report doesn't wake us). Old kernel: unfiltered, still works.
/// Called again on an open device when the aim button changes in the settings.
fn only_code(fd: i32, code: u16) -> bool {
    let mut types = [0u64; EV_CNT.div_ceil(64)];
    types[EV_KEY as usize / 64] |= 1 << (EV_KEY as usize % 64);
    let mut keys = [0u64; KEY_CNT.div_ceil(64)];
    if (code as usize) < KEY_CNT {
        keys[code as usize / 64] |= 1 << (code as usize % 64);
    }
    let set = |ty: u32, bits: &[u64]| {
        let m = InputMask { ty, codes_size: std::mem::size_of_val(bits) as u32, codes_ptr: bits.as_ptr() as u64 };
        unsafe { libc::ioctl(fd, EVIOCSMASK as _, &m) == 0 }
    };
    // Codes first, then the type mask (until the type mask is set, nothing is filtered).
    set(EV_KEY as u32, &keys) && set(0, &types)
}

/// The evdev code for "Aim with" (settings aim_button / aim_vk): right / middle / mouse 4 (back) / mouse 5
/// (forward) / a keyboard key. None = a key that has no evdev code.
pub fn aim_code(button: u32, vk: u32) -> Option<u16> {
    match button {
        1 => Some(0x112), // BTN_MIDDLE
        2 => Some(0x113), // BTN_SIDE (mouse 4, "back")
        3 => Some(0x114), // BTN_EXTRA (mouse 5, "forward")
        4 => super::keys::key_code(vk),
        _ => Some(BTN_RIGHT),
    }
}

/// Mouse buttons are read from mice, keys from keyboards (by-id / by-path names `-event-mouse` / `-event-kbd`).
fn devices(kind: &str) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for dir in ["/dev/input/by-id", "/dev/input/by-path"] {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n.ends_with(kind) {
                    if let Ok(real) = std::fs::canonicalize(e.path()) {
                        if !v.contains(&real) {
                            v.push(real);
                        }
                    }
                }
            }
        }
    }
    v
}

/// The code the reader threads look for (changes take effect at once: the threads compare against it).
static CODE: AtomicU16 = AtomicU16::new(BTN_RIGHT);
/// Devices with a reader thread: (path, raw fd for re-masking).
static OPEN: Mutex<Vec<(PathBuf, i32)>> = Mutex::new(Vec::new());
/// Which device has the button down (bit per thread); any one counts.
static HELD: AtomicU32 = AtomicU32::new(0);
static NEXT: AtomicU32 = AtomicU32::new(0);

/// Starts (or re-points) the reader threads for `code` (evdev::aim_code). Returns the status for the settings
/// window: "ok", "no-permission" (devices not readable) or "none" (no mouse / keyboard found).
pub fn start(send: Send, code: u16) -> &'static str {
    use std::os::fd::AsRawFd;
    let old = CODE.swap(code, Ordering::Relaxed);
    let mut open = OPEN.lock().unwrap();
    if old != code {
        // Re-filter the devices already being read; a button held before the change doesn't count any more.
        for (_, fd) in open.iter() {
            only_code(*fd, code);
        }
        if HELD.swap(0, Ordering::Relaxed) != 0 {
            send(Ev::Aim(false));
        }
    }
    let kind = if code >= 0x110 { "-event-mouse" } else { "-event-kbd" };
    let want: Vec<PathBuf> = devices(kind);
    let mut readable = want.iter().filter(|d| open.iter().any(|(p, _)| p == *d)).count();
    let new: Vec<PathBuf> = want.iter().filter(|d| !open.iter().any(|(p, _)| p == *d)).cloned().collect();
    for d in &new {
        let Ok(mut f) = std::fs::File::open(d) else { continue };
        if !only_code(f.as_raw_fd(), code) {
            eprintln!("aimdot: {}: no kernel event filter (old kernel?) - every event wakes the reader", d.display());
        }
        readable += 1;
        let fd = f.as_raw_fd();
        open.push((d.clone(), fd));
        let send = send.clone();
        let bit = 1u32 << (NEXT.fetch_add(1, Ordering::Relaxed) % 32);
        let _ = std::thread::Builder::new().name("evdev".into()).stack_size(64 * 1024).spawn(move || {
            let mut buf = [0u8; EV_SIZE * 64];
            while let Ok(n) = f.read(&mut buf) {
                if n == 0 {
                    break;
                }
                for ev in buf[..n - n % EV_SIZE].chunks_exact(EV_SIZE) {
                    let ty = u16::from_ne_bytes([ev[16], ev[17]]);
                    let c = u16::from_ne_bytes([ev[18], ev[19]]);
                    let val = i32::from_ne_bytes([ev[20], ev[21], ev[22], ev[23]]);
                    if ty == EV_KEY && c == CODE.load(Ordering::Relaxed) && val != 2 {
                        // Any device holding the button counts (two mice plugged in).
                        let before = HELD.load(Ordering::Relaxed);
                        let now = if val == 1 { before | bit } else { before & !bit };
                        HELD.store(now, Ordering::Relaxed);
                        if (before != 0) != (now != 0) {
                            send(Ev::Aim(now != 0));
                        }
                    }
                }
            }
            // Unplugged: forget it (its fd number may be reused), let a press it held go.
            OPEN.lock().unwrap().retain(|(_, f)| *f != fd);
            let before = HELD.fetch_and(!bit, Ordering::Relaxed);
            if before & bit != 0 && before & !bit == 0 {
                send(Ev::Aim(false));
            }
        });
    }
    if want.is_empty() {
        "none"
    } else if readable == 0 {
        "no-permission"
    } else {
        "ok"
    }
}
