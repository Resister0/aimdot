//! The crosshair painter: a square pixel grid with an odd size, so there is one centre pixel. Brush, eraser,
//! flood fill, straight lines and shapes (disk, ring, square, cross with a gap), all with optional symmetry
//! (mirror left/right, up/down, both, 4-way turn, 8-way), undo/redo, and an
//! optional 1 px outline when the picture is exported. No platform code and no crates (the settings window
//! turns the exported RGBA into a PNG file in the image library).
//!
//! Centring: an odd picture of size n is drawn by the engine with its centre pixel at index n/2, which is the
//! same screen pixel the Designer uses for a crosshair with an odd thickness (see the tests).

use crate::image::Image;

pub const MIN_SIZE: usize = 9;
pub const MAX_SIZE: usize = 63;
pub const DEFAULT_SIZE: usize = 31;
const UNDO_STEPS: usize = 200;

/// Straight-alpha RGBA.
pub type Px = [u8; 4];
pub const CLEAR: Px = [0, 0, 0, 0];

/// Symmetry: every pixel you paint is also painted at these places (around the centre pixel).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Sym {
    #[default]
    Off,
    /// mirror left / right
    X,
    /// mirror up / down
    Y,
    /// both mirrors (4 copies)
    XY,
    /// 4-way turn (90 degree steps) - like a pinwheel
    Turn4,
    /// 8-way: both mirrors + the diagonals
    Eight,
}

impl Sym {
    pub const ALL: [Sym; 6] = [Sym::Off, Sym::X, Sym::Y, Sym::XY, Sym::Turn4, Sym::Eight];
    pub fn from_index(i: i32) -> Sym {
        Sym::ALL.get(i.max(0) as usize).copied().unwrap_or(Sym::Off)
    }
    pub fn index(self) -> i32 {
        Sym::ALL.iter().position(|&s| s == self).unwrap_or(0) as i32
    }
    /// The offsets (from the centre) that (dx, dy) is copied to, itself first.
    pub fn orbit(self, dx: i32, dy: i32) -> Vec<(i32, i32)> {
        let mut v = match self {
            Sym::Off => vec![(dx, dy)],
            Sym::X => vec![(dx, dy), (-dx, dy)],
            Sym::Y => vec![(dx, dy), (dx, -dy)],
            Sym::XY => vec![(dx, dy), (-dx, dy), (dx, -dy), (-dx, -dy)],
            Sym::Turn4 => vec![(dx, dy), (-dy, dx), (-dx, -dy), (dy, -dx)],
            Sym::Eight => vec![(dx, dy), (-dx, dy), (dx, -dy), (-dx, -dy), (dy, dx), (-dy, dx), (dy, -dx), (-dy, -dx)],
        };
        let first = v[0];
        v.sort_unstable();
        v.dedup();
        v.retain(|&p| p != first);
        v.insert(0, first);
        v
    }
}

/// Shape tools: drawn from the pressed pixel (the shape's centre) out to the mouse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    /// filled circle
    Disk,
    /// circle outline, `thick` px wide
    Ring,
    /// square outline, `thick` px wide
    Square,
    /// four arms, `thick` px wide, starting `gap` px from the centre
    Cross,
}

/// Cells of a shape around (cx, cy). `r` = radius / half size / arm length in px.
pub fn shape_cells(shape: Shape, cx: i32, cy: i32, r: i32, thick: i32, gap: i32) -> Vec<(i32, i32)> {
    let (r, t) = (r.max(0), thick.max(1));
    let mut v = Vec::new();
    match shape {
        Shape::Disk | Shape::Ring => {
            let (outer, inner) = (r as f32 + 0.5, r as f32 + 0.5 - t as f32);
            for dy in -r..=r {
                for dx in -r..=r {
                    let d = ((dx * dx + dy * dy) as f32).sqrt();
                    if d <= outer && (shape == Shape::Disk || d > inner) {
                        v.push((cx + dx, cy + dy));
                    }
                }
            }
        }
        Shape::Square => {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dy.abs()) > r - t {
                        v.push((cx + dx, cy + dy));
                    }
                }
            }
        }
        Shape::Cross => {
            let (a, b) = (-(t - 1) / 2, t / 2);
            for i in gap.max(0)..=r {
                for w in a..=b {
                    if i == 0 {
                        v.push((cx + w, cy));
                        v.push((cx, cy + w));
                    } else {
                        v.extend_from_slice(&[(cx + i, cy + w), (cx - i, cy + w), (cx + w, cy + i), (cx + w, cy - i)]);
                    }
                }
            }
            v.sort_unstable();
            v.dedup();
        }
    }
    v
}

/// Odd size inside the limits (rounds an even size up).
pub fn odd_size(n: usize) -> usize {
    let n = n.clamp(MIN_SIZE, MAX_SIZE);
    if n % 2 == 0 {
        (n + 1).min(MAX_SIZE)
    } else {
        n
    }
}

/// How a layer moves when the picture is animated (`Painter::frames` > 1). Frame 0 always shows the layer as
/// painted; `phase` runs 0..1 over one loop.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Effect {
    #[default]
    None,
    /// on for the first half of the loop, off for the second
    Blink,
    /// fades out and back in
    Pulse,
    /// turns once around the centre pixel
    Spin,
    /// grows to 1.5x and back (around the centre pixel)
    Grow,
}

impl Effect {
    pub const ALL: [Effect; 5] = [Effect::None, Effect::Blink, Effect::Pulse, Effect::Spin, Effect::Grow];
    pub fn from_index(i: i32) -> Effect {
        Effect::ALL.get(i.max(0) as usize).copied().unwrap_or(Effect::None)
    }
    pub fn index(self) -> i32 {
        Effect::ALL.iter().position(|&e| e == self).unwrap_or(0) as i32
    }
    pub fn name(self) -> &'static str {
        ["Still", "Blink", "Pulse", "Spin", "Grow"][self.index() as usize]
    }
}

pub const MAX_LAYERS: usize = 8;
pub const MAX_FRAMES: usize = 8;

/// One layer. Layers are drawn bottom (index 0) to top.
#[derive(Clone, PartialEq, Debug)]
pub struct Layer {
    pub name: String,
    /// `n * n` pixels - EMPTY for the active layer, whose pixels are `Painter::px` while it is active.
    pub px: Vec<Px>,
    pub visible: bool,
    pub opacity: f32,
    /// Show every pixel of the layer in this colour (each keeps its own opacity).
    pub tint: Option<[u8; 3]>,
    pub effect: Effect,
}

