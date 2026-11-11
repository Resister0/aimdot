//! Aimdot overlay: the small program that runs for the whole game session (crosshair window, hotkeys, tray).
//! The settings window is a separate program (aimdot-settings) that is started only when needed.
//!
//! `aimdot`              start (or, if it already runs: open the settings window)
//! `aimdot --tray`       start without opening the settings window (the settings program starting the overlay)
//! `aimdot --autostart`  start with the PC: opens the settings window unless "Start minimised" is on
//! `aimdot --quit`       tell a running overlay to quit
#![cfg_attr(windows, windows_subsystem = "windows")]

mod ctl;
/// Per-game profiles: which program is in front (Windows WinEvent / Linux XWayland), only while a rule exists.
mod games;
mod ipc_server;

#[cfg(any(windows, feature = "wincheck"))]
#[cfg_attr(not(windows), allow(dead_code))]
mod win;

/// Game Bar mode: writes the crosshair into the Game Bar widget's LocalState folder (no Windows-only code inside)
#[cfg(any(windows, feature = "wincheck"))]
#[cfg_attr(not(windows), allow(dead_code))]
mod gamebar;

#[cfg(target_os = "linux")]
mod linux;

fn main() {
    #[cfg(target_os = "linux")]
    {
        // A crash line in aimdot.log (Windows has its own hook in win.rs).
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            crosshair_core::log::line("error", &format!("crash: {info}"));
            prev(info);
        }));
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--quit") {
        if !ipc_server::tell_running("quit") {
            eprintln!("aimdot: not running");
        }
        return;
    }
    // --autostart is a --tray start too (a second one never opens a window); whether the first one opens the
    // settings window is decided once the config is read (start_minimized).
    let autostart = args.iter().any(|a| a == "--autostart");
    let tray = autostart || args.iter().any(|a| a == "--tray");
    #[cfg(windows)]
    win::run(tray, autostart);
    #[cfg(target_os = "linux")]
    linux::run(tray, autostart);
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (tray, autostart);
        eprintln!("aimdot: this system isn't supported");
    }
}
