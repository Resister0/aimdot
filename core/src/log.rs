//! `aimdot.log`: one small text file in the config folder (`~/.config/aimdot`, `%APPDATA%\Aimdot`), like Echo's
//! `proto.log`, so a problem on someone's PC (above all in Game Bar mode, where nothing can be seen from outside)
//! can be read afterwards. One dated line per event:
//!
//! ```text
//! 2026-11-10 14:03:07Z [start] Aimdot 0.1.0 overlay - Windows 10.0.26100, Game Bar widget installed
//! 2026-11-10 14:03:09Z [gamebar] wrote crosshair.png 4.1 KB + crosshair.ini (seq 1731247389001) to Packages\Resister0.AimdotWidget_x\LocalState
//! 2026-11-10 14:03:10Z [widget/ini] seq 1731247389001 visible 1 41x41 rainbow 0
//! ```
//!
//! - Times are UTC (the `Z`): core has no clock-zone code and the widget writes UTC too, so both line up.
//! - Events only - never per frame. Things that can repeat every tick go through [`Quiet`].
//! - No home folder or user name: every line goes through [`scrub`] (`~` for the home folder).
//! - Over [`MAX_BYTES`] the file is renamed to `aimdot.log.1` (the old `.1` goes) and a new one starts.
//! - Two programs may write (overlay + settings): each line is one `write` in append mode, which both OSes keep whole.
//!
//! Only std. A write error is ignored: the log must never break the app.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// The file name in the config folder.
pub const FILE: &str = "aimdot.log";
/// Rotate at this size (current + `.1` = at most twice this on disk).
pub const MAX_BYTES: u64 = 256 * 1024;
/// The Game Bar widget's own log, in its package's `LocalState` folder (`gamebar-widget/WidgetLog.cs`).
pub const WIDGET_FILE: &str = "widget.log";

struct State {
    path: Option<PathBuf>,
    off: bool,
}

static STATE: Mutex<State> = Mutex::new(State { path: None, off: false });

/// Where the log is: `<config folder>/aimdot.log` (`CROSSHAIR_HOME` test copies log into their own folder).
pub fn path() -> PathBuf {
    let st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    st.path.clone().unwrap_or_else(|| crate::paths::config_dir().join(FILE))
}

/// The folder the log is in (for "Open log folder").
pub fn folder() -> PathBuf {
    path().parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

/// Log somewhere else (tests), or `None` = back to the normal place.
pub fn set_path(p: Option<PathBuf>) {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).path = p;
}

/// Turn writing off / on (e.g. a `--no-log` switch). Reading still works.
pub fn set_enabled(on: bool) {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).off = !on;
}

/// The first line of a run: `[start] Aimdot <version> <program> - <os>`. `program` = "overlay" / "settings";
/// `os` = what the platform layer knows best ("Windows 10.0.26100", "Linux, Wayland (KWin)"); empty = core's guess.
pub fn start(program: &str, os: &str) {
    let os = if os.is_empty() { os_guess() } else { os.to_string() };
    line("start", &format!("{} {} {program} - {os}", crate::APP_NAME, crate::VERSION));
}

/// Last line of a run (`[stop] <why>`).
pub fn stop(why: &str) {
    line("stop", why);
}

/// One line: `<date> <time>Z [area] message`. Areas are short lower-case words: start, stop, window, aim, hotkey,
/// profile, mode, gamebar, settings, error.
pub fn line(area: &str, msg: &str) {
    raw(&format_line(now_secs(), area, msg));
}

/// `line` with `format!` arguments: `aimdot_log!("gamebar", "wrote {} bytes", n)`.
#[macro_export]
macro_rules! aimdot_log {
    ($area:expr, $($arg:tt)*) => {
        $crate::log::line($area, &format!($($arg)*))
    };
}

/// A ready line (already dated), scrubbed and appended.
fn raw(text: &str) {
    let (path, off) = {
        let st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        (st.path.clone(), st.off)
    };
    if off {
        return;
    }
    let path = path.unwrap_or_else(|| crate::paths::config_dir().join(FILE));
    let _ = append(&path, &scrub(text));
}

