//! Viewer state independent of the windowing system: layers, tile cache, camera, scheduling.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use decoder::{J2kFile, TileDecoder};

use crate::overlay;
use crate::renderer::{GpuTile, Quad, Renderer};
use crate::scheduler::{Job, Scheduler};
use crate::view::{
    all_tiles, level_for_scale, overview_level, tile_image_rect, visible_tiles, Camera, TileKey,
};

/// Uploads per `update()`; keeps a burst of finished tiles from stalling a frame.
const UPLOADS_PER_UPDATE: usize = 24;
/// Size of a full tile texture, used to project cache growth when planning.
const TILE_BYTES: u64 = (crate::view::TILE as u64) * (crate::view::TILE as u64);
/// Default GPU tile cache budget.
pub const DEFAULT_CACHE_BYTES: u64 = 1 << 30;

struct Cached {
    gpu: GpuTile,
    last_used: u64,
}

struct Layer {
    file: Arc<J2kFile>,
    overview: u8,
    tiles: HashMap<TileKey, Cached>,
}

pub struct Viewer {
    pub renderer: Renderer,
    scheduler: Scheduler,
    files: Vec<PathBuf>,
    cur: usize,
    layers: HashMap<usize, Layer>,
    failed: HashSet<(usize, TileKey)>,
    pub last_error: Option<String>,
    pub camera: Camera,
    /// Re-fit on resize and layer change until the user zooms or pans.
    fit_mode: bool,
    frame: u64,
    budget: u64,
    dirty: bool,
    /// Jobs published by the last plan that were not loaded yet.
    unmet: usize,
    /// Prefetch this many layers on each side of the current one.
    pub prefetch: usize,
    /// Draw the status panel.
    pub overlay: bool,
    /// Pixel multiplier for overlay text.
    pub overlay_scale: u32,
    /// Draw the on-screen buttons (off for offscreen snapshots unless --overlay is given).
    pub show_buttons: bool,
    overlay_tile: Option<(Vec<String>, GpuTile)>,
    /// Button textures by label, normal and hovered.
    button_tiles: HashMap<(String, bool), GpuTile>,
    /// Index into this frame's button list.
    hover: Option<usize>,
    /// The dropdown under the Menu button is open.
    menu_open: bool,
    pub adapter_name: String,
    pub stats: Stats,
}

/// What an on-screen button does.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum UiAction {
    /// Pick an image; the other images in its folder become the layer list.
    OpenFile,
    ToggleMenu,
    ToggleDebug,
}

struct Btn {
    action: UiAction,
    label: String,
    /// Screen-pixel rectangle `(x0, y0, x1, y1)`.
    rect: [f32; 4],
}

struct Layout {
    /// Top-left of the status/welcome panel, if it is shown.
    panel_origin: Option<(f32, f32)>,
    buttons: Vec<Btn>,
}

/// Running counters shown in the overlay and used by the benchmark.
#[derive(Default, Clone, Debug)]
pub struct Stats {
    pub decoded: u64,
    pub decode_ms_sum: f64,
    pub last_decode_ms: f64,
    pub evicted: u64,
    /// CPU time of the last `render` call, ms.
    pub frame_ms: f64,
}

impl Viewer {
    pub fn new(
        renderer: Renderer,
        scheduler: Scheduler,
        files: Vec<PathBuf>,
        index: usize,
        viewport: (u32, u32),
    ) -> Self {
        let (vw, vh) = (f64::from(viewport.0.max(1)), f64::from(viewport.1.max(1)));
        let mut v = Self {
            renderer,
            scheduler,
            files,
            cur: 0,
            layers: HashMap::new(),
            failed: HashSet::new(),
            last_error: None,
            camera: Camera {
                cx: 0.0,
                cy: 0.0,
                scale: 1.0,
                vw,
                vh,
            },
            fit_mode: true,
            frame: 0,
            budget: DEFAULT_CACHE_BYTES,
            dirty: true,
            unmet: 0,
            prefetch: 1,
            overlay: false,
            overlay_scale: 1,
            show_buttons: true,
            overlay_tile: None,
            button_tiles: HashMap::new(),
            hover: None,
            menu_open: false,
            adapter_name: String::new(),
            stats: Stats::default(),
        };
        v.open_index(index);
        v
    }

