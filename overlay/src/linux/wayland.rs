//! Wayland overlay: one small `zwlr_layer_shell_v1` surface on the overlay layer (KDE Plasma, Hyprland, Sway,
//! other wlroots desktops). Not anchored to any edge = the compositor centres it on the monitor; exclusive zone -1
//! = centred on the whole monitor, not the space next to panels. Empty input region = every click goes through.
//! The X/Y offset is done by making the surface bigger on one side, so nothing depends on knowing the monitor size.
//! Hidden (hotkey, aiming, Game Bar mode) = no surface at all: a mapped surface - even a fully transparent one -
//! is still a window on top of the game, so KWin has to composite every game frame instead of scanning the game
//! out directly. The layer surface is destroyed and made again when shown: a brand-new surface is the one path
//! where the protocol guarantees a configure (re-mapping an unmapped layer surface worked in the headless test
//! KWin but not on the real Plasma 6.7 session - the crosshair never came back). Showing = one configure round
//! trip (well under a millisecond on the local socket); KWin's fade / scale effects don't animate layer-shell
//! surfaces. Nothing is committed while nothing changes.

use crosshair_core::gfx::Canvas;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use wayland_client::globals::{registry_queue_init, GlobalList, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer::{self, WlBuffer},
    wl_compositor::WlCompositor,
    wl_output::{self, WlOutput},
    wl_region::WlRegion,
    wl_registry::{self, WlRegistry},
    wl_shm::{self, WlShm},
    wl_shm_pool::WlShmPool,
    wl_surface::{self, WlSurface},
};
use wayland_client::{delegate_noop, Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
    wp_fractional_scale_v1::{self, WpFractionalScaleV1},
};
use wayland_protocols::wp::viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

#[derive(Default)]
struct Output {
    reg: u32,
    wl: Option<WlOutput>,
    name: String,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    scale: i32,
}

struct Buf {
    id: u32,
    wl: WlBuffer,
    _pool: WlShmPool,
    _fd: OwnedFd,
    ptr: *mut u8,
    len: usize,
    w: i32,
    h: i32,
    busy: bool,
    /// What it shows: picture stamp + offset (so re-showing the same picture needs no copy).
    stamp: (u64, i32, i32),
}

impl Drop for Buf {
    fn drop(&mut self) {
        self.wl.destroy();
        unsafe { libc::munmap(self.ptr as *mut _, self.len) };
    }
}

struct Surf {
    wl: WlSurface,
    layer: ZwlrLayerSurfaceV1,
    /// The output it was made for; 0 = "follow the game" (no output given: KWin puts it on its active screen).
    out_reg: u32,
    /// The output it is on (wl_surface.enter) - for "follow the game", where we don't pick it.
    entered: Option<u32>,
    /// The first configure was acked: a buffer may be attached.
    configured: bool,
    frac: Option<WpFractionalScaleV1>,
    viewport: Option<WpViewport>,
    /// Preferred scale in 120ths (fractional-scale protocol), 0 = unknown.
    scale120: u32,
    size: (i32, i32),
    buffer_scale: i32,
}

pub struct St {
    compositor: WlCompositor,
    shm: WlShm,
    layer_shell: ZwlrLayerShellV1,
    viewporter: Option<WpViewporter>,
    frac_mgr: Option<WpFractionalScaleManagerV1>,
    outputs: Vec<Output>,
    surf: Option<Surf>,
    bufs: Vec<Buf>,
    next_buf: u32,
    stamp: u64,
    /// A present found no free buffer: draw again when the compositor gives one back.
    retry: bool,
    /// Show / hide "Instant" (mod.rs Display::set_keep_mapped): hidden = a transparent buffer, same surface.
    pub keep_mapped: bool,
    /// Last preferred scale (120ths) per output, so a surface made again starts at the right size.
    last_scale120: Vec<(u32, u32)>,
    /// Outputs or scale changed: the caller should redraw + resend the monitor list.
    pub changed: bool,
}

