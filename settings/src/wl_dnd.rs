//! Images page, Linux Wayland: files dragged from Dolphin (or any Wayland app) onto the settings window are
//! imported (images.rs `dropped`). winit 0.30 has no Wayland drag and drop (only X11 / Windows send DroppedFile),
//! and Slint 1.18 is still on winit 0.30, so this listens itself: a wl_data_device on the SAME Wayland connection
//! as the window (the window handle gives the wl_display), on its own event queue + thread. Nothing else here
//! has a wl_data_device on that connection (winit: none; the clipboard = arboard over wlr-data-control on its own
//! connection). The same method as Echo's native app.
//! - only `text/uri-list` drags are accepted, and only as COPY (a file manager also offers move - a move would
//!   delete the file from where it was)
//! - only local files (file://); the page shows a note for anything else
use std::path::PathBuf;
use std::sync::Mutex;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle};

const URIS: &str = "text/uri-list";

/// the mime types one offer has
#[derive(Default)]
struct Mimes(Mutex<Vec<String>>);

impl Mimes {
    fn has(&self, m: &str) -> bool {
        self.0.lock().map(|v| v.iter().any(|x| x == m)).unwrap_or(false)
    }
}

struct St {
    conn: Connection,
    /// wl_data_device_manager version (3+ = set_actions / finish)
    ver: u32,
    /// the drag over the window now, if it has files
    cur: Option<WlDataOffer>,
}

thread_local! {
    static STARTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Called from the winit event hook (images.rs): the first time, on Wayland, starts the listener thread.
pub fn start(win: &slint::Window) {
    use slint::winit_030::winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
    use slint::winit_030::WinitWindowAccessor;
    if STARTED.with(|s| s.replace(true)) {
        return;
    }
    let display = win.with_winit_window(|w| match w.display_handle().map(|d| d.as_raw()) {
        Ok(RawDisplayHandle::Wayland(h)) => Some(h.display.as_ptr()),
        _ => None, // X11: winit's own drag and drop
    });
    let Some(Some(display)) = display else { return };
    // SAFETY: the wl_display is winit's, alive for the whole event loop (= the settings window's life). "Guest"
    // backend: it never closes the display.
    let backend = unsafe { wayland_backend::client::Backend::from_foreign_display(display as *mut _) };
    let conn = Connection::from_backend(backend);
    let r = std::thread::Builder::new().name("wl-dnd".into()).spawn(move || {
        if let Err(e) = run(conn) {
            crosshair_core::log::line("images", &format!("drag and drop off: {e}"));
        }
    });
    if let Err(e) = r {
        crosshair_core::log::line("images", &format!("drag and drop: no thread ({e})"));
    }
}

fn run(conn: Connection) -> Result<(), String> {
    let (globals, mut q) = registry_queue_init::<St>(&conn).map_err(|e| e.to_string())?;
    let qh = q.handle();
    let seat: WlSeat = globals.bind(&qh, 1..=1, ()).map_err(|e| format!("no seat: {e}"))?;
    let mgr: WlDataDeviceManager = globals.bind(&qh, 1..=3, ()).map_err(|e| format!("no data device manager: {e}"))?;
    let ver = mgr.version();
    let _dev = mgr.get_data_device(&seat, &qh, ());
    let mut st = St { conn, ver, cur: None };
    loop {
        q.blocking_dispatch(&mut st).map_err(|e| e.to_string())?;
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for St {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlSeat, ()> for St {
    fn event(_: &mut Self, _: &WlSeat, _: <WlSeat as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlDataDeviceManager, ()> for St {
    fn event(_: &mut Self, _: &WlDataDeviceManager, _: <WlDataDeviceManager as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlDataOffer, Mimes> for St {
    fn event(_: &mut Self, _: &WlDataOffer, ev: wl_data_offer::Event, m: &Mimes, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_data_offer::Event::Offer { mime_type } = ev {
            if let Ok(mut v) = m.0.lock() {
                v.push(mime_type);
            }
        }
    }
}

fn hover(on: bool) {
    let _ = slint::invoke_from_event_loop(move || super::drag_over(on));
}

impl Dispatch<WlDataDevice, ()> for St {
    fn event(st: &mut Self, _: &WlDataDevice, ev: wl_data_device::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use wl_data_device::Event as E;
        match ev {
            E::Enter { serial, id, .. } => {
                if let Some(old) = st.cur.take() {
                    old.destroy();
                }
                let Some(o) = id else { return };
                if !o.data::<Mimes>().is_some_and(|m| m.has(URIS)) {
                    o.accept(serial, None); // not files (text, a picture from a browser): no drop here
                    o.destroy();
                    return;
                }
                o.accept(serial, Some(URIS.into()));
                if st.ver >= 3 {
                    o.set_actions(DndAction::Copy, DndAction::Copy);
                }
                st.cur = Some(o);
                hover(true);
            }
            E::Leave => {
                if let Some(o) = st.cur.take() {
                    o.destroy();
                    hover(false);
                }
            }
            E::Drop => {
                let Some(o) = st.cur.take() else { return };
                let (conn, ver) = (st.conn.clone(), st.ver);
                // read the data off this thread: the dragging app writes it while this queue keeps running
                std::thread::spawn(move || {
                    let paths = read(&o, &conn, URIS).map(|b| parse_uris(&b)).unwrap_or_default();
                    if ver >= 3 {
                        o.finish();
                    }
                    o.destroy();
                    let _ = conn.flush();
                    super::dropped(paths);
                });
            }
            E::Selection { id: Some(o) } => o.destroy(), // the clipboard (read through Slint / arboard, not here)
            _ => {}
        }
    }

    event_created_child!(St, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, Mimes::default()),
    ]);
}

/// one mime type of the dropped data, read to the end
fn read(o: &WlDataOffer, conn: &Connection, mime: &str) -> Option<Vec<u8>> {
    use std::io::Read;
    let (mut r, w) = std::io::pipe().ok()?;
    o.receive(mime.into(), std::os::fd::AsFd::as_fd(&w));
    conn.flush().ok()?;
    drop(w); // only the dragging app holds the write end now: EOF when it's done
    let mut buf = vec![];
    r.read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// text/uri-list: one URI per line, "#" = comment; only file:// (local files)
fn parse_uris(b: &[u8]) -> Vec<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    String::from_utf8_lossy(b)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("file://"))
        // file:///home/… or file://host/home/…
        .map(|rest| if rest.starts_with('/') { rest } else { rest.find('/').map_or("", |i| &rest[i..]) })
        .filter(|p| !p.is_empty())
        .map(|p| PathBuf::from(std::ffi::OsString::from_vec(unescape(p))))
        .collect()
}

/// %20 -> the byte (paths may not be UTF-8)
fn unescape(s: &str) -> Vec<u8> {
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uris() {
        let v = parse_uris(b"# c\r\nfile:///data/My%20Cross.svg\r\nfile://host/x/a%25b.png\r\nhttps://x/y.png\r\nfile:///x%2\r\n");
        assert_eq!(v, vec![PathBuf::from("/data/My Cross.svg"), PathBuf::from("/x/a%b.png"), PathBuf::from("/x%2")]);
    }
}