fn append(path: &Path, text: &str) -> std::io::Result<()> {
    if fs::metadata(path).map(|m| m.len() >= MAX_BYTES).unwrap_or(false) {
        let _ = fs::rename(path, rotated(path));
    }
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let mut buf = String::with_capacity(text.len() + 2);
    // One line per entry, whatever the message had in it.
    for ch in text.chars() {
        buf.push(if ch == '\n' || ch == '\r' { ' ' } else { ch });
    }
    buf.push_str(if cfg!(windows) { "\r\n" } else { "\n" });
    OpenOptions::new().create(true).append(true).open(path)?.write_all(buf.as_bytes())
}

/// `aimdot.log` -> `aimdot.log.1`.
pub fn rotated(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".1");
    PathBuf::from(s)
}

fn format_line(secs: u64, area: &str, msg: &str) -> String {
    format!("{} [{area}] {msg}", stamp(secs))
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `2026-11-10 14:03:07Z` (UTC).
pub fn stamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

// ---- Privacy -----------------------------------------------------------------------------------------------------

/// No real names in the log: the home folder becomes `~` (any case on Windows, `\` or `/`), and a user-name folder
/// that is left (`\Users\<name>\` in another spelling, `/home/<name>/`) becomes `<user>`.
pub fn scrub(text: &str) -> String {
    let home = std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_default();
    let user = std::env::var(if cfg!(windows) { "USERNAME" } else { "USER" }).unwrap_or_default();
    scrub_with(text, &home, &user)
}

fn scrub_with(text: &str, home: &str, user: &str) -> String {
    let mut out = text.to_string();
    let home = home.trim_end_matches(['/', '\\']);
    if home.len() > 3 {
        for h in [home.to_string(), home.replace('\\', "/"), home.replace('/', "\\")] {
            out = replace_ci_bounded(&out, &h, "~");
        }
    }
    if user.len() >= 2 {
        for sep in ['\\', '/'] {
            for parent in ["Users", "home"] {
                let pat = format!("{sep}{parent}{sep}{user}");
                out = replace_ci_bounded(&out, &pat, &format!("{sep}{parent}{sep}<user>"));
            }
            // Windows' short (8.3) form of the folder: first 6 letters + `~1`.
            if let Some(short) = user.get(..user.len().min(6)) {
                out = replace_ci(&out, &format!("{sep}Users{sep}{short}~"), &format!("{sep}Users{sep}<user>~"));
            }
        }
    }
    out
}

/// ASCII-case-insensitive replace (Windows paths are; on Linux a home path in another case is no real risk).
fn replace_ci(text: &str, pat: &str, with: &str) -> String {
    replace_where(text, pat, with, |_, _| true)
}

/// Like `replace_ci`, but only where the match ends at a path end (separator, quote, space, end of text).
fn replace_ci_bounded(text: &str, pat: &str, with: &str) -> String {
    replace_where(text, pat, with, |t, end| t[end..].chars().next().map_or(true, |c| !c.is_alphanumeric() && c != '_' && c != '.' && c != '-' && c != '~'))
}

fn replace_where(text: &str, pat: &str, with: &str, ok: impl Fn(&str, usize) -> bool) -> String {
    if pat.is_empty() || text.len() < pat.len() {
        return text.to_string();
    }
    let (lt, lp) = (text.to_ascii_lowercase(), pat.to_ascii_lowercase());
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(k) = lt[i..].find(&lp) {
        let (s, e) = (i + k, i + k + lp.len());
        // ASCII lower-casing keeps byte positions, so these are char boundaries in `text` too.
        if ok(text, e) {
            out.push_str(&text[i..s]);
            out.push_str(with);
        } else {
            out.push_str(&text[i..e]);
        }
        i = e;
    }
    out.push_str(&text[i..]);
    out
}

// ---- Things that would repeat ------------------------------------------------------------------------------------

/// For events that can come every tick (e.g. "can't write to the widget folder" each second): logs a message only
/// when it differs from the last one, and then says how often the old one came again.
#[derive(Default)]
pub struct Quiet {
    last: String,
    again: u32,
}

impl Quiet {
    pub fn new() -> Quiet {
        Quiet::default()
    }

    /// Logs `msg` unless it's the same as last time.
    pub fn line(&mut self, area: &str, msg: &str) {
        if msg == self.last {
            self.again = self.again.saturating_add(1);
            return;
        }
        if self.again > 0 {
            line(area, &format!("(the line before came {} more times)", self.again));
        }
        self.last = msg.to_string();
        self.again = 0;
        line(area, msg);
    }

    /// Forget the last message (e.g. the problem went away), so the same one is logged again if it comes back.
    pub fn reset(&mut self) {
        self.last.clear();
        self.again = 0;
    }
}

/// For events that can come in bursts (a slider drag makes a Game Bar write per change): at most one line per
/// `gap`. Lines in between are held back; the newest one is written by the next `line` / `tick` after the gap,
/// with how many were left out - so the last state always reaches the log. Call `tick` from a 1 s timer.
pub struct Throttle {
    gap: std::time::Duration,
    at: Option<std::time::Instant>,
    held: Option<(String, String)>,
    skipped: u32,
}

impl Throttle {
    pub fn new(gap: std::time::Duration) -> Throttle {
        Throttle { gap, at: None, held: None, skipped: 0 }
    }

    pub fn line(&mut self, area: &str, msg: &str) {
        self.held = Some((area.to_string(), msg.to_string()));
        self.skipped += 1;
        self.tick();
    }

    /// Writes the held line once the gap is over.
    pub fn tick(&mut self) {
        let now = std::time::Instant::now();
        if self.at.is_some_and(|t| now.duration_since(t) < self.gap) {
            return;
        }
        if let Some((area, msg)) = self.held.take() {
            match self.skipped {
                0 | 1 => line(&area, &msg),
                n => line(&area, &format!("{msg} (+{} like it before, left out)", n - 1)),
            }
            self.skipped = 0;
            self.at = Some(now);
        }
    }
}

// ---- Copying another log in (the Game Bar widget's) ------------------------------------------------------------

/// Follows a log file someone else writes (the widget's `LocalState\widget.log`) and hands over the lines that
/// are new since last time. Read-only; the file may be rotated or recreated at any time.
#[derive(Default)]
pub struct Follow {
    /// Byte position read up to (always just after a line end). `None` = never read yet.
    pos: Option<u64>,
}

impl Follow {
    pub fn new() -> Follow {
        Follow::default()
    }

    /// Start over (e.g. Game Bar mode was switched on again): the next `read` gives the last `first` lines again.
    pub fn reset(&mut self) {
        self.pos = None;
    }

    /// The complete lines added since the last call. On the first call (or after the file got shorter, i.e. was
    /// rotated): only its last `first` lines. Reads at most 64 KB per call. Missing file = nothing.
    pub fn read(&mut self, path: &Path, first: usize) -> Vec<String> {
        const MAX: u64 = 64 * 1024;
        let Ok(mut f) = File::open(path) else { return Vec::new() };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let fresh = match self.pos {
            Some(p) => p > len,
            None => true,
        };
        let from = if fresh { len.saturating_sub(MAX) } else { self.pos.unwrap_or(0) };
        if from >= len {
            self.pos = Some(len);
            return Vec::new();
        }
        let want = (len - from).min(MAX);
        let mut buf = Vec::with_capacity(want as usize);
        if f.seek(SeekFrom::Start(from)).is_err() || f.take(want).read_to_end(&mut buf).is_err() {
            return Vec::new();
        }
        // Only up to the last line end: a line being written right now waits for the next call.
        let Some(end) = buf.iter().rposition(|&b| b == b'\n') else {
            if fresh {
                self.pos = Some(from);
            }
            return Vec::new();
        };
        let mut text = &buf[..=end];
        // Started in the middle of the file: drop the partial first line.
        if fresh && from > 0 {
            match text.iter().position(|&b| b == b'\n') {
                Some(nl) => text = &text[nl + 1..],
                None => text = &[],
            }
        }
        self.pos = Some(from + end as u64 + 1);
        let mut lines: Vec<String> = String::from_utf8_lossy(text)
            .lines()
            .map(|l| l.trim_end_matches('\r').to_string())
            .filter(|l| !l.trim().is_empty())
            .collect();
        if fresh && lines.len() > first {
            lines.drain(..lines.len() - first);
        }
        lines
    }
}

/// Copies lines of another log into this one, keeping their own time: `<time> [area] msg` becomes
/// `<time> [<from>/area] msg`; a line without that shape gets this moment's time and `[<from>]`.
pub fn copy_in(from: &str, lines: &[String]) {
    for l in lines {
        raw(&relabel(from, l, now_secs()));
    }
}

fn relabel(from: &str, l: &str, now: u64) -> String {
    let l = l.trim();
    if let Some(open) = l.find(" [") {
        if let Some(close) = l[open + 2..].find("] ") {
            let (time, area, msg) = (&l[..open], &l[open + 2..open + 2 + close], &l[open + 2 + close + 2..]);
            if looks_like_stamp(time) && !area.contains(['[', ']']) {
                return format!("{time} [{from}/{area}] {msg}");
            }
        }
    }
    format_line(now, from, l)
}

fn looks_like_stamp(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 20 && b[4] == b'-' && b[7] == b'-' && b[10] == b' ' && b[19] == b'Z' && b[..4].iter().all(u8::is_ascii_digit)
}

// ---- Reading it back (settings: "Copy log") ----------------------------------------------------------------------

/// The end of the log for the clipboard: up to `max` bytes of whole lines from `aimdot.log.1` + `aimdot.log`,
/// already scrubbed (files written by an older version might not be).
pub fn tail_text(max: usize) -> String {
    tail_of(&path(), max)
}

fn tail_of(path: &Path, max: usize) -> String {
    let mut all = fs::read(rotated(path)).unwrap_or_default();
    all.extend(fs::read(path).unwrap_or_default());
    let mut start = all.len().saturating_sub(max);
    if start > 0 {
        start = all[start..].iter().position(|&b| b == b'\n').map(|p| start + p + 1).unwrap_or(all.len());
    }
    scrub(&String::from_utf8_lossy(&all[start..]))
}

// ---- What we're running on ---------------------------------------------------------------------------------------

/// Best guess without platform calls: "Linux (Nobara Linux 44), Wayland, KDE" / "Windows". The platform layers
/// pass something better to [`start`] where they know it.
pub fn os_guess() -> String {
    if cfg!(windows) {
        return "Windows".into();
    }
    let mut s = std::env::consts::OS.to_string();
    if let Some(c) = s.get_mut(..1) {
        c.make_ascii_uppercase();
    }
    if let Ok(t) = fs::read_to_string("/etc/os-release") {
        if let Some(name) = t.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")) {
            s = format!("{s} ({})", name.trim_matches('"'));
        }
    }
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    if var("WAYLAND_DISPLAY").is_some() {
        s.push_str(", Wayland");
    } else if var("DISPLAY").is_some() {
        s.push_str(", X11");
    }
    if let Some(d) = var("XDG_CURRENT_DESKTOP") {
        s.push_str(&format!(", {d}"));
    }
    s
}

