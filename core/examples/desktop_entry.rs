//! Runs the Linux app-menu entry check once and prints what it did (core/src/desktop_entry.rs).
//! Try it only with a scratch folder, e.g.:
//!   HOME=/tmp/de XDG_DATA_HOME=/tmp/de/share APPIMAGE=/tmp/de/Aimdot.AppImage cargo run -p crosshair-core --example desktop_entry
fn main() {
    println!("{:?}", crosshair_core::desktop_entry::ensure());
}
