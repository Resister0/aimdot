//! Test bench: pictures of Settings > Appearance rendered off-screen (Slint's software renderer into a PNG) - no
//! window is ever opened, so it's safe while a game runs. Only the window's look: no overlay, no config.ini.
//!   CROSSHAIR_HOME=<scratch> cargo run -p crosshair-settings --example theme_shots -- <out dir> <w>x<h>[@scale] <theme id>...
//! Writes <out dir>/appearance-<w>-<theme>.png (theme coral = the default; CROSSHAIR_PAGE=n: another page). The real window draws with OpenGL (femtovg):
//! text may look a little different, sizes and layout are the same.
#![allow(dead_code)]
slint::include_modules!();
#[path = "../src/theme.rs"]
mod theme;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::ComponentHandle;
use std::rc::Rc;

struct Offscreen(Rc<MinimalSoftwareWindow>);
impl Platform for Offscreen {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn main() {
    if std::env::var_os("CROSSHAIR_HOME").is_none() {
        eprintln!("set CROSSHAIR_HOME to a scratch folder");
        std::process::exit(2);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(out), Some(size)) = (args.first(), args.get(1)) else {
        eprintln!("usage: theme_shots <out dir> <w>x<h>[@scale] <theme id>...");
        std::process::exit(2);
    };
    let (dims, scale) = size.split_once('@').map_or((size.as_str(), 1.0), |(d, s)| (d, s.parse().unwrap_or(1.0)));
    let (w, h) = dims.split_once('x').and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))).expect("size");
    let ids: Vec<&str> = if args.len() > 2 { args[2..].iter().map(|s| if s == "coral" { "" } else { s.as_str() }).collect() } else { vec![""] };

    let win = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Offscreen(win.clone()))).unwrap();
    win.dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: scale });
    std::fs::create_dir_all(out).unwrap();
    for id in ids {
        let ui = AppWindow::new().unwrap();
        let s = ui.global::<S>();
        s.set_is_windows(true);
        // Settings, scrolled to Appearance (CROSSHAIR_PAGE=n: another page, see S.page in ui/app.slint)
        let page = std::env::var("CROSSHAIR_PAGE").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
        s.set_page(page);
        s.set_version("test".into());
        // things drawn ON the accent, so the pictures show them: 2 switches on (Start) + the primary "Start" button (About)
        s.set_autostart(true);
        s.set_start_minimized(true);
        s.set_conn("offline".into());
        theme::apply(&ui, &theme::palette(id, "", ""));
        s.set_ui_theme(id.into());
        s.set_themes(theme::cards(id, "", ""));
        ui.window().set_size(slint::PhysicalSize::new(w, h));
        ui.show().unwrap();
        // let the start timer scroll to Appearance and the fades settle
        let mut buf = vec![slint::Rgb8Pixel::default(); (w * h) as usize];
        for _ in 0..40 {
            slint::platform::update_timers_and_animations();
            win.request_redraw();
            win.draw_if_needed(|r| {
                r.render(&mut buf, w as usize);
            });
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        let name = if id.is_empty() { "coral" } else { id };
        let what = if page == 3 { "appearance".to_string() } else { format!("page{page}") };
        let path = std::path::Path::new(out).join(format!("{what}-{w}-{name}.png"));
        let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(&path).unwrap()), w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let bytes: Vec<u8> = buf.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
        enc.write_header().unwrap().write_image_data(&bytes).unwrap();
        println!("{}", path.display());
        ui.hide().unwrap();
    }
}
