//! Messages between the overlay (server) and the settings window (client).
//!
//! A message is `u32 little-endian length` + UTF-8 text `kind\nbody`. Kinds:
//! - client -> overlay: `hello` (body = token), `state` (body = whole config as INI text),
//!   `capture` (body `1`/`0`: hotkey capture in progress - the overlay lets go of its hotkeys), `quit`.
//! - overlay -> client: `state` (on connect, and when a hotkey or the tray changed something), `info` (key=value
//!   lines: platform, monitors, hk_ok, aim, gamebar).
//!
//! Transport: Linux = Unix socket in `$XDG_RUNTIME_DIR` (only this user can open it).
//! Windows = TCP on 127.0.0.1 with a random token (the port + token sit in a file in the user's own
//! %LOCALAPPDATA% folder), because a named pipe opened for normal (blocking) I/O can't read and write at the
//! same time from two threads.

use std::io::{self, Read, Write};

pub const MAX_MSG: usize = 4 << 20;

pub fn write_msg(w: &mut impl Write, kind: &str, body: &str) -> io::Result<()> {
    let len = kind.len() + 1 + body.len();
    let mut buf = Vec::with_capacity(4 + len);
    buf.extend_from_slice(&(len as u32).to_le_bytes());
    buf.extend_from_slice(kind.as_bytes());
    buf.push(b'\n');
    buf.extend_from_slice(body.as_bytes());
    w.write_all(&buf)?;
    w.flush()
}

pub fn read_msg(r: &mut impl Read) -> io::Result<(String, String)> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_MSG {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too big"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    let text = String::from_utf8(buf).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not UTF-8"))?;
    Ok(match text.split_once('\n') {
        Some((k, b)) => (k.to_string(), b.to_string()),
        None => (text, String::new()),
    })
}

/// `key=value` lines (the `info` message).
pub fn kv(body: &str) -> impl Iterator<Item = (&str, &str)> {
    body.lines().filter_map(|l| l.split_once('='))
}

/// A random-enough token without crates: time + process id + stack address, mixed.
pub fn token() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0) as u64;
    let local = 0u8;
    let mut h = t ^ (std::process::id() as u64).rotate_left(32) ^ (&local as *const u8 as u64);
    let mut out = String::new();
    for _ in 0..4 {
        h ^= h << 13;
        h ^= h >> 7;
        h ^= h << 17;
        out.push_str(&format!("{h:016x}"));
    }
    out
}