impl Layer {
    fn new(name: String) -> Layer {
        Layer { name, px: Vec::new(), visible: true, opacity: 1.0, tint: None, effect: Effect::None }
    }
}

#[derive(Clone, PartialEq)]
struct Snap {
    n: usize,
    px: Vec<Px>,
    layers: Vec<Layer>,
    active: usize,
}

#[derive(Clone)]
pub struct Painter {
    pub n: usize,
    /// The ACTIVE layer's pixels (row-major, `n * n`). Every tool paints here.
    pub px: Vec<Px>,
    pub sym: Sym,
    layers: Vec<Layer>,
    active: usize,
    /// Animation: 1 = a still picture, 2..=MAX_FRAMES = that many frames per loop.
    pub frames: usize,
    pub fps: f32,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
}

impl Default for Painter {
    fn default() -> Self {
        Painter::new(DEFAULT_SIZE)
    }
}

fn resize_px(px: &[Px], old: usize, n: usize) -> Vec<Px> {
    let mut out = vec![CLEAR; n * n];
    let off = (n as i32 - old as i32) / 2; // both odd: the difference is even
    for y in 0..old as i32 {
        for x in 0..old as i32 {
            let (tx, ty) = (x + off, y + off);
            if tx >= 0 && ty >= 0 && (tx as usize) < n && (ty as usize) < n {
                out[ty as usize * n + tx as usize] = px[y as usize * old + x as usize];
            }
        }
    }
    out
}

/// `top` over `bottom` (straight alpha).
fn over(top: Px, bottom: Px) -> Px {
    let (ta, ba) = (top[3] as f32 / 255.0, bottom[3] as f32 / 255.0);
    let a = ta + ba * (1.0 - ta);
    if a <= 0.0 {
        return CLEAR;
    }
    let ch = |i: usize| ((top[i] as f32 * ta + bottom[i] as f32 * ba * (1.0 - ta)) / a).round().clamp(0.0, 255.0) as u8;
    [ch(0), ch(1), ch(2), (a * 255.0).round() as u8]
}

