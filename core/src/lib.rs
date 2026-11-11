//! Aimdot core: the crosshair engine (coverage masks + re-tint), settings / profiles in the
//! original INI format, the PNG decoder, and the small message protocol between the overlay and the settings window.
//! No platform code and no crates, so both programs (and the tests) share exactly the same picture.
// Style lints only: `map_or`/manual div_ceil are kept so the code builds on older toolchains (as in the first version).
#![allow(clippy::too_many_arguments, clippy::manual_div_ceil, clippy::unnecessary_map_or, clippy::manual_repeat_n)]

pub mod config;
pub mod crosshair;
pub mod desktop_entry;
pub mod games;
pub mod gfx;
pub mod image;
pub mod import;
pub mod log;
pub mod ipc;
pub mod model;
pub mod paint;
pub mod paths;
pub mod presets_more;
pub mod share;
pub mod update;

#[cfg(test)]
mod tests;

/// Loads a crosshair image from the library. Only PNG: the settings window converts other formats on import.
pub fn load_png(p: &std::path::Path) -> Option<image::Image> {
    image::decode_png(&std::fs::read(p).ok()?).ok()
}

pub const APP_NAME: &str = "Aimdot";
/// The two programs (file names without .exe). Crate names stay crosshair-*: internal only.
pub const OVERLAY_EXE: &str = "aimdot";
pub const SETTINGS_EXE: &str = "aimdot-settings";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const ANTICHEAT_NOTE: &str =
    "Some games don't allow crosshair overlays in ranked - check before you play.";
