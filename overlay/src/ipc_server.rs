//! The overlay's local socket for the settings window (see core/src/ipc.rs for the protocol).
//! One accept thread + one reader thread per connection; everything is handed to the main loop via `send`.

use crate::ctl::Ev;
use crosshair_core::{ipc, paths};
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub type Send = Arc<dyn Fn(Ev) + std::marker::Send + Sync>;

static NEXT: AtomicU64 = AtomicU64::new(1);

fn serve<S: Read + Write + std::marker::Send + 'static>(stream: S, writer: Box<dyn Write + std::marker::Send>, send: &Send) {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    send(Ev::Connected(id, writer));
    let send = send.clone();
    let _ = std::thread::Builder::new().name("ipc-client".into()).stack_size(64 * 1024).spawn(move || {
        let mut r = io::BufReader::new(stream);
        while let Ok((k, b)) = ipc::read_msg(&mut r) {
            send(Ev::Msg(id, k, b));
        }
        send(Ev::Gone(id));
    });
}

pub enum Start {
    /// The token a client must send first (Windows), None on Linux.
    #[cfg_attr(not(windows), allow(dead_code))]
    Listening(Option<String>),
    /// Another overlay is already running (this is its socket). Linux only: Windows uses a mutex.
    #[cfg_attr(windows, allow(dead_code))]
    Running,
}

#[cfg(not(windows))]
pub fn start(send: Send) -> io::Result<Start> {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    let path = paths::socket_path();
    if UnixStream::connect(&path).is_ok() {
        return Ok(Start::Running);
    }
    let _ = std::fs::remove_file(&path);
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let l = UnixListener::bind(&path)?;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    std::thread::Builder::new().name("ipc-accept".into()).stack_size(64 * 1024).spawn(move || {
        for s in l.incoming().flatten() {
            if let Ok(w) = s.try_clone() {
                serve(s, Box::new(w), &send);
            }
        }
    })?;
    Ok(Start::Listening(None))
}

#[cfg(windows)]
pub fn start(send: Send) -> io::Result<Start> {
    use std::net::TcpListener;
    let l = TcpListener::bind("127.0.0.1:0")?;
    let port = l.local_addr()?.port();
    let token = ipc::token();
    let file = paths::port_file();
    if let Some(d) = file.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    std::fs::write(&file, format!("{port} {token}"))?;
    std::thread::Builder::new().name("ipc-accept".into()).stack_size(64 * 1024).spawn(move || {
        for s in l.incoming().flatten() {
            let _ = s.set_nodelay(true);
            if let Ok(w) = s.try_clone() {
                serve(s, Box::new(w), &send);
            }
        }
    })?;
    Ok(Start::Listening(Some(token)))
}

/// A second `aimdot` start talks to the running one (e.g. "open" = show the settings window).
pub fn tell_running(kind: &str) -> bool {
    #[cfg(not(windows))]
    let s = std::os::unix::net::UnixStream::connect(paths::socket_path()).ok();
    #[cfg(windows)]
    let s = (|| {
        let text = std::fs::read_to_string(paths::port_file()).ok()?;
        let (port, token) = text.trim().split_once(' ')?;
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port.parse::<u16>().ok()?)).ok()?;
        ipc::write_msg(&mut s, "hello", token).ok()?;
        Some(s)
    })();
    match s {
        Some(mut s) => ipc::write_msg(&mut s, kind, "").is_ok(),
        None => false,
    }
}