impl Painter {
    pub fn new(n: usize) -> Painter {
        let n = odd_size(n);
        Painter {
            n,
            px: vec![CLEAR; n * n],
            sym: Sym::Off,
            layers: vec![Layer::new("Layer 1".into())],
            active: 0,
            frames: 1,
            fps: 8.0,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// A copy without the undo history (for previews, e.g. a line while it is dragged).
    pub fn sketch(&self) -> Painter {
        Painter { undo: Vec::new(), redo: Vec::new(), ..self.clone_light() }
    }

    fn clone_light(&self) -> Painter {
        Painter {
            n: self.n,
            px: self.px.clone(),
            sym: self.sym,
            layers: self.layers.clone(),
            active: self.active,
            frames: self.frames,
            fps: self.fps,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn centre(&self) -> usize {
        self.n / 2
    }

    /// A pixel of the active layer.
    pub fn get(&self, x: i32, y: i32) -> Option<Px> {
        self.inside(x, y).then(|| self.px[y as usize * self.n + x as usize])
    }

    fn inside(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && (x as usize) < self.n && (y as usize) < self.n
    }

    /// Nothing painted on any layer.
    pub fn is_empty(&self) -> bool {
        self.px.iter().all(|p| p[3] == 0) && self.layers.iter().all(|l| l.px.iter().all(|p| p[3] == 0))
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn snap(&self) -> Snap {
        Snap { n: self.n, px: self.px.clone(), layers: self.layers.clone(), active: self.active }
    }

    fn restore(&mut self, s: Snap) {
        self.n = s.n;
        self.px = s.px;
        self.layers = s.layers;
        self.active = s.active.min(self.layers.len() - 1);
    }

    /// Call once before every change (a stroke, a fill, a line, clear, resize, a layer change): one undo step.
    pub fn checkpoint(&mut self) {
        self.undo.push(self.snap());
        if self.undo.len() > UNDO_STEPS {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Drops the last checkpoint if nothing changed since (a click that painted nothing new).
    pub fn drop_checkpoint_if_same(&mut self) {
        if self.undo.last().map_or(false, |s| *s == self.snap()) {
            self.undo.pop();
        }
    }

    pub fn undo(&mut self) -> bool {
        match self.undo.pop() {
            Some(s) => {
                self.redo.push(self.snap());
                self.restore(s);
                true
            }
            None => false,
        }
    }

    /// Forgets the redo steps (after taking back something that shouldn't come back, e.g. a cancelled import).
    pub fn drop_redo(&mut self) {
        self.redo.clear();
    }

    pub fn redo(&mut self) -> bool {
        match self.redo.pop() {
            Some(s) => {
                self.undo.push(self.snap());
                self.restore(s);
                true
            }
            None => false,
        }
    }

    /// The point plus its symmetric copies (no duplicates).
    fn mirrored(&self, x: i32, y: i32) -> Vec<(i32, i32)> {
        let c = self.centre() as i32;
        self.sym.orbit(x - c, y - c).into_iter().map(|(dx, dy)| (c + dx, c + dy)).collect()
    }

    fn set_raw(&mut self, x: i32, y: i32, c: Px) {
        if self.inside(x, y) {
            let i = y as usize * self.n + x as usize;
            self.px[i] = if c[3] == 0 { CLEAR } else { c };
        }
    }

    /// One dab of a square brush (`size` 1, 3, 5 ... centred on the pixel), mirrored. `CLEAR` erases.
    pub fn dab(&mut self, x: i32, y: i32, c: Px, size: i32) {
        let r = (size.max(1) - 1) / 2;
        for (mx, my) in self.mirrored(x, y) {
            for dy in -r..=r {
                for dx in -r..=r {
                    self.set_raw(mx + dx, my + dy, c);
                }
            }
        }
    }

    /// Cells of a straight line (Bresenham), both ends included.
    pub fn line_cells(x0: i32, y0: i32, x1: i32, y1: i32) -> Vec<(i32, i32)> {
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let (mut x, mut y, mut err) = (x0, y0, dx + dy);
        let mut v = Vec::with_capacity((dx - dy + 1) as usize);
        loop {
            v.push((x, y));
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
        v
    }

    /// A line of brush dabs (also used to join the mouse positions of a stroke, so fast strokes have no gaps).
    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Px, size: i32) {
        for (x, y) in Self::line_cells(x0, y0, x1, y1) {
            self.dab(x, y, c, size);
        }
    }

    /// A shape (see `shape_cells`), with symmetry.
    pub fn shape(&mut self, shape: Shape, cx: i32, cy: i32, r: i32, c: Px, thick: i32, gap: i32) {
        for (x, y) in shape_cells(shape, cx, cy, r, thick, gap) {
            self.dab(x, y, c, 1);
        }
    }

    /// Flood fill (4-neighbours, exact colour match), mirrored.
    pub fn fill(&mut self, x: i32, y: i32, c: Px) {
        let c = if c[3] == 0 { CLEAR } else { c };
        for (sx, sy) in self.mirrored(x, y) {
            let Some(target) = self.get(sx, sy) else { continue };
            if target == c {
                continue;
            }
            let mut stack = vec![(sx, sy)];
            while let Some((px, py)) = stack.pop() {
                if self.get(px, py) != Some(target) {
                    continue;
                }
                self.set_raw(px, py, c);
                stack.extend_from_slice(&[(px + 1, py), (px - 1, py), (px, py + 1), (px, py - 1)]);
            }
        }
    }

    /// Clears the active layer.
    pub fn clear(&mut self) {
        self.px.fill(CLEAR);
    }

    /// New size (odd) for every layer; the picture stays centred (cut or padded evenly).
    pub fn resize(&mut self, n: usize) {
        let n = odd_size(n);
        if n == self.n {
            return;
        }
        let old = self.n;
        self.px = resize_px(&self.px, old, n);
        for l in &mut self.layers {
            if !l.px.is_empty() {
                l.px = resize_px(&l.px, old, n);
            }
        }
        self.n = n;
    }

    /// Opens a picture (up to MAX_SIZE on each side), centred in an odd canvas, as the only layer.
    /// Keeps the undo history (loading is one undo step if `checkpoint` was called first).
    pub fn load(&mut self, img: &Image) -> bool {
        if img.w == 0 || img.h == 0 || img.w > MAX_SIZE || img.h > MAX_SIZE || img.rgba.len() < img.w * img.h * 4 {
            return false;
        }
        let n = odd_size(img.w.max(img.h));
        let (ox, oy) = ((n - img.w) / 2, (n - img.h) / 2);
        let mut px = vec![CLEAR; n * n];
        for y in 0..img.h {
            for x in 0..img.w {
                let s = &img.rgba[(y * img.w + x) * 4..(y * img.w + x) * 4 + 4];
                px[(y + oy) * n + x + ox] = if s[3] == 0 { CLEAR } else { [s[0], s[1], s[2], s[3]] };
            }
        }
        self.n = n;
        self.px = px;
        self.layers = vec![Layer::new("Layer 1".into())];
        self.active = 0;
        true
    }

    // ---------------------------------------------------------------- layers

    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    pub fn active(&self) -> usize {
        self.active
    }

    /// Pixels of layer `i` (the active one's are `px`).
    pub fn layer_px(&self, i: usize) -> &[Px] {
        if i == self.active {
            &self.px
        } else {
            &self.layers[i].px
        }
    }

    /// Name, visibility, opacity, tint and effect of layer `i` (not its pixels - use `select_layer` + `px`).
    pub fn layer_mut(&mut self, i: usize) -> Option<&mut Layer> {
        self.layers.get_mut(i)
    }

    /// Runs `f` on all layers with every layer's pixels in place, then takes the active one's back out.
    fn with_all<R>(&mut self, f: impl FnOnce(&mut Vec<Layer>, &mut usize, usize) -> R) -> R {
        let n = self.n;
        self.layers[self.active].px = std::mem::take(&mut self.px);
        let r = f(&mut self.layers, &mut self.active, n);
        if self.layers.is_empty() {
            self.layers.push(Layer::new("Layer 1".into()));
        }
        for l in &mut self.layers {
            if l.px.len() != n * n {
                l.px = vec![CLEAR; n * n];
            }
        }
        self.active = self.active.min(self.layers.len() - 1);
        self.px = std::mem::take(&mut self.layers[self.active].px);
        r
    }

    /// Paint on layer `i` from now on.
    pub fn select_layer(&mut self, i: usize) {
        if i < self.layers.len() && i != self.active {
            self.with_all(|_, active, _| *active = i);
        }
    }

    fn free_layer_name(&self) -> String {
        (1..).map(|k| format!("Layer {k}")).find(|n| !self.layers.iter().any(|l| &l.name == n)).unwrap()
    }

    /// A new empty layer above the active one, made active. False when there are MAX_LAYERS already.
    pub fn add_layer(&mut self) -> bool {
        if self.layers.len() >= MAX_LAYERS {
            return false;
        }
        let name = self.free_layer_name();
        self.with_all(|ls, active, n| {
            let mut l = Layer::new(name);
            l.px = vec![CLEAR; n * n];
            ls.insert(*active + 1, l);
            *active += 1;
        });
        true
    }

    /// Copies of the chosen layers, each right above its original (as many as fit).
    pub fn duplicate_layers(&mut self, sel: &[usize]) -> bool {
        let mut sel: Vec<usize> = sel.iter().copied().filter(|&i| i < self.layers.len()).collect();
        sel.sort_unstable();
        sel.dedup();
        if sel.is_empty() || self.layers.len() + sel.len() > MAX_LAYERS {
            return false;
        }
        self.with_all(|ls, active, _| {
            for &i in sel.iter().rev() {
                let mut c = ls[i].clone();
                c.name = format!("{} copy", ls[i].name).chars().take(24).collect();
                ls.insert(i + 1, c);
                if *active > i {
                    *active += 1;
                }
            }
        });
        true
    }

    /// Removes the chosen layers (one empty layer stays when all go).
    pub fn delete_layers(&mut self, sel: &[usize]) {
        self.with_all(|ls, active, _| {
            let keep: Vec<bool> = (0..ls.len()).map(|i| !sel.contains(&i)).collect();
            let before = keep.iter().take(*active).filter(|&&k| !k).count();
            let mut i = 0;
            ls.retain(|_| {
                i += 1;
                keep[i - 1]
            });
            *active = active.saturating_sub(before);
        });
    }

    /// Bakes the chosen layers (2 or more) into the lowest of them: colours, opacity and visibility as they look
    /// now (frame 0). The merged layer keeps the lowest one's name and effect.
    pub fn merge_layers(&mut self, sel: &[usize]) -> bool {
        let mut sel: Vec<usize> = sel.iter().copied().filter(|&i| i < self.layers.len()).collect();
        sel.sort_unstable();
        sel.dedup();
        if sel.len() < 2 {
            return false;
        }
        let n = self.n;
        let c = self.centre() as i32;
        self.with_all(|ls, active, _| {
            let mut out = vec![CLEAR; n * n];
            for &i in &sel {
                if ls[i].visible {
                    let shown = shown_px(&ls[i], &ls[i].px, n, c, 0.0);
                    for (o, p) in out.iter_mut().zip(shown) {
                        *o = over(p, *o);
                    }
                }
            }
            let low = sel[0];
            ls[low].px = out;
            ls[low].opacity = 1.0;
            ls[low].tint = None;
            ls[low].visible = true;
            let gone = &sel[1..];
            let before = gone.iter().filter(|&&g| g < *active).count();
            let hit = gone.contains(active);
            let mut i = 0;
            ls.retain(|_| {
                i += 1;
                !gone.contains(&(i - 1))
            });
            *active = if hit { low } else { active.saturating_sub(before) };
        });
        true
    }

    /// Moves layer `i` one step up (towards the top) or down.
    pub fn move_layer(&mut self, i: usize, up: bool) -> bool {
        let j = if up { i + 1 } else { i.wrapping_sub(1) };
        if i >= self.layers.len() || j >= self.layers.len() {
            return false;
        }
        self.with_all(|ls, active, _| {
            ls.swap(i, j);
            if *active == i {
                *active = j;
            } else if *active == j {
                *active = i;
            }
        });
        true
    }

    /// Shifts the chosen layers' pixels by (dx, dy) (what moves out of the canvas is gone).
    pub fn nudge(&mut self, sel: &[usize], dx: i32, dy: i32) {
        self.with_all(|ls, _, n| {
            for &i in sel {
                let Some(l) = ls.get_mut(i) else { continue };
                let mut out = vec![CLEAR; n * n];
                for y in 0..n as i32 {
                    for x in 0..n as i32 {
                        let (tx, ty) = (x + dx, y + dy);
                        if tx >= 0 && ty >= 0 && (tx as usize) < n && (ty as usize) < n {
                            out[ty as usize * n + tx as usize] = l.px[y as usize * n + x as usize];
                        }
                    }
                }
                l.px = out;
            }
        });
    }

    // ---------------------------------------------------------------- the finished picture

    /// All visible layers drawn together at animation `phase` (0..1; 0 = as painted).
    pub fn composite(&self, phase: f32) -> Vec<Px> {
        let (n, c) = (self.n, self.centre() as i32);
        let mut out = vec![CLEAR; n * n];
        for (i, l) in self.layers.iter().enumerate() {
            if !l.visible {
                continue;
            }
            for (o, p) in out.iter_mut().zip(shown_px(l, self.layer_px(i), n, c, phase)) {
                if p[3] > 0 {
                    *o = over(p, *o);
                }
            }
        }
        out
    }

    /// The picture with an outline: transparent pixels next to (8 neighbours) a painted one get `outline`.
    /// `pad` = 1 adds a 1 px border all round so the outline also fits at the edges (size stays odd).
    pub fn outlined(&self, outline: Option<Px>, pad: usize) -> (usize, Vec<Px>) {
        self.outlined_at(outline, pad, 0.0)
    }

    pub fn outlined_at(&self, outline: Option<Px>, pad: usize, phase: f32) -> (usize, Vec<Px>) {
        let src = self.composite(phase);
        let n = self.n + 2 * pad;
        let mut out = vec![CLEAR; n * n];
        for y in 0..self.n {
            for x in 0..self.n {
                out[(y + pad) * n + x + pad] = src[y * self.n + x];
            }
        }
        if let Some(o) = outline.filter(|o| o[3] > 0) {
            let painted: Vec<bool> = out.iter().map(|p| p[3] > 0).collect();
            for y in 0..n as i32 {
                for x in 0..n as i32 {
                    if painted[y as usize * n + x as usize] {
                        continue;
                    }
                    let near = (-1..=1).any(|dy: i32| {
                        (-1..=1).any(|dx: i32| {
                            let (ax, ay) = (x + dx, y + dy);
                            ax >= 0 && ay >= 0 && (ax as usize) < n && (ay as usize) < n && painted[ay as usize * n + ax as usize]
                        })
                    });
                    if near {
                        out[y as usize * n + x as usize] = o;
                    }
                }
            }
        }
        (n, out)
    }

    /// The finished picture (frame 0) for the library (with a 1 px border when there is an outline).
    pub fn export(&self, outline: Option<Px>) -> Image {
        self.export_at(outline, 0.0)
    }

    pub fn export_at(&self, outline: Option<Px>, phase: f32) -> Image {
        let pad = if outline.map_or(false, |o| o[3] > 0) { 1 } else { 0 };
        let (n, px) = self.outlined_at(outline, pad, phase);
        Image { w: n, h: n, rgba: px.concat() }
    }

    /// Number of frames in one loop (1 when still).
    pub fn frame_count(&self) -> usize {
        self.frames.clamp(1, MAX_FRAMES)
    }

    /// Every frame of the animation (one frame when still).
    pub fn export_frames(&self, outline: Option<Px>) -> Vec<Image> {
        let k = self.frame_count();
        (0..k).map(|i| self.export_at(outline, i as f32 / k as f32)).collect()
    }

    /// All frames side by side in one picture - what the overlay plays (see `anim_name`).
    pub fn export_strip(&self, outline: Option<Px>) -> Image {
        strip(&self.export_frames(outline))
    }
}

/// Frames side by side (all the same size).
pub fn strip(frames: &[Image]) -> Image {
    let (w, h, k) = (frames[0].w, frames[0].h, frames.len());
    let mut rgba = Vec::with_capacity(w * h * k * 4);
    for y in 0..h {
        for f in frames {
            rgba.extend_from_slice(&f.rgba[y * w * 4..(y + 1) * w * 4]);
        }
    }
    Image { w: w * k, h, rgba }
}

/// A layer as it shows at `phase`: tint, opacity, then the effect.
fn shown_px(l: &Layer, px: &[Px], n: usize, c: i32, phase: f32) -> Vec<Px> {
    let tau = std::f32::consts::TAU;
    let mut alpha = l.opacity.clamp(0.0, 1.0);
    match l.effect {
        Effect::Blink if phase >= 0.5 => alpha = 0.0,
        Effect::Pulse => alpha *= 0.2 + 0.8 * (0.5 + 0.5 * (tau * phase).cos()),
        _ => {}
    }
    let look = |p: Px| -> Px {
        if p[3] == 0 || alpha <= 0.0 {
            return CLEAR;
        }
        let rgb = l.tint.unwrap_or([p[0], p[1], p[2]]);
        [rgb[0], rgb[1], rgb[2], (p[3] as f32 * alpha).round() as u8]
    };
    // where each pixel comes from (spin / grow: around the centre pixel, nearest neighbour)
    let (cos, sin, grow) = match l.effect {
        Effect::Spin => ((tau * phase).cos(), (tau * phase).sin(), 1.0),
        Effect::Grow => (1.0, 0.0, 1.0 + 0.25 * (1.0 - (tau * phase).cos())),
        _ => return px.iter().map(|&p| look(p)).collect(),
    };
    let mut out = vec![CLEAR; n * n];
    for y in 0..n as i32 {
        for x in 0..n as i32 {
            let (dx, dy) = ((x - c) as f32 / grow, (y - c) as f32 / grow);
            let sx = c + (dx * cos + dy * sin).round() as i32;
            let sy = c + (-dx * sin + dy * cos).round() as i32;
            if sx >= 0 && sy >= 0 && (sx as usize) < n && (sy as usize) < n {
                out[y as usize * n + x as usize] = look(px[sy as usize * n + sx as usize]);
            }
        }
    }
    out
}

/// File name of an animated picture: `stem (anim 4x8).png` = 4 frames side by side, 8 frames a second.
pub fn anim_name(stem: &str, frames: usize, fps: f32) -> String {
    format!("{stem} (anim {frames}x{}).png", fps.round().clamp(1.0, 60.0) as u32)
}

/// (frames, fps) from an animated picture's name (see `anim_name`); None for a still one.
pub fn anim_of(name: &str) -> Option<(usize, f32)> {
    let stem = name.strip_suffix(".png").or_else(|| name.strip_suffix(".PNG"))?;
    let inner = stem.strip_suffix(')')?;
    let at = inner.rfind("(anim ")?;
    let (f, s) = inner[at + 6..].split_once('x')?;
    let (f, s): (usize, u32) = (f.trim().parse().ok()?, s.trim().parse().ok()?);
    (2..=16).contains(&f).then_some((f, (s as f32).clamp(1.0, 60.0)))
}

// ------------------------------------------------------------------ screenshot import

/// The centre `n` x `n` square of a screenshot, around pixel ((w-1)/2, (h-1)/2) - the pixel the overlay uses as
/// the centre (1920 wide -> 959). Outside the picture = transparent.
pub fn centre_crop(img: &Image, n: usize) -> Vec<Px> {
    let (cx, cy) = (((img.w.max(1) - 1) / 2) as i64, ((img.h.max(1) - 1) / 2) as i64);
    let h = (n / 2) as i64;
    let mut v = Vec::with_capacity(n * n);
    for y in 0..n as i64 {
        for x in 0..n as i64 {
            let (sx, sy) = (cx - h + x, cy - h + y);
            if sx < 0 || sy < 0 || sx as usize >= img.w || sy as usize >= img.h || img.rgba.len() < img.w * img.h * 4 {
                v.push(CLEAR);
            } else {
                let i = (sy as usize * img.w + sx as usize) * 4;
                v.push([img.rgba[i], img.rgba[i + 1], img.rgba[i + 2], img.rgba[i + 3]]);
            }
        }
    }
    v
}

/// Colour distance 0..1 (RGB).
pub fn colour_dist(a: [u8; 3], b: [u8; 3]) -> f32 {
    let d = |i: usize| (a[i] as f32 - b[i] as f32) / 255.0;
    ((d(0) * d(0) + d(1) * d(1) + d(2) * d(2)) / 3.0).sqrt()
}

/// A good first guess for the crosshair's colour in a crop: the pixel least like the border (the background).
pub fn auto_key(crop: &[Px], n: usize) -> [u8; 3] {
    let mut sum = [0f32; 3];
    let mut cnt = 0f32;
    for y in 0..n {
        for x in 0..n {
            if x == 0 || y == 0 || x == n - 1 || y == n - 1 {
                let p = crop[y * n + x];
                for k in 0..3 {
                    sum[k] += p[k] as f32;
                }
                cnt += 1.0;
            }
        }
    }
    let bg = [(sum[0] / cnt.max(1.0)) as u8, (sum[1] / cnt.max(1.0)) as u8, (sum[2] / cnt.max(1.0)) as u8];
    crop.iter()
        .filter(|p| p[3] > 0)
        .map(|p| [p[0], p[1], p[2]])
        .max_by(|a, b| colour_dist(*a, bg).total_cmp(&colour_dist(*b, bg)))
        .unwrap_or([255, 255, 255])
}

/// Keeps the pixels within `tol` (0..1) of `key`, fully opaque; everything else becomes transparent.
/// `one_colour`: paint every kept pixel in that colour instead of its own.
pub fn key_out(crop: &[Px], key: [u8; 3], tol: f32, one_colour: Option<Px>) -> Vec<Px> {
    crop.iter()
        .map(|p| {
            if p[3] > 0 && colour_dist([p[0], p[1], p[2]], key) <= tol {
                one_colour.unwrap_or([p[0], p[1], p[2], 255])
            } else {
                CLEAR
            }
        })
        .collect()
}

impl Painter {
    /// Puts a whole picture of this canvas's size in (screenshot import). One undo step if `checkpoint` came first.
    pub fn set_pixels(&mut self, px: Vec<Px>) -> bool {
        if px.len() != self.n * self.n {
            return false;
        }
        self.px = px;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Profile;
    use crate::crosshair::Renderer;
    use crate::gfx::Canvas;

    const RED: Px = [255, 0, 0, 255];
    const BLUE: Px = [0, 0, 255, 128];

    fn count(p: &Painter, c: Px) -> usize {
        p.px.iter().filter(|&&q| q == c).count()
    }

    #[test]
    fn sizes_are_odd_and_clamped() {
        assert_eq!(odd_size(30), 31);
        assert_eq!(odd_size(31), 31);
        assert_eq!(odd_size(1), MIN_SIZE);
        assert_eq!(odd_size(500), MAX_SIZE);
        assert_eq!(odd_size(64), MAX_SIZE);
        assert_eq!(Painter::new(20).n, 21);
        assert_eq!(Painter::new(31).centre(), 15);
    }

    #[test]
    fn brush_mirror_and_eraser() {
        let mut p = Painter::new(15);
        p.sym = Sym::XY;
        p.dab(2, 3, RED, 1);
        for (x, y) in [(2, 3), (12, 3), (2, 11), (12, 11)] {
            assert_eq!(p.get(x, y), Some(RED));
        }
        assert_eq!(count(&p, RED), 4);
        // The centre maps onto itself: one pixel, not four.
        p.dab(7, 7, BLUE, 1);
        assert_eq!(count(&p, BLUE), 1);
        p.dab(12, 11, CLEAR, 1);
        assert_eq!(count(&p, RED), 0);
        // 3x3 brush at the edge is clipped, no panic
        p.sym = Sym::Off;
        p.dab(0, 0, RED, 3);
        assert_eq!(count(&p, RED), 4);
    }

    #[test]
    fn lines_have_no_gaps() {
        let cells = Painter::line_cells(0, 0, 10, 3);
        assert_eq!(cells.first(), Some(&(0, 0)));
        assert_eq!(cells.last(), Some(&(10, 3)));
        assert_eq!(cells.len(), 11);
        for w in cells.windows(2) {
            assert!((w[0].0 - w[1].0).abs() <= 1 && (w[0].1 - w[1].1).abs() <= 1);
        }
        let mut p = Painter::new(15);
        p.line(0, 7, 14, 7, RED, 1);
        assert_eq!(count(&p, RED), 15);
        assert_eq!(Painter::line_cells(4, 4, 4, 4), vec![(4, 4)]);
    }

    #[test]
    fn fill_stays_inside_a_border() {
        let mut p = Painter::new(15);
        // a closed box 3..=9
        for i in 3..=9 {
            p.dab(i, 3, RED, 1);
            p.dab(i, 9, RED, 1);
            p.dab(3, i, RED, 1);
            p.dab(9, i, RED, 1);
        }
        p.fill(6, 6, BLUE);
        assert_eq!(count(&p, BLUE), 25); // 5x5 inside
        p.fill(0, 0, RED);
        assert_eq!(count(&p, CLEAR), 0);
        // filling with the same colour does nothing (and doesn't loop)
        p.fill(6, 6, BLUE);
        assert_eq!(count(&p, BLUE), 25);
    }

    #[test]
    fn undo_redo_and_resize() {
        let mut p = Painter::new(15);
        assert!(!p.can_undo());
        p.checkpoint();
        p.dab(7, 7, RED, 1);
        p.checkpoint();
        p.resize(21);
        assert_eq!(p.n, 21);
        assert_eq!(p.get(10, 10), Some(RED)); // still in the centre
        assert!(p.undo());
        assert_eq!(p.n, 15);
        assert_eq!(p.get(7, 7), Some(RED));
        assert!(p.undo());
        assert!(p.is_empty());
        assert!(!p.undo());
        assert!(p.redo());
        assert_eq!(p.get(7, 7), Some(RED));
        // a new change clears the redo list
        p.checkpoint();
        p.clear();
        assert!(!p.can_redo());
        // a click that changed nothing leaves no undo step
        let steps = p.undo.len();
        p.checkpoint();
        p.dab(1, 1, CLEAR, 1);
        p.drop_checkpoint_if_same();
        assert_eq!(p.undo.len(), steps);
        // shrinking cuts evenly around the centre
        let mut q = Painter::new(21);
        q.dab(10, 10, RED, 1);
        q.dab(0, 0, BLUE, 1);
        q.resize(15);
        assert_eq!(q.get(7, 7), Some(RED));
        assert_eq!(count(&q, BLUE), 0);
    }

    #[test]
    fn outline_and_export() {
        let mut p = Painter::new(15);
        p.dab(0, 7, RED, 1);
        let black = [0, 0, 0, 255];
        let img = p.export(Some(black));
        assert_eq!((img.w, img.h), (17, 17));
        let at = |x: usize, y: usize| &img.rgba[(y * 17 + x) * 4..(y * 17 + x) * 4 + 4];
        assert_eq!(at(1, 8), &RED[..]); // moved by the 1 px border
        assert_eq!(at(0, 8), &black[..]); // outline fits at the left edge
        assert_eq!(at(2, 9), &black[..]); // diagonal neighbour
        assert_eq!(at(3, 8), &CLEAR[..]);
        let plain = p.export(None);
        assert_eq!((plain.w, plain.h), (15, 15));
        // load back (odd size, centred)
        let mut q = Painter::new(31);
        assert!(q.load(&plain));
        assert_eq!(q.n, 15);
        assert_eq!(q.get(0, 7), Some(RED));
        let wide = Image { w: 20, h: 10, rgba: vec![255; 20 * 10 * 4] };
        assert!(q.load(&wide));
        assert_eq!(q.n, 21);
        assert_eq!(q.get(0, 5), Some([255; 4]));
        assert_eq!(q.get(0, 4), Some(CLEAR));
        let big = Image { w: 64, h: 8, rgba: vec![255; 64 * 8 * 4] };
        assert!(!q.load(&big));
    }

    #[test]
    fn a_changed_file_is_read_again() {
        let dir = std::env::temp_dir().join(format!("aimdot-paint-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let fx = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let f = dir.join("draft.png");
        std::fs::copy(fx.join("cross.png"), &f).unwrap();
        let mut r = Renderer::new(dir.clone(), crate::load_png);
        let a = r.image("draft.png").map(|i| (i.w, i.h));
        assert!(r.thumb("draft.png", 32).is_some());
        std::fs::copy(fx.join("rgba8.png"), &f).unwrap();
        let b = r.image("draft.png").map(|i| (i.w, i.h));
        assert!(a.is_some() && b.is_some() && a != b, "{a:?} {b:?}");
        std::fs::remove_file(&f).unwrap();
        assert!(r.image("draft.png").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn symmetry_modes() {
        let n = |s: Sym, dx, dy| s.orbit(dx, dy).len();
        assert_eq!(n(Sym::Off, 3, 1), 1);
        assert_eq!(n(Sym::X, 3, 1), 2);
        assert_eq!(n(Sym::XY, 3, 1), 4);
        assert_eq!(n(Sym::Turn4, 3, 1), 4);
        assert_eq!(n(Sym::Eight, 3, 1), 8);
        assert_eq!(n(Sym::Eight, 3, 3), 4); // on a diagonal
        assert_eq!(n(Sym::Eight, 0, 0), 1); // the centre
        assert_eq!(Sym::Turn4.orbit(3, 1), vec![(3, 1), (-3, -1), (-1, 3), (1, -3)]);
        for s in Sym::ALL {
            assert_eq!(Sym::from_index(s.index()), s);
        }
        let mut p = Painter::new(15);
        p.sym = Sym::Turn4;
        p.dab(10, 6, RED, 1); // (3, -1) from the centre 7,7
        for (x, y) in [(10, 6), (8, 10), (4, 8), (6, 4)] {
            assert_eq!(p.get(x, y), Some(RED), "{x},{y}");
        }
        assert_eq!(count(&p, RED), 4);
    }

    #[test]
    fn shapes() {
        let ring = shape_cells(Shape::Ring, 0, 0, 5, 1, 0);
        assert!(ring.contains(&(5, 0)) && ring.contains(&(0, -5)) && !ring.contains(&(0, 0)) && !ring.contains(&(3, 0)));
        let disk = shape_cells(Shape::Disk, 0, 0, 2, 1, 0);
        assert!(disk.contains(&(0, 0)) && disk.contains(&(2, 0)) && !disk.contains(&(2, 2)));
        assert_eq!(shape_cells(Shape::Disk, 4, 4, 0, 1, 0), vec![(4, 4)]);
        let sq = shape_cells(Shape::Square, 0, 0, 3, 1, 0);
        assert_eq!(sq.len(), 7 * 7 - 5 * 5);
        let cross = shape_cells(Shape::Cross, 0, 0, 6, 1, 2);
        assert_eq!(cross.len(), 4 * 5); // arms 2..=6
        assert!(!cross.contains(&(1, 0)) && cross.contains(&(2, 0)) && cross.contains(&(0, -6)));
        let fat = shape_cells(Shape::Cross, 0, 0, 3, 3, 0);
        assert!(fat.contains(&(0, 0)) && fat.contains(&(3, 1)) && fat.contains(&(-1, -3)));
        let mut p = Painter::new(15);
        p.shape(Shape::Ring, 7, 7, 4, RED, 1, 0);
        assert_eq!(p.get(11, 7), Some(RED));
        assert_eq!(p.get(7, 7), Some(CLEAR));
    }

    #[test]
    fn screenshot_import() {
        // a 40x30 grey "screenshot" with a green plus around the overlay's centre pixel (19, 14)
        let (w, h) = (40usize, 30usize);
        let mut rgba = vec![90u8; w * h * 4];
        for i in 0..w * h {
            rgba[i * 4 + 3] = 255;
        }
        let mut put = |x: usize, y: usize, c: [u8; 4]| rgba[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&c);
        for d in 2..5 {
            put(19 + d, 14, [20, 250, 30, 255]);
            put(19 - d, 14, [20, 250, 30, 255]);
            put(19, 14 + d, [30, 240, 40, 255]);
            put(19, 14 - d, [30, 240, 40, 255]);
        }
        let img = Image { w, h, rgba };
        let crop = centre_crop(&img, 15);
        assert_eq!(crop.len(), 225);
        assert_eq!(crop[7 * 15 + 9], [20, 250, 30, 255]); // 2 px right of the centre pixel
        let key = auto_key(&crop, 15);
        assert!(colour_dist(key, [25, 245, 35]) < 0.05, "{key:?}");
        let out = key_out(&crop, key, 0.1, None);
        assert_eq!(out.iter().filter(|p| p[3] > 0).count(), 12);
        let one = key_out(&crop, key, 0.1, Some(RED));
        assert_eq!(one[7 * 15 + 9], RED);
        let mut p = Painter::new(15);
        assert!(p.set_pixels(out));
        assert_eq!(p.get(9, 7), Some([20, 250, 30, 255]));
        assert!(!p.set_pixels(vec![CLEAR; 3]));
        // a crop bigger than the picture: the outside is transparent
        let tiny = Image { w: 3, h: 3, rgba: vec![255; 36] };
        let c = centre_crop(&tiny, 9);
        assert_eq!(c[4 * 9 + 4], [255; 4]);
        assert_eq!(c[0], CLEAR);
    }

    #[test]
    fn layers_add_select_merge_delete() {
        let mut p = Painter::new(15);
        p.dab(7, 7, RED, 1);
        assert!(p.add_layer());
        assert_eq!((p.layers().len(), p.active()), (2, 1));
        assert_eq!(p.get(7, 7), Some(CLEAR)); // the new layer is empty
        p.dab(1, 1, BLUE, 1);
        let c = p.composite(0.0);
        assert_eq!(c[7 * 15 + 7], RED);
        assert_eq!(c[15 + 1], BLUE);
        // hide the bottom layer
        p.layer_mut(0).unwrap().visible = false;
        assert_eq!(p.composite(0.0)[7 * 15 + 7], CLEAR);
        p.layer_mut(0).unwrap().visible = true;
        // tint + opacity
        p.layer_mut(0).unwrap().tint = Some([0, 0, 255]);
        p.layer_mut(0).unwrap().opacity = 0.5;
        assert_eq!(p.composite(0.0)[7 * 15 + 7], [0, 0, 255, 128]);
        // switching keeps each layer's pixels
        p.select_layer(0);
        assert_eq!(p.get(7, 7), Some(RED));
        p.select_layer(1);
        assert_eq!(p.get(1, 1), Some(BLUE));
        // undo covers layer changes
        p.checkpoint();
        assert!(p.add_layer());
        assert_eq!(p.layers().len(), 3);
        assert!(p.undo());
        assert_eq!((p.layers().len(), p.active()), (2, 1));
        // merge bakes tint + opacity into the lowest layer
        assert!(p.merge_layers(&[0, 1]));
        assert_eq!((p.layers().len(), p.active()), (1, 0));
        assert_eq!(p.get(7, 7), Some([0, 0, 255, 128]));
        assert_eq!(p.get(1, 1), Some(BLUE));
        assert_eq!(p.layers()[0].tint, None);
        // duplicate / move / delete
        assert!(p.duplicate_layers(&[0]));
        assert_eq!(p.layers()[1].name, "Layer 1 copy");
        assert!(p.move_layer(1, false));
        assert_eq!(p.layers()[0].name, "Layer 1 copy");
        p.delete_layers(&[0, 1]);
        assert_eq!(p.layers().len(), 1);
        assert!(p.is_empty());
        for _ in 0..MAX_LAYERS {
            p.add_layer();
        }
        assert_eq!(p.layers().len(), MAX_LAYERS);
        assert!(!p.add_layer());
        // resize reaches every layer
        p.select_layer(0);
        p.dab(7, 7, RED, 1);
        p.select_layer(3);
        p.resize(21);
        assert_eq!(p.layer_px(0)[10 * 21 + 10], RED);
    }

    #[test]
    fn nudge_moves_the_chosen_layers() {
        let mut p = Painter::new(15);
        p.dab(7, 7, RED, 1);
        p.add_layer();
        p.dab(7, 7, BLUE, 1);
        p.nudge(&[0], 2, -1);
        assert_eq!(p.layer_px(0)[6 * 15 + 9], RED);
        assert_eq!(p.get(7, 7), Some(BLUE)); // layer 1 not chosen
        p.nudge(&[1], 20, 0); // off the canvas: gone
        assert_eq!(p.get(7, 7), Some(CLEAR));
    }

    #[test]
    fn effects_and_frames() {
        let mut p = Painter::new(15);
        p.dab(10, 7, RED, 1); // 3 px right of the centre
        p.frames = 4;
        let at = |p: &Painter, ph: f32, x: usize, y: usize| p.composite(ph)[y * 15 + x];
        // frame 0 is always as painted
        for e in Effect::ALL {
            p.layer_mut(0).unwrap().effect = e;
            assert_eq!(at(&p, 0.0, 10, 7), RED, "{e:?}");
        }
        p.layer_mut(0).unwrap().effect = Effect::Blink;
        assert_eq!(at(&p, 0.75, 10, 7), CLEAR);
        p.layer_mut(0).unwrap().effect = Effect::Pulse;
        assert!(at(&p, 0.5, 10, 7)[3] < 60);
        p.layer_mut(0).unwrap().effect = Effect::Spin;
        assert_eq!(at(&p, 0.25, 7, 10), RED); // a quarter turn: right -> down
        assert_eq!(at(&p, 0.5, 4, 7), RED);
        p.layer_mut(0).unwrap().effect = Effect::Grow;
        let g = p.composite(0.5);
        assert!(g[7 * 15 + 11][3] > 0 || g[7 * 15 + 12][3] > 0, "moved out at 1.5x");
        // frames + strip
        let frames = p.export_frames(None);
        assert_eq!(frames.len(), 4);
        let s = p.export_strip(None);
        assert_eq!((s.w, s.h), (60, 15));
        assert_eq!(&s.rgba[(7 * 60 + 10) * 4..(7 * 60 + 10) * 4 + 4], &RED[..]);
        p.frames = 1;
        assert_eq!(p.export_frames(None).len(), 1);
    }

    #[test]
    fn animated_names() {
        assert_eq!(anim_name("Ring", 4, 8.0), "Ring (anim 4x8).png");
        assert_eq!(anim_of("Ring (anim 4x8).png"), Some((4, 8.0)));
        assert_eq!(anim_of(".paint-a (anim 6x12).png"), Some((6, 12.0)));
        assert_eq!(anim_of("Ring.png"), None);
        assert_eq!(anim_of("Ring (anim 1x8).png"), None);
        assert_eq!(anim_of("Ring (anim x).png"), None);
        let p = Profile { mode: 1, image: "Ring (anim 4x8).png".into(), ..Profile::default() };
        assert!(p.animated() && !p.rainbow_animated());
        assert!(!Profile { mode: 1, image: "Ring.png".into(), ..Profile::default() }.animated());
    }

    fn strip_loader(_: &std::path::Path) -> Option<Image> {
        // 4 frames of 9x9: frame i has one pixel at x = i
        let frames: Vec<Image> = (0..4)
            .map(|i| {
                let mut p = Painter::new(9);
                p.dab(i, 4, [255, 255, 255, 255], 1);
                p.export(None)
            })
            .collect();
        Some(strip(&frames))
    }

    #[test]
    fn the_renderer_plays_a_strip() {
        let mut r = Renderer::new(std::path::PathBuf::from("/nonexistent"), strip_loader);
        let p = Profile { mode: 1, image: "x (anim 4x8).png".into(), img_scale: 1.0, ..Profile::default() };
        assert_eq!(r.image_size(&p), Some((9, 9)));
        assert_eq!(r.thumb("x (anim 4x8).png", 9).map(|t| (t.w, t.h)), Some((9, 9)));
        let mut c = Canvas::new(0, 0);
        for (t, x) in [(0.0, 0), (0.13, 1), (0.26, 2), (0.38, 3), (0.5, 0)] {
            r.render(&p, t, &mut c);
            assert_eq!(ink(&c).0, x - 5, "t={t}"); // canvas 10 wide: x - 5 from the centre
        }
    }

    fn centre_dot_loader(_: &std::path::Path) -> Option<Image> {
        let mut p = Painter::new(31);
        p.dab(15, 15, [255, 255, 255, 255], 1);
        Some(p.export(None))
    }

    /// Bounding box of the visible pixels, relative to the canvas centre.
    fn ink(c: &Canvas) -> (i32, i32, i32, i32) {
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for y in 0..c.h {
            for x in 0..c.w {
                if c.px[(y * c.w + x) as usize] >> 24 > 0 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        (x0 - c.w / 2, y0 - c.h / 2, x1 - c.w / 2, y1 - c.h / 2)
    }

    #[test]
    fn painted_centre_pixel_is_the_designer_centre() {
        let mut r = Renderer::new(std::path::PathBuf::from("/nonexistent"), centre_dot_loader);
        let mut img = Canvas::new(0, 0);
        let p = Profile { mode: 1, image: "dot.png".into(), img_scale: 1.0, ..Profile::default() };
        r.render(&p, 0.0, &mut img);
        let mut des = Canvas::new(0, 0);
        let d = Profile { inner: false, outline: false, dot: true, dot_size: 1.0, ..Profile::default() };
        r.render(&d, 0.0, &mut des);
        assert_eq!(ink(&img), (-1, -1, -1, -1));
        assert_eq!(ink(&img), ink(&des));
        // 3x: the 3x3 block stays centred on the same pixel
        let p3 = Profile { img_scale: 3.0, img_smooth: false, ..p };
        r.render(&p3, 0.0, &mut img);
        assert_eq!(ink(&img), (-2, -2, 0, 0));
    }
}
