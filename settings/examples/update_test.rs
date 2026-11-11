//! Test bench for src/update.rs + ui/update.slint, without the real settings window (main.rs isn't needed):
//!   CROSSHAIR_HOME=<scratch> AIMDOT_TEST_RELEASE=1 AIMDOT_UPDATE_URL=http://127.0.0.1:<port> \
//!     cargo run -p crosshair-settings --example update_test -- --check-update     (check only, writes update.ini)
//!   ... -- --update        (check + install with no window: the Linux AppImage path when APPIMAGE is set)
//!   ... -- --window        (the banner + card in a small window, for looking at them)
//! Never against the real settings folder (always set CROSSHAIR_HOME).
#[path = "../src/update.rs"]
mod update;

slint::slint! {
    import { Upd, UpdateBanner, UpdateCard } from "ui/update.slint";
    export { Upd }
    export component W inherits Window {
        width: 560px;
        height: 260px;
        background: #0d0d10;
        VerticalLayout { padding: 16px; spacing: 12px; UpdateBanner { } UpdateCard { } }
    }
}

fn main() {
    if std::env::var_os("CROSSHAIR_HOME").is_none() {
        eprintln!("set CROSSHAIR_HOME to a scratch folder");
        std::process::exit(2);
    }
    if update::cli() {
        println!("{}", std::fs::read_to_string(crosshair_core::update::state_path()).unwrap_or_default());
        return;
    }
    if std::env::args().any(|a| a == "--window") {
        let ui = W::new().unwrap();
        let _u = update::wire!(ui);
        ui.run().unwrap();
        return;
    }
    // headless: print every view change; --update installs (exits the process on success)
    let sink: update::Sink = std::sync::Arc::new(|v: update::View| println!("view: {v:?}"));
    let _u = update::start(sink);
    std::thread::sleep(std::time::Duration::from_secs(20));
}