pub struct Wayland {
    pub conn: Connection,
    pub queue: EventQueue<St>,
    pub st: St,
    _globals: GlobalList,
}

impl Wayland {
    /// None = no Wayland session, or the compositor has no layer-shell (GNOME): use the X11 fallback.
    pub fn connect() -> Result<Wayland, String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("no Wayland session ({e})"))?;
        let (globals, mut queue) = registry_queue_init::<St>(&conn).map_err(|e| format!("Wayland registry: {e}"))?;
        let qh = queue.handle();
        let compositor: WlCompositor = globals.bind(&qh, 4..=6, ()).map_err(|e| format!("wl_compositor: {e}"))?;
        let shm: WlShm = globals.bind(&qh, 1..=1, ()).map_err(|e| format!("wl_shm: {e}"))?;
        let layer_shell: ZwlrLayerShellV1 =
            globals.bind(&qh, 1..=4, ()).map_err(|_| "this desktop has no layer-shell (GNOME?)".to_string())?;
        let viewporter = globals.bind(&qh, 1..=1, ()).ok();
        let frac_mgr = globals.bind(&qh, 1..=1, ()).ok();
        let mut st = St {
            compositor,
            shm,
            layer_shell,
            viewporter,
            frac_mgr,
            outputs: Vec::new(),
            surf: None,
            bufs: Vec::new(),
            next_buf: 1,
            stamp: 0,
            retry: false,
            keep_mapped: true,
            last_scale120: Vec::new(),
            changed: true,
        };
        let list = globals.contents().clone_list();
        for g in list.iter().filter(|g| g.interface == WlOutput::interface().name) {
            st.add_output(globals.registry(), g.name, g.version, &qh);
        }
        // Two round trips: output details arrive after the bind.
        queue.roundtrip(&mut st).map_err(|e| format!("Wayland: {e}"))?;
        queue.roundtrip(&mut st).map_err(|e| format!("Wayland: {e}"))?;
        Ok(Wayland { conn, queue, st, _globals: globals })
    }
}

impl St {
    fn add_output(&mut self, reg: &WlRegistry, name: u32, version: u32, qh: &QueueHandle<St>) {
        let wl: WlOutput = reg.bind(name, version.min(4), qh, name);
        self.outputs.push(Output { reg: name, wl: Some(wl), scale: 1, ..Default::default() });
        self.changed = true;
    }

