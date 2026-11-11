//! Where things live. `CROSSHAIR_HOME=<dir>` puts config + socket into one test folder (test copies never
//! touch the real settings).

use std::path::PathBuf;

fn test_home() -> Option<PathBuf> {
    std::env::var_os("CROSSHAIR_HOME").filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Settings + image library: `%APPDATA%\Aimdot` / `$XDG_CONFIG_HOME/aimdot` (`~/.config/aimdot`).
pub fn config_dir() -> PathBuf {
    if let Some(h) = test_home() {
        return h;
    }
    #[cfg(windows)]
    {
        let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        base.join("Aimdot")
    }
    #[cfg(not(windows))]
    {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("aimdot")
    }
}

/// Linux: the overlay's Unix socket (`$XDG_RUNTIME_DIR/aimdot.sock`).
#[cfg(not(windows))]
pub fn socket_path() -> PathBuf {
    if let Some(h) = test_home() {
        return h.join("aimdot.sock");
    }
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d).join("aimdot.sock"),
        None => config_dir().join("aimdot.sock"),
    }
}

/// Windows: file with "<port> <token>" for the overlay's loopback socket (`%LOCALAPPDATA%\Aimdot\ipc`).
#[cfg(windows)]
pub fn port_file() -> PathBuf {
    if let Some(h) = test_home() {
        return h.join("ipc");
    }
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(config_dir);
    base.join("Aimdot").join("ipc")
}

/// The other program, next to this one (`aimdot` <-> `aimdot-settings`).
pub fn sibling_exe(name: &str) -> PathBuf {
    let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(&file))).unwrap_or_else(|| PathBuf::from(file))
}