    pub fn set_cache_budget(&mut self, bytes: u64) {
        self.budget = bytes;
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn current_path(&self) -> Option<&std::path::Path> {
        self.files.get(self.cur).map(PathBuf::as_path)
    }

    /// No image is on screen (nothing opened yet, or the opened file could not be read).
    pub fn is_empty_state(&self) -> bool {
        !self.layers.contains_key(&self.cur)
    }

    fn current_name(&self) -> String {
        self.current_path()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn ensure_layer(&mut self, i: usize) -> bool {
        if self.layers.contains_key(&i) {
            return true;
        }
        match J2kFile::open(&self.files[i]) {
            Ok(file) => {
                let overview = overview_level(file.info());
                self.layers.insert(
                    i,
                    Layer {
                        file: Arc::new(file),
                        overview,
                        tiles: HashMap::new(),
                    },
                );
                true
            }
            Err(e) => {
                self.last_error = Some(e);
                false
            }
        }
    }

    /// Show layer `i`, keeping the current zoom and position.
    pub fn open_index(&mut self, i: usize) {
        if i >= self.files.len() {
            return;
        }
        self.last_error = None;
        let first = self.layers.is_empty();
        if !self.ensure_layer(i) {
            return;
        }
        self.cur = i;
        let span = self.prefetch;
        self.layers.retain(|&k, _| k.abs_diff(i) <= span);
        self.failed.retain(|&(k, _)| k.abs_diff(i) <= span);
        let info = self.layers[&i].file.info().clone();
        if first || self.fit_mode {
            self.fit();
        } else {
            self.camera.clamp_centre(info.width, info.height);
        }
        self.dirty = true;
    }

    /// Replace the layer list (a new folder or dropped file) and show `index`, fitted.
    pub fn set_files(&mut self, files: Vec<PathBuf>, index: usize) {
        self.files = files;
        self.layers.clear();
        self.failed.clear();
        self.fit_mode = true;
        self.open_index(index.min(self.files.len().saturating_sub(1)));
    }

    /// Place the camera explicitly (image-px centre and screen px per image px).
    pub fn set_view(&mut self, cx: Option<f64>, cy: Option<f64>, scale: Option<f64>) {
        if let Some(s) = scale {
            self.camera.scale = s.clamp(1e-4, crate::view::MAX_SCALE);
        }
        if let Some(x) = cx {
            self.camera.cx = x;
        }
        if let Some(y) = cy {
            self.camera.cy = y;
        }
        self.fit_mode = false;
        self.dirty = true;
    }

    pub fn step(&mut self, delta: isize) {
        let n = self.files.len() as isize;
        if n == 0 {
            return;
        }
        let target = (self.cur as isize + delta).clamp(0, n - 1) as usize;
        if target != self.cur {
            self.open_index(target);
        }
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        self.camera.vw = f64::from(w.max(1));
        self.camera.vh = f64::from(h.max(1));
        if self.fit_mode {
            self.fit();
        }
        self.dirty = true;
    }

    pub fn fit(&mut self) {
        if let Some(l) = self.layers.get(&self.cur) {
            let i = l.file.info();
            self.camera = Camera::fit(i.width, i.height, self.camera.vw, self.camera.vh);
        }
        self.fit_mode = true;
        self.dirty = true;
    }

    /// 1 image px = 1 screen px, around the current centre.
    pub fn actual_size(&mut self) {
        self.camera.scale = 1.0;
        self.fit_mode = false;
        self.dirty = true;
    }

    pub fn zoom_at(&mut self, sx: f64, sy: f64, factor: f64) {
        let Some(l) = self.layers.get(&self.cur) else {
            return;
        };
        let i = l.file.info();
        let min = Camera::min_scale(i.width, i.height, self.camera.vw, self.camera.vh);
        self.camera.zoom_at(sx, sy, factor, min);
        self.fit_mode = false;
        self.dirty = true;
    }

    pub fn pan(&mut self, dx: f64, dy: f64) {
        let Some(l) = self.layers.get(&self.cur) else {
            return;
        };
        let i = l.file.info();
        self.camera.pan(dx, dy);
        self.camera.clamp_centre(i.width, i.height);
        self.fit_mode = false;
        self.dirty = true;
    }

    /// Pyramid level shown at the current zoom.
    pub fn current_level(&self) -> u8 {
        self.layers
            .get(&self.cur)
            .map(|l| level_for_scale(self.camera.scale, l.file.info().levels).min(l.overview))
            .unwrap_or(0)
    }

    /// Collect finished decodes, upload them, and publish the next set of wanted tiles.
    /// Returns true if more finished tiles are waiting (call again soon).
    pub fn update(&mut self) -> bool {
        let mut more = false;
        for n in 0.. {
            if n == UPLOADS_PER_UPDATE {
                more = true;
                break;
            }
            let Some(done) = self.scheduler.try_recv() else {
                break;
            };
            match done.result {
                Ok(buf) => {
                    self.stats.decoded += 1;
                    self.stats.decode_ms_sum += done.ms;
                    self.stats.last_decode_ms = done.ms;
                    if let Some(layer) = self.layers.get_mut(&done.image) {
                        let gpu = self.renderer.upload(&buf);
                        layer.tiles.insert(
                            done.key,
                            Cached {
                                gpu,
                                last_used: self.frame,
                            },
                        );
                        self.dirty = true;
                    }
                }
                Err(e) => {
                    eprintln!("tile {:?} of layer {} failed: {e}", done.key, done.image);
                    self.last_error = Some(e);
                    self.failed.insert((done.image, done.key));
                }
            }
        }
        if self.dirty {
            self.dirty = false;
            self.plan();
        }
        self.evict();
        // Let wgpu actually free textures dropped by eviction.
        let _ = self.renderer.device.poll(wgpu::PollType::Poll);
        more
    }

    /// Every wanted tile (current layer and prefetch) is resident or failed, and nothing is decoding.
    pub fn is_settled(&self) -> bool {
        self.unmet == 0 && self.scheduler.outstanding() == 0 && !self.dirty
    }

    fn plan(&mut self) {
        let mut jobs = Vec::new();
        let mut projected = self.cache_bytes();
        let order: Vec<usize> = std::iter::once(self.cur)
            .chain((1..=self.prefetch).flat_map(|d| [self.cur + d, self.cur.wrapping_sub(d)]))
            .filter(|&i| i < self.files.len())
            .collect();
        for &i in &order {
            if !self.ensure_layer(i) {
                continue;
            }
            let layer = &self.layers[&i];
            let info = layer.file.info();
            let want_level = level_for_scale(self.camera.scale, info.levels).min(layer.overview);
            // The whole-image overview first (it is the stand-in while detail loads), then detail
            // around the view. Neighbouring layers get the same view, assuming similar sizes.
            // Only the current layer's overview and visible tiles are required; the margin and
            // neighbouring layers are optional and are skipped once the cache budget is full, or
            // eviction would drop them and the planner would immediately re-request them.
            let mut keys: Vec<(TileKey, bool)> = all_tiles(info, layer.overview)
                .into_iter()
                .map(|k| (k, i == self.cur))
                .collect();
            if want_level != layer.overview {
                let visible = visible_tiles(&self.camera, info, want_level, 0);
                keys.extend(visible.iter().map(|&k| (k, i == self.cur)));
                if i == self.cur {
                    keys.extend(
                        visible_tiles(&self.camera, info, want_level, 1)
                            .into_iter()
                            .filter(|k| !visible.contains(k))
                            .map(|k| (k, false)),
                    );
                }
            }
            for (key, required) in keys {
                if layer.tiles.contains_key(&key) || self.failed.contains(&(i, key)) {
                    continue;
                }
                if !required && projected + TILE_BYTES > self.budget {
                    continue;
                }
                projected += TILE_BYTES;
                jobs.push(Job {
                    image: i,
                    key,
                    file: layer.file.clone(),
                });
            }
        }
        self.unmet = jobs.len();
        self.scheduler.set_wanted(jobs);
    }

    /// Drop least-recently-drawn tiles when over budget. Overview tiles stay resident.
    fn evict(&mut self) {
        let total: u64 = self
            .layers
            .values()
            .flat_map(|l| l.tiles.values())
            .map(|c| c.gpu.bytes())
            .sum();
        if total <= self.budget {
            return;
        }
        let frame = self.frame;
        let mut victims: Vec<(u64, usize, TileKey)> = self
            .layers
            .iter()
            .flat_map(|(&i, l)| {
                l.tiles
                    .iter()
                    .filter(move |(k, c)| k.level != l.overview && c.last_used < frame)
                    .map(move |(&k, c)| (c.last_used, i, k))
            })
            .collect();
        victims.sort_unstable_by_key(|v| v.0);
        let mut total = total;
        for (_, i, key) in victims {
            if total <= self.budget {
                break;
            }
            if let Some(c) = self.layers.get_mut(&i).and_then(|l| l.tiles.remove(&key)) {
                total -= c.gpu.bytes();
                self.stats.evicted += 1;
                self.dirty = true; // evicted tiles that are still wanted must be re-requested
            }
        }
    }

    /// Draw the current layer into `target`.
    pub fn render(&mut self, target: &wgpu::TextureView, size: (u32, u32)) {
        let t = Instant::now();
        self.with_quads(|r, quads, nearest| r.render(target, size, nearest, quads));
        self.stats.frame_ms = t.elapsed().as_secs_f64() * 1e3;
    }

    /// RGBA pixels of the current view, for `--snapshot`.
    pub fn render_to_rgba(&mut self, size: (u32, u32)) -> Vec<u8> {
        self.with_quads(|r, quads, nearest| r.render_to_rgba(size, nearest, quads))
    }

    /// Build this frame's quads (tiles coarse-to-fine, then the overlay panel) and hand them to `f`.
    fn with_quads<R>(&mut self, f: impl FnOnce(&mut Renderer, &[Quad], bool) -> R) -> R {
        let (rects, nearest) = self.draw_list();
        self.refresh_overlay();
        let layout = self.layout();
        self.ensure_button_tiles(&layout);
        let layer = self.layers.get(&self.cur);
        let mut quads: Vec<Quad> = match layer {
            Some(l) => rects
                .iter()
                .map(|(key, rect)| Quad {
                    rect: *rect,
                    tile: &l.tiles[key].gpu,
                })
                .collect(),
            None => Vec::new(),
        };
        if let (Some((x, y)), Some((_, tile))) = (layout.panel_origin, &self.overlay_tile) {
            quads.push(Quad {
                rect: [x, y, x + tile.w as f32, y + tile.h as f32],
                tile,
            });
        }
        for (i, b) in layout.buttons.iter().enumerate() {
            if !self.show_buttons {
                break;
            }
            if let Some(tile) = self
                .button_tiles
                .get(&(b.label.clone(), self.hover == Some(i)))
            {
                quads.push(Quad { rect: b.rect, tile });
            }
        }
        // Tiles use the view's filter; the panels are 1:1 so filtering never blurs them.
        f(&mut self.renderer, &quads, nearest)
    }

    /// Where the status/welcome panel and the buttons go this frame (screen px).
    fn layout(&self) -> Layout {
        let s = self.overlay_scale.max(1);
        let sf = s as f32;
        let (vw, vh) = (self.camera.vw as f32, self.camera.vh as f32);
        let empty = self.is_empty_state();
        let m = 12.0 * sf;
        let size = |label: &str| {
            let (w, h) = overlay::text_size(&[label.to_string()], s);
            (w as f32, h as f32)
        };
        let mut buttons: Vec<Btn> = Vec::new();
        let mut push = |action, label: String, x: f32, y: f32, w: f32, h: f32| {
            buttons.push(Btn {
                action,
                label,
                rect: [x.round(), y.round(), (x + w).round(), (y + h).round()],
            });
        };

        // Menu button in the top-right corner, with its dropdown hanging below it.
        let menu_label = if self.menu_open { "Menu ^" } else { "Menu v" };
        let (mw, mh) = size(menu_label);
        push(
            UiAction::ToggleMenu,
            menu_label.to_string(),
            (vw - m - mw).max(0.0),
            m,
            mw,
            mh,
        );
        if self.menu_open {
            let items = [
                (UiAction::OpenFile, "Open image...  (O)".to_string()),
                (
                    UiAction::ToggleDebug,
                    format!("[{}] Debug info  (I)", if self.overlay { 'x' } else { ' ' }),
                ),
            ];
            let chars = items.iter().map(|i| i.1.chars().count()).max().unwrap_or(0);
            let mut y = m + mh + 4.0 * sf;
            for (action, label) in items {
                let label = format!("{label:<chars$}"); // equal widths so the list lines up
                let (iw, ih) = size(&label);
                push(action, label, (vw - m - iw).max(0.0), y, iw, ih);
                y += ih + 2.0 * sf;
            }
        }

        if empty {
            // Welcome panel in the middle with a prominent Open button under it.
            let (pw, ph) = overlay::text_size(&self.panel_lines(), s);
            let (pw, ph) = (pw as f32, ph as f32);
            let (bw, bh) = size("Open (O)");
            let space = 16.0 * sf;
            let top = ((vh - (ph + space + bh)) / 2.0).max(0.0);
            push(
                UiAction::OpenFile,
                "Open (O)".to_string(),
                ((vw - bw) / 2.0).max(0.0),
                top + ph + space,
                bw,
                bh,
            );
            Layout {
                panel_origin: Some((((vw - pw) / 2.0).max(0.0).round(), top.round())),
                buttons,
            }
        } else {
            Layout {
                panel_origin: self.overlay.then_some((m, m)),
                buttons,
            }
        }
    }

    /// Button under screen position `(x, y)`: its index in this frame's list and its action.
    pub fn button_at(&self, x: f64, y: f64) -> Option<(usize, UiAction)> {
        let (x, y) = (x as f32, y as f32);
        self.layout()
            .buttons
            .iter()
            .enumerate()
            .find(|(_, b)| x >= b.rect[0] && x < b.rect[2] && y >= b.rect[1] && y < b.rect[3])
            .map(|(i, b)| (i, b.action))
    }

    /// Set the hovered button; true if it changed (a redraw is needed).
    pub fn set_hover(&mut self, h: Option<(usize, UiAction)>) -> bool {
        let h = h.map(|(i, _)| i);
        std::mem::replace(&mut self.hover, h) != h
    }

    /// Run a button's action. Returns `Some` when the window must do it (file dialogs).
    pub fn activate(&mut self, action: UiAction) -> Option<UiAction> {
        self.hover = None; // the layout is about to change under the cursor
        match action {
            UiAction::ToggleMenu => {
                self.menu_open = !self.menu_open;
                None
            }
            UiAction::ToggleDebug => {
                self.overlay = !self.overlay;
                self.menu_open = false;
                None
            }
            UiAction::OpenFile => {
                self.menu_open = false;
                Some(action)
            }
        }
    }

    pub fn menu_open(&self) -> bool {
        self.menu_open
    }

    pub fn close_menu(&mut self) {
        self.menu_open = false;
        self.hover = None;
    }

    fn ensure_button_tiles(&mut self, layout: &Layout) {
        let scale = self.overlay_scale.max(1);
        for b in &layout.buttons {
            for hot in [false, true] {
                if !self.button_tiles.contains_key(&(b.label.clone(), hot)) {
                    let bg = if hot { 110 } else { 62 };
                    let buf = overlay::render_styled(&[b.label.clone()], scale, bg);
                    self.button_tiles
                        .insert((b.label.clone(), hot), self.renderer.upload(&buf));
                }
            }
        }
    }

    /// Lines of the main panel: the welcome text when nothing is open, else live status.
    fn panel_lines(&self) -> Vec<String> {
        if !self.is_empty_state() {
            return self.status_lines();
        }
        let mut v = vec![
            "j2k viewer".to_string(),
            String::new(),
            "Open an image: the other images in its folder load as layers.".to_string(),
            "You can also drag and drop a file or folder onto this window.".to_string(),
        ];
        if let Some(e) = &self.last_error {
            v.push(String::new());
            v.push(format!("ERROR: {e}"));
        }
        v
    }

    /// Re-render the panel texture only when its text changed.
    fn refresh_overlay(&mut self) {
        if !self.overlay && !self.is_empty_state() {
            return;
        }
        let lines = self.panel_lines();
        if self
            .overlay_tile
            .as_ref()
            .is_some_and(|(old, _)| *old == lines)
        {
            return;
        }
        let tile = self
            .renderer
            .upload(&overlay::render_text(&lines, self.overlay_scale.max(1)));
        self.overlay_tile = Some((lines, tile));
    }

    /// Text for the status panel.
    pub fn status_lines(&self) -> Vec<String> {
        let name = self.current_name();
        let mut v = vec![format!("{name}  [{}/{}]", self.cur + 1, self.files.len())];
        if let Some(layer) = self.layers.get(&self.cur) {
            let i = layer.file.info();
            v.push(format!(
                "{}x{} px  {:.0} MB file",
                i.width,
                i.height,
                layer.file.file_len() as f64 / 1e6
            ));
            v.push(format!(
                "zoom {:.1}%  level {}/{}  tiles {}",
                self.camera.scale * 100.0,
                self.current_level(),
                i.levels,
                layer.tiles.len()
            ));
        }
        let cache = self.cache_bytes() as f64 / 1e6;
        v.push(format!(
            "cache {cache:.0} / {:.0} MB  evicted {}",
            self.budget as f64 / 1e6,
            self.stats.evicted
        ));
        let avg = if self.stats.decoded > 0 {
            self.stats.decode_ms_sum / self.stats.decoded as f64
        } else {
            0.0
        };
        v.push(format!(
            "decoding {}  last {:.0} ms  avg {avg:.0} ms",
            self.scheduler.outstanding(),
            self.stats.last_decode_ms
        ));
        v.push(format!(
            "frame {:.1} ms  prefetch +-{}",
            self.stats.frame_ms, self.prefetch
        ));
        if !self.adapter_name.is_empty() {
            v.push(self.adapter_name.clone());
        }
        if let Some(e) = &self.last_error {
            v.push(format!("ERROR: {e}"));
        }
        v
    }

    /// Do what a rendered frame does to cache bookkeeping (mark drawn tiles as used) without any
    /// GPU work. Lets headless runs exercise eviction like the window does.
    pub fn simulate_frame(&mut self) {
        let _ = self.draw_list();
    }

    /// Header facts of the current layer.
    pub fn current_info(&self) -> Option<decoder::Info> {
        self.layers.get(&self.cur).map(|l| l.file.info().clone())
    }

    /// Bytes of tile textures resident across all cached layers.
    pub fn cache_bytes(&self) -> u64 {
        self.layers
            .values()
            .flat_map(|l| l.tiles.values())
            .map(|c| c.gpu.bytes())
            .sum()
    }

    /// Everything the current view needs on the current layer is resident: the overview plus
    /// the visible tiles at the zoom's level (what is drawn, ignoring prefetch margin).
    pub fn current_ready(&self) -> bool {
        let Some(layer) = self.layers.get(&self.cur) else {
            return true;
        };
        let info = layer.file.info();
        let want = level_for_scale(self.camera.scale, info.levels).min(layer.overview);
        all_tiles(info, layer.overview)
            .into_iter()
            .chain(visible_tiles(&self.camera, info, want, 0))
            .all(|k| layer.tiles.contains_key(&k) || self.failed.contains(&(self.cur, k)))
    }

    /// Resident tiles to draw, coarse first so detail overwrites the stand-in, with screen rects.
    fn draw_list(&mut self) -> (Vec<(TileKey, [f32; 4])>, bool) {
        self.frame += 1;
        let frame = self.frame;
        let cam = self.camera;
        let nearest = cam.scale >= 1.0;
        let Some(layer) = self.layers.get_mut(&self.cur) else {
            return (Vec::new(), nearest);
        };
        let info = layer.file.info().clone();
        let want = level_for_scale(cam.scale, info.levels).min(layer.overview);
        let mut out = Vec::new();
        for level in (want..=layer.overview).rev() {
            for key in visible_tiles(&cam, &info, level, 0) {
                let Some(c) = layer.tiles.get_mut(&key) else {
                    continue;
                };
                c.last_used = frame;
                let (x0, y0, x1, y1) = tile_image_rect(key, c.gpu.w, c.gpu.h);
                let (sx0, sy0) = cam.image_to_screen(x0, y0);
                let (sx1, sy1) = cam.image_to_screen(x1, y1);
                // Pixel-exact edges when magnified, so texels are square and neighbours abut.
                let snap = |v: f64| if nearest { v.round() } else { v };
                out.push((
                    key,
                    [
                        snap(sx0) as f32,
                        snap(sy0) as f32,
                        snap(sx1) as f32,
                        snap(sy1) as f32,
                    ],
                ));
            }
        }
        (out, nearest)
    }

    pub fn title(&self) -> String {
        let name = self.current_name();
        let Some(layer) = self.layers.get(&self.cur) else {
            return match &self.last_error {
                Some(e) => format!("j2k viewer - {name} - ERROR: {e}"),
                None => "j2k viewer".to_string(),
            };
        };
        let info = layer.file.info();
        let mb = self.cache_bytes() as f64 / 1e6;
        let mut s = format!(
            "{name} [{}/{}] {}x{} - {:.1}% L{} - cache {mb:.0} MB",
            self.cur + 1,
            self.files.len(),
            info.width,
            info.height,
            self.camera.scale * 100.0,
            self.current_level(),
        );
        let n = self.scheduler.outstanding();
        if n > 0 {
            s.push_str(&format!(" - decoding {n}"));
        }
        if let Some(e) = &self.last_error {
            s.push_str(&format!(" - ERROR: {e}"));
        }
        s
    }

    /// (tiles resident on the current layer, outstanding decodes) for logging.
    pub fn stats(&self) -> (usize, usize) {
        (
            self.layers.get(&self.cur).map_or(0, |l| l.tiles.len()),
            self.scheduler.outstanding(),
        )
    }
}