/// A size for the log: "512 B", "4.1 KB", "1.2 MB".
pub fn size(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aimdot-log-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn stamps() {
        assert_eq!(stamp(0), "1970-01-01 00:00:00Z");
        assert_eq!(stamp(951_782_400), "2000-02-29 00:00:00Z");
        assert_eq!(stamp(1_794_312_187), "2026-11-10 12:03:07Z");
        assert!(looks_like_stamp(&stamp(1_794_312_187)));
    }

    #[test]
    fn scrubs_home_and_user() {
        // (home paths are split with concat! so the public repo's leak check doesn't see a real-looking one)
        let s = scrub_with(concat!(r"wrote C:\Users", r"\Kim\AppData\Local\Packages\x and c:/users/kim/a"), r"C:\Users\Kim", "Kim");
        assert_eq!(s, r"wrote ~\AppData\Local\Packages\x and ~/a");
        let s = scrub_with(r"short name C:\Users\KIM~1\x, other \Users\kim\y, \Users\kimberly\z", r"C:\Users\Kim", "kim");
        assert_eq!(s, r"short name C:\Users\<user>~1\x, other \Users\<user>\y, \Users\kimberly\z");
        assert_eq!(scrub_with(concat!("/home", "/kim/.config/aimdot ok"), "/home/kim", "kim"), "~/.config/aimdot ok");
        assert_eq!(scrub_with(concat!("/mnt/home", "/kim/x"), "/var/empty", "kim"), "/mnt/home/<user>/x");
        assert_eq!(scrub_with("nothing", "", ""), "nothing");
    }

    #[test]
    fn rotates() {
        let d = dir("rot");
        let p = d.join(FILE);
        let big = "x".repeat(1000);
        for _ in 0..(MAX_BYTES / 1000 + 5) {
            append(&p, &big).unwrap();
        }
        assert!(rotated(&p).is_file());
        assert!(fs::metadata(&p).unwrap().len() < MAX_BYTES);
        let t = tail_of(&p, 2500);
        assert!(t.len() <= 2500 && t.lines().all(|l| l == big), "whole lines only");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn follows_another_log() {
        let d = dir("follow");
        let p = d.join(WIDGET_FILE);
        let mut f = Follow::new();
        assert!(f.read(&p, 5).is_empty(), "missing file");
        fs::write(&p, "a\r\nb\nc\nd\npartial").unwrap();
        assert_eq!(f.read(&p, 2), ["c", "d"]);
        assert!(f.read(&p, 2).is_empty());
        OpenOptions::new().append(true).open(&p).unwrap().write_all(b" line\ne\n").unwrap();
        assert_eq!(f.read(&p, 2), ["partial line", "e"]);
        fs::write(&p, "new\n").unwrap(); // rotated: shorter than before
        assert_eq!(f.read(&p, 2), ["new"]);
        f.reset();
        assert_eq!(f.read(&p, 2), ["new"]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn relabels_copied_lines() {
        let t = stamp(1_794_312_187);
        assert_eq!(relabel("widget", &format!("{t} [ini] seq 5 visible 1"), 0), format!("{t} [widget/ini] seq 5 visible 1"));
        assert_eq!(relabel("widget", "odd line", 0), "1970-01-01 00:00:00Z [widget] odd line");
    }

    #[test]
    fn quiet_and_lines() {
        let d = dir("quiet");
        let p = d.join(FILE);
        // Only this test writes through the global path.
        set_path(Some(p.clone()));
        let mut q = Quiet::new();
        for _ in 0..4 {
            q.line("gamebar", "can't write");
        }
        q.line("gamebar", "wrote again");
        aimdot_log!("mode", "Game Bar mode {}", "on\nsecond");
        let mut t = Throttle::new(std::time::Duration::from_secs(3600));
        t.line("gamebar", "write 1");
        t.line("gamebar", "write 2");
        t.line("gamebar", "write 3");
        t.tick();
        t.gap = std::time::Duration::ZERO;
        t.tick();
        set_path(None);
        let text = fs::read_to_string(&p).unwrap();
        let lines: Vec<&str> = text.lines().map(|l| &l[21..]).collect();
        assert_eq!(
            lines,
            [
                "[gamebar] can't write",
                "[gamebar] (the line before came 3 more times)",
                "[gamebar] wrote again",
                "[mode] Game Bar mode on second",
                "[gamebar] write 1",
                "[gamebar] write 3 (+1 like it before, left out)"
            ]
        );
        let _ = fs::remove_dir_all(&d);
    }
}