    /// Monitors left to right (then top to bottom); index = the "Monitor" setting.
    fn sorted(&self) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.outputs.len()).collect();
        v.sort_by_key(|&i| (self.outputs[i].x, self.outputs[i].y, self.outputs[i].reg));
        v
    }

    pub fn monitor_height(&self, monitor: u32) -> Option<i32> {
        self.output_index(monitor).map(|i| self.outputs[i].h)
    }

    /// Index into `outputs` for a "Monitor" setting (or FOLLOW: the output the surface is on, else the first).
    fn output_index(&self, monitor: u32) -> Option<usize> {
        let order = self.sorted();
        if monitor == super::FOLLOW {
            let on = self.surf.as_ref().and_then(|s| s.entered).and_then(|r| self.outputs.iter().position(|o| o.reg == r));
            return on.or_else(|| order.first().copied());
        }
        order.get((monitor as usize).min(order.len().max(1) - 1)).copied()
    }

    pub fn monitor_names(&self) -> Vec<String> {
        self.sorted()
            .into_iter()
            .map(|i| {
                let o = &self.outputs[i];
                let n = if o.name.is_empty() { format!("Monitor {}", o.reg) } else { o.name.clone() };
                format!("{n} ({}x{})", o.w, o.h)
            })
            .collect()
    }

    fn drop_surface(&mut self) {
        if let Some(s) = self.surf.take() {
            if s.scale120 > 0 {
                self.last_scale120.retain(|&(o, _)| o != s.out_reg);
                self.last_scale120.push((s.out_reg, s.scale120));
            }
            if let Some(f) = s.frac {
                f.destroy();
            }
            if let Some(v) = s.viewport {
                v.destroy();
            }
            s.layer.destroy();
            s.wl.destroy();
        }
        self.bufs.clear();
        self.retry = false;
    }

    /// `out` None = no output given: the compositor picks (KWin: the active screen = the focused window's).
    fn make_surface(&mut self, out: Option<usize>, qh: &QueueHandle<St>) {
        let (wl_out, reg) = out.map_or((None, 0), |i| (self.outputs[i].wl.clone(), self.outputs[i].reg));
        let wl = self.compositor.create_surface(qh, ());
        let layer = self.layer_shell.get_layer_surface(&wl, wl_out.as_ref(), Layer::Overlay, "aimdot".into(), qh, ());
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        // Empty input region: the crosshair never takes a click.
        let region: WlRegion = self.compositor.create_region(qh, ());
        wl.set_input_region(Some(&region));
        region.destroy();
        let frac = self.frac_mgr.as_ref().map(|m| m.get_fractional_scale(&wl, qh, ()));
        let viewport = self.viewporter.as_ref().map(|v| v.get_viewport(&wl, qh, ()));
        layer.set_size(8, 8);
        wl.commit();
        self.surf = Some(Surf {
            wl,
            layer,
            out_reg: reg,
            entered: None,
            configured: false,
            frac,
            viewport,
            scale120: self.last_scale120.iter().find(|&&(r, _)| r == reg).map_or(0, |&(_, s)| s),
            size: (8, 8),
            buffer_scale: 1,
        });
    }

    /// Shows `pic` (premultiplied ARGB, design centre = picture centre) on monitor `monitor`, moved by `off`
    /// physical pixels; `visible = false` unmaps the surface. Called only when something changed.
    pub fn present(&mut self, qh: &QueueHandle<St>, pic: &Canvas, visible: bool, monitor: u32, off: (i32, i32), new_pic: bool) {
        if new_pic {
            // Also while hidden: the buffer must not show an old picture when it comes back.
            self.stamp += 1;
        }
        if self.outputs.is_empty() {
            return;
        }
        if !visible && !self.keep_mapped {
            // Fade: gone from the screen; made again when shown (KWin fades it in and out).
            self.drop_surface();
            return;
        }
        // Instant: hidden = the same surface with a transparent buffer - no new / closed window, so no KWin
        // animation; it shows and hides in the same frame.
        let blank = !visible;
        let follow = monitor == super::FOLLOW;
        let Some(out) = self.output_index(monitor) else { return };
        let want = if follow { 0 } else { self.outputs[out].reg };
        if self.surf.as_ref().map_or(true, |s| s.out_reg != want) {
            self.drop_surface();
            self.make_surface((!follow).then_some(out), qh);
            return; // drawn after the first configure
        }
        let oscale = self.outputs[out].scale.max(1);
        let Some(s) = &mut self.surf else { return };
        // Physical size: picture + room for the offset on both sides (so the centre moves by exactly `off`).
        let (cw, ch) = (pic.w.max(2), pic.h.max(2));
        let (pw, ph) = (cw + 2 * off.0.abs(), ch + 2 * off.1.abs());
        let frac = if s.scale120 > 0 { s.scale120 as f64 / 120.0 } else { oscale as f64 };
        let int_scale = (frac - frac.round()).abs() < 1e-6;
        let (lw, lh, bw, bh, bscale) = if int_scale || s.viewport.is_none() {
            let k = if int_scale { frac.round() as i32 } else { oscale }.max(1);
            let (lw, lh) = ((pw + k - 1) / k, (ph + k - 1) / k);
            (lw, lh, lw * k, lh * k, k)
        } else {
            // Fractional scale: buffer in real pixels, viewport maps it to the logical size. ⚠ centring may be off
            // by up to 1 px, depending on how the compositor rounds.
            let lw = (pw as f64 / frac).ceil() as i32;
            let lh = (ph as f64 / frac).ceil() as i32;
            (lw, lh, (lw as f64 * frac).round() as i32, (lh as f64 * frac).round() as i32, 1)
        };
        if s.size != (lw, lh) {
            s.layer.set_size(lw as u32, lh as u32);
            s.size = (lw, lh);
        }
        if bscale != s.buffer_scale {
            s.wl.set_buffer_scale(bscale);
            s.buffer_scale = bscale;
        }
        if let Some(v) = &s.viewport {
            if bscale == 1 && !int_scale {
                v.set_destination(lw, lh);
            } else {
                v.set_destination(-1, -1);
            }
        }
        if !s.configured {
            return; // drawn after the first configure
        }
        let want = if blank { (u64::MAX - 1, 0, 0) } else { (self.stamp, off.0, off.1) };
        let surface = s.wl.clone();
        let Some(i) = self.buffer(bw, bh, qh) else {
            self.retry = true;
            return;
        };
        let b = &mut self.bufs[i];
        if b.stamp != want {
            let px = unsafe { std::slice::from_raw_parts_mut(b.ptr as *mut u32, (bw * bh) as usize) };
            px.fill(0);
            let x0 = (bw - cw) / 2 + off.0;
            let y0 = (bh - ch) / 2 + off.1;
            for y in 0..if blank { 0 } else { pic.h } {
                let dy = y0 + y;
                if dy < 0 || dy >= bh {
                    continue;
                }
                for x in 0..pic.w {
                    let dx = x0 + x;
                    if dx >= 0 && dx < bw {
                        px[(dy * bw + dx) as usize] = pic.px[(y * pic.w + x) as usize];
                    }
                }
            }
            b.stamp = want;
        }
        b.busy = true;
        surface.attach(Some(&b.wl), 0, 0);
        surface.damage_buffer(0, 0, bw, bh);
        surface.commit();
    }

    /// A free buffer of that size (keeps at most 3).
    fn buffer(&mut self, w: i32, h: i32, qh: &QueueHandle<St>) -> Option<usize> {
        self.bufs.retain(|b| b.busy || (b.w, b.h) == (w, h));
        if let Some(i) = self.bufs.iter().position(|b| !b.busy) {
            return Some(i);
        }
        if self.bufs.len() >= 3 {
            return None;
        }
        let len = (w * h * 4) as usize;
        unsafe {
            let fd = libc::memfd_create(c"aimdot".as_ptr(), libc::MFD_CLOEXEC);
            if fd < 0 {
                return None;
            }
            let fd = OwnedFd::from_raw_fd(fd);
            if libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) != 0 {
                return None;
            }
            let ptr = libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd.as_raw_fd(), 0);
            if ptr == libc::MAP_FAILED {
                return None;
            }
            let pool = self.shm.create_pool(fd.as_fd(), len as i32, qh, ());
            let id = self.next_buf;
            self.next_buf += 1;
            let wl = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, id);
            self.bufs.push(Buf { id, wl, _pool: pool, _fd: fd, ptr: ptr as *mut u8, len, w, h, busy: false, stamp: (u64::MAX, 0, 0) });
        }
        Some(self.bufs.len() - 1)
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for St {
    fn event(st: &mut St, reg: &WlRegistry, ev: wl_registry::Event, _: &GlobalListContents, _: &Connection, qh: &QueueHandle<St>) {
        match ev {
            wl_registry::Event::Global { name, interface, version } if interface == WlOutput::interface().name => {
                st.add_output(reg, name, version, qh);
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(i) = st.outputs.iter().position(|o| o.reg == name) {
                    if st.surf.as_ref().map_or(false, |s| s.out_reg == name || s.entered == Some(name)) {
                        st.drop_surface();
                    }
                    if let Some(wl) = st.outputs.remove(i).wl {
                        if wl.version() >= 3 {
                            wl.release();
                        }
                    }
                    st.changed = true;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, u32> for St {
    fn event(st: &mut St, _: &WlOutput, ev: wl_output::Event, reg: &u32, _: &Connection, _: &QueueHandle<St>) {
        let Some(o) = st.outputs.iter_mut().find(|o| o.reg == *reg) else { return };
        match ev {
            wl_output::Event::Geometry { x, y, .. } => {
                o.x = x;
                o.y = y;
            }
            wl_output::Event::Mode { flags, width, height, .. } => {
                if flags.into_result().map_or(false, |f| f.contains(wl_output::Mode::Current)) {
                    o.w = width;
                    o.h = height;
                }
            }
            wl_output::Event::Scale { factor } => o.scale = factor,
            wl_output::Event::Name { name } => o.name = name,
            wl_output::Event::Done => st.changed = true,
            _ => {}
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for St {
    fn event(st: &mut St, layer: &ZwlrLayerSurfaceV1, ev: zwlr_layer_surface_v1::Event, _: &(), _: &Connection, _: &QueueHandle<St>) {
        match ev {
            zwlr_layer_surface_v1::Event::Configure { serial, .. } => {
                layer.ack_configure(serial);
                // Only the current surface counts (not one already destroyed while hidden).
                if let Some(s) = st.surf.as_mut().filter(|s| &s.layer == layer) {
                    if !s.configured {
                        s.configured = true;
                        st.changed = true;
                    }
                }
            }
            zwlr_layer_surface_v1::Event::Closed => {
                // The compositor took it away (monitor gone, ...): make a new one on the next draw.
                if st.surf.as_ref().is_some_and(|s| &s.layer == layer) {
                    st.drop_surface();
                    st.changed = true;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WpFractionalScaleV1, ()> for St {
    fn event(st: &mut St, frac: &WpFractionalScaleV1, ev: wp_fractional_scale_v1::Event, _: &(), _: &Connection, _: &QueueHandle<St>) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = ev {
            if let Some(s) = st.surf.as_mut().filter(|s| s.frac.as_ref() == Some(frac)) {
                if s.scale120 != scale {
                    s.scale120 = scale;
                    st.changed = true;
                }
            }
        }
    }
}

impl Dispatch<WlBuffer, u32> for St {
    fn event(st: &mut St, _: &WlBuffer, ev: wl_buffer::Event, id: &u32, _: &Connection, _: &QueueHandle<St>) {
        if let wl_buffer::Event::Release = ev {
            if let Some(b) = st.bufs.iter_mut().find(|b| b.id == *id) {
                b.busy = false;
                if std::mem::take(&mut st.retry) {
                    st.changed = true;
                }
            }
        }
    }
}

delegate_noop!(St: WlCompositor);
delegate_noop!(St: WlShmPool);
delegate_noop!(St: WlRegion);
delegate_noop!(St: ZwlrLayerShellV1);
delegate_noop!(St: WpViewporter);
delegate_noop!(St: WpViewport);
delegate_noop!(St: WpFractionalScaleManagerV1);
delegate_noop!(St: ignore WlShm);
impl Dispatch<WlSurface, ()> for St {
    fn event(st: &mut St, surface: &WlSurface, ev: wl_surface::Event, _: &(), _: &Connection, _: &QueueHandle<St>) {
        if let wl_surface::Event::Enter { output } = ev {
            let reg = st.outputs.iter().find(|o| o.wl.as_ref() == Some(&output)).map(|o| o.reg);
            if let Some(s) = st.surf.as_mut().filter(|s| &s.wl == surface) {
                if reg.is_some() && s.entered != reg {
                    s.entered = reg;
                    st.changed = true; // its scale / height (size as % of the screen) may differ
                }
            }
        }
    }
}
