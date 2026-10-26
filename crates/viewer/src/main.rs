// A GUI program on Windows, so double-clicking it does not open a console window. When it is run
// from a terminal, `attach_parent_console` reconnects stdout/stderr to that terminal so
// `--snapshot`, `--bench` and error messages still print.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod bench;
mod files;
mod overlay;
mod renderer;
mod scheduler;
mod view;
mod viewer;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use renderer::{Renderer, SNAPSHOT_FORMAT};
use scheduler::Scheduler;
use viewer::{UiAction, Viewer};

/// The application icon for the window and taskbar. On Windows the same icon is also embedded in
/// the executable (see `build.rs`); this covers X11 and other platforms.
fn window_icon() -> Option<winit::window::Icon> {
    static ICON_PNG: &[u8] = include_bytes!("../../../assets/icon-256.png");
    let mut reader = png::Decoder::new(std::io::Cursor::new(ICON_PNG))
        .read_info()
        .ok()?;
    let mut rgba = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut rgba).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    rgba.truncate(info.buffer_size());
    winit::window::Icon::from_rgba(rgba, info.width, info.height).ok()
}

/// Messages from other threads to the window's event loop.
enum AppEvent {
    /// A decode worker finished a tile.
    Wake,
    /// The file/folder dialog closed (`None` = cancelled).
    Opened(Option<PathBuf>),
}

const USAGE: &str = "\
usage: viewer [OPTIONS] [file-or-folder]

With no file or folder the window opens empty; use the Open button (or the O key).

Keys: Left/Right/PageUp/PageDown = previous/next layer, Home/End = first/last,
      F = fit, 1 = 100%, +/- = zoom, I = status panel, O = open (pick an image; its folder
      becomes the layer list), Shift+O = pick a folder directly, Esc = quit.
Mouse: wheel = zoom, left-drag or hold the wheel button = pan, Menu button (top right) = Open and
debug info.

options:
  --workers N        decode threads (default: cores/4, 2..6)
  --cache-mb N       GPU tile cache budget in MB (default 1024)
  --prefetch N       layers to prefetch on each side (default 1)
  --snapshot FILE    render one frame offscreen to a PNG and exit
  --size WxH         snapshot size (default 1600x900)
  --zoom S           snapshot/bench: screen px per image px (default: fit / 0.25)
  --at X,Y           snapshot: image point at the centre of the view
  --overlay          snapshot: include the status panel
  --bench            benchmark layer stepping, panning and the cache on a folder
  --steps N          bench: layer steps per prefetch setting (default 6)
  --pan N            bench: horizontal pan px per 60 Hz frame (default 48)
";

#[derive(Default)]
struct Args {
    path: Option<PathBuf>,
    workers: Option<usize>,
    cache_mb: Option<u64>,
    snapshot: Option<PathBuf>,
    size: Option<(u32, u32)>,
    zoom: Option<f64>,
    at: Option<(f64, f64)>,
    prefetch: Option<usize>,
    overlay: bool,
    bench: bool,
    steps: Option<usize>,
    pan: Option<f64>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        let bad = |name: &str, v: &str| format!("bad value for {name}: {v}");
        match arg.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--workers" => {
                let v = value("--workers")?;
                a.workers = Some(v.parse().map_err(|_| bad("--workers", &v))?);
            }
            "--cache-mb" => {
                let v = value("--cache-mb")?;
                a.cache_mb = Some(v.parse().map_err(|_| bad("--cache-mb", &v))?);
            }
            "--prefetch" => {
                let v = value("--prefetch")?;
                a.prefetch = Some(v.parse().map_err(|_| bad("--prefetch", &v))?);
            }
            "--steps" => {
                let v = value("--steps")?;
                a.steps = Some(v.parse().map_err(|_| bad("--steps", &v))?);
            }
            "--pan" => {
                let v = value("--pan")?;
                a.pan = Some(v.parse().map_err(|_| bad("--pan", &v))?);
            }
            "--overlay" => a.overlay = true,
            "--bench" => a.bench = true,
            "--snapshot" => a.snapshot = Some(value("--snapshot")?.into()),
            "--size" => {
                let v = value("--size")?;
                let (w, h) = v.split_once('x').ok_or_else(|| bad("--size", &v))?;
                a.size = Some((
                    w.parse().map_err(|_| bad("--size", &v))?,
                    h.parse().map_err(|_| bad("--size", &v))?,
                ));
            }
            "--zoom" => {
                let v = value("--zoom")?;
                a.zoom = Some(v.parse().map_err(|_| bad("--zoom", &v))?);
            }
            "--at" => {
                let v = value("--at")?;
                let (x, y) = v.split_once(',').ok_or_else(|| bad("--at", &v))?;
                a.at = Some((
                    x.parse().map_err(|_| bad("--at", &v))?,
                    y.parse().map_err(|_| bad("--at", &v))?,
                ));
            }
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            _ => a.path = Some(arg.into()),
        }
    }
    Ok(a)
}

fn default_workers() -> usize {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    (cores / 4).clamp(2, 6)
}

/// Prefer a discrete GPU, then any GPU, then a software adapter, so the app runs everywhere.
fn request_adapter(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'_>>,
) -> Result<wgpu::Adapter, String> {
    let attempts = [
        (wgpu::PowerPreference::HighPerformance, false),
        (wgpu::PowerPreference::LowPower, false),
        (wgpu::PowerPreference::None, true),
    ];
    for (power_preference, force_fallback_adapter) in attempts {
        let options = wgpu::RequestAdapterOptions {
            power_preference,
            force_fallback_adapter,
            compatible_surface: surface,
            ..Default::default()
        };
        if let Ok(adapter) = pollster::block_on(instance.request_adapter(&options)) {
            return Ok(adapter);
        }
    }
    Err("no usable GPU or software adapter found".into())
}

fn request_device(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue), String> {
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("viewer"),
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .map_err(|e| format!("device: {e}"))
}

/// Re-attach stdout/stderr to the terminal that launched us (no-op when started from Explorer,
/// which has no console to attach to).
#[cfg(windows)]
fn attach_parent_console() {
    use std::os::windows::io::AsRawHandle;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
        fn GetStdHandle(std_handle: u32) -> *mut std::ffi::c_void;
        fn SetStdHandle(std_handle: u32, handle: *mut std::ffi::c_void) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;

    // SAFETY: plain Win32 calls with valid arguments; the opened handles are leaked on purpose
    // so they stay valid for the life of the process.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }
        for std_handle in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // Keep a handle we were given (output piped or redirected to a file); only fill in
            // the missing ones with the terminal.
            let existing = GetStdHandle(std_handle) as isize;
            if existing != 0 && existing != -1 {
                continue;
            }
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
                SetStdHandle(std_handle, f.as_raw_handle().cast());
                std::mem::forget(f);
            }
        }
    }
}

fn main() {
    #[cfg(windows)]
    attach_parent_console();
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("error: {e}\n");
            }
            eprint!("{USAGE}");
            std::process::exit(if e.is_empty() { 0 } else { 2 });
        }
    };
    let (files, index) = match args.path.clone() {
        Some(path) => match files::resolve(&path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        },
        // The window can start empty and let the user pick something. (A snapshot with no file
        // renders that empty screen; only the benchmark needs real input.)
        None if !args.bench => (Vec::new(), 0),
        None => {
            eprint!("{USAGE}");
            std::process::exit(2);
        }
    };
    let result = if args.bench {
        run_bench(&args, files, index)
    } else if args.snapshot.is_some() {
        run_snapshot(&args, files, index)
    } else {
        run_window(&args, files, index)
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------- headless snapshot

/// GPU device without a window, plus a human-readable adapter description.
fn headless_gpu() -> Result<(wgpu::Device, wgpu::Queue, String), String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = request_adapter(&instance, None)?;
    let info = adapter.get_info();
    let name = format!("{} ({:?}, {:?})", info.name, info.device_type, info.backend);
    println!("adapter: {name}");
    let (device, queue) = request_device(&adapter)?;
    Ok((device, queue, name))
}

fn run_bench(args: &Args, files: Vec<PathBuf>, index: usize) -> Result<(), String> {
    let size = args.size.unwrap_or((1600, 900));
    let (device, queue, name) = headless_gpu()?;
    let workers = args.workers.unwrap_or_else(default_workers);
    println!(
        "{} layers, {workers} decode workers, view {}x{}\n",
        files.len(),
        size.0,
        size.1
    );
    let mk = |prefetch: usize| {
        let renderer = Renderer::new(device.clone(), queue.clone(), SNAPSHOT_FORMAT);
        let scheduler = Scheduler::new(workers, || {});
        let mut v = Viewer::new(renderer, scheduler, files.clone(), index, size);
        v.prefetch = prefetch;
        v.adapter_name = name.clone();
        v
    };
    bench::run(
        &mk,
        &bench::Opts {
            steps: args.steps.unwrap_or(6),
            zoom: args.zoom.unwrap_or(0.25),
            cache_mb: args.cache_mb.unwrap_or(128),
            pan: args.pan.unwrap_or(48.0),
        },
    );
    Ok(())
}

fn run_snapshot(args: &Args, files: Vec<PathBuf>, index: usize) -> Result<(), String> {
    let out = args.snapshot.clone().unwrap();
    let size = args.size.unwrap_or((1600, 900));
    let (device, queue, name) = headless_gpu()?;
    let renderer = Renderer::new(device, queue, SNAPSHOT_FORMAT);
    let scheduler = Scheduler::new(args.workers.unwrap_or_else(default_workers), || {});
    let mut viewer = Viewer::new(renderer, scheduler, files, index, size);
    viewer.adapter_name = name;
    viewer.overlay = args.overlay;
    viewer.show_buttons = args.overlay;
    viewer.overlay_scale = if size.0 >= 2500 { 2 } else { 1 };
    if let Some(p) = args.prefetch {
        viewer.prefetch = p;
    }
    if let Some(mb) = args.cache_mb {
        viewer.set_cache_budget(mb << 20);
    }
    if args.zoom.is_some() || args.at.is_some() {
        viewer.set_view(args.at.map(|p| p.0), args.at.map(|p| p.1), args.zoom);
    }
    let start = Instant::now();
    loop {
        viewer.update();
        if viewer.is_settled() {
            break;
        }
        if start.elapsed() > Duration::from_secs(300) {
            return Err("timed out waiting for tiles".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let (tiles, _) = viewer.stats();
    println!(
        "{} | {tiles} tiles decoded in {:.0} ms",
        viewer.title(),
        start.elapsed().as_secs_f64() * 1e3
    );
    let t = Instant::now();
    let rgba = viewer.render_to_rgba(size);
    println!(
        "rendered {}x{} in {:.1} ms",
        size.0,
        size.1,
        t.elapsed().as_secs_f64() * 1e3
    );
    let file = std::fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), size.0, size.1);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut w| w.write_image_data(&rgba))
        .map_err(|e| e.to_string())?;
    println!("wrote {}", out.display());
    Ok(())
}

// ---------------------------------------------------------------- window

struct Gfx {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    viewer: Viewer,
}

struct App {
    args: Args,
    files: Option<(Vec<PathBuf>, usize)>,
    proxy: EventLoopProxy<AppEvent>,
    gfx: Option<Gfx>,
    cursor: (f64, f64),
    dragging: bool,
    error: Option<String>,
    /// A native dialog is open (it runs on its own thread); ignore further open requests.
    dialog_open: bool,
    /// Where the next dialog starts.
    last_dir: Option<PathBuf>,
}

fn run_window(args: &Args, files: Vec<PathBuf>, index: usize) -> Result<(), String> {
    let event_loop = EventLoop::<AppEvent>::with_user_event()
        .build()
        .map_err(|e| e.to_string())?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App {
        args: Args {
            workers: args.workers,
            cache_mb: args.cache_mb,
            prefetch: args.prefetch,
            ..Default::default()
        },
        files: Some((files, index)),
        proxy: event_loop.create_proxy(),
        gfx: None,
        cursor: (0.0, 0.0),
        dragging: false,
        error: None,
        dialog_open: false,
        last_dir: None,
    };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    app.error.map_or(Ok(()), Err)
}

impl App {
    fn init(&mut self, el: &ActiveEventLoop) -> Result<(), String> {
        let attrs = Window::default_attributes()
            .with_title("j2k viewer")
            .with_window_icon(window_icon())
            .with_inner_size(LogicalSize::new(1600.0, 900.0));
        let window = Arc::new(el.create_window(attrs).map_err(|e| e.to_string())?);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| e.to_string())?;
        let adapter = request_adapter(&instance, Some(&surface))?;
        let info = adapter.get_info();
        eprintln!(
            "adapter: {} ({:?}, {:?})",
            info.name, info.device_type, info.backend
        );
        let (device, queue) = request_device(&adapter)?;

        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface not supported by adapter")?;
        // Grayscale values are already display-referred: avoid an sRGB surface, which would
        // encode them a second time and wash the image out.
        let caps = surface.get_capabilities(&adapter);
        if let Some(f) = caps.formats.iter().copied().find(|f| !f.is_srgb()) {
            config.format = f;
        }
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);

        let proxy = self.proxy.clone();
        let scheduler = Scheduler::new(
            self.args.workers.unwrap_or_else(default_workers),
            move || {
                let _ = proxy.send_event(AppEvent::Wake);
            },
        );
        let renderer = Renderer::new(device, queue, config.format);
        let (files, index) = self.files.take().ok_or("already initialised")?;
        let mut viewer = Viewer::new(renderer, scheduler, files, index, (size.width, size.height));
        if let Some(mb) = self.args.cache_mb {
            viewer.set_cache_budget(mb << 20);
        }
        if let Some(p) = self.args.prefetch {
            viewer.prefetch = p;
        }
        // The debug panel starts hidden; the Menu button (or I) turns it on.
        viewer.overlay = false;
        // Bitmap text is fixed-size, so scale it up on high-DPI settings and on very wide monitors.
        let wide = window
            .current_monitor()
            .is_some_and(|m| m.size().width >= 2500);
        viewer.overlay_scale = (window.scale_factor().round() as u32)
            .max(if wide { 2 } else { 1 })
            .clamp(1, 3);
        viewer.adapter_name = format!("{} ({:?})", info.name, info.backend);
        window.request_redraw();
        self.gfx = Some(Gfx {
            window,
            surface,
            config,
            viewer,
        });
        Ok(())
    }

    fn redraw(&mut self) {
        let Some(g) = self.gfx.as_mut() else { return };
        let more = g.viewer.update();
        match g.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                let view = frame
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                g.viewer.render(&view, (g.config.width, g.config.height));
                g.window.pre_present_notify();
                g.viewer.renderer.queue.present(frame);
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                g.surface.configure(&g.viewer.renderer.device, &g.config);
                g.window.request_redraw();
            }
            _ => {}
        }
        g.window.set_title(&g.viewer.title());
        if more {
            g.window.request_redraw();
        }
    }

    /// Show the native file or folder picker. It runs on its own thread so the window keeps
    /// redrawing, and reports back through `AppEvent::Opened`.
    fn open_dialog(&mut self, folder: bool) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;
        let proxy = self.proxy.clone();
        let start = self.last_dir.clone();
        std::thread::spawn(move || {
            let mut dialog = rfd::FileDialog::new();
            if let Some(dir) = start {
                dialog = dialog.set_directory(dir);
            }
            let picked = if folder {
                dialog.set_title("Open folder of layers").pick_folder()
            } else {
                dialog
                    .set_title("Open image")
                    .add_filter(
                        "JPEG 2000 / HTJ2K",
                        &["jp2", "j2k", "j2c", "jph", "jhc", "jpc"],
                    )
                    .add_filter("All files", &["*"])
                    .pick_file()
            };
            let _ = proxy.send_event(AppEvent::Opened(picked));
        });
    }

    fn open_path(&mut self, path: &std::path::Path) {
        let Some(g) = self.gfx.as_mut() else { return };
        self.last_dir = if path.is_dir() {
            Some(path.to_path_buf())
        } else {
            path.parent().map(PathBuf::from)
        };
        match files::resolve(path) {
            Ok((files, index)) => g.viewer.set_files(files, index),
            Err(e) => g.viewer.last_error = Some(e),
        }
        g.window.request_redraw();
    }

    fn run_action(&mut self, action: UiAction) {
        let Some(g) = self.gfx.as_mut() else { return };
        let for_window = g.viewer.activate(action);
        g.window.request_redraw();
        if for_window == Some(UiAction::OpenFile) {
            self.open_dialog(false);
        }
    }

    fn key(&mut self, el: &ActiveEventLoop, key: &Key) {
        // "o" opens a file, "O" (Shift+O) a folder.
        if let Key::Character(s) = key {
            match s.as_str() {
                "o" => return self.open_dialog(false),
                "O" => return self.open_dialog(true),
                _ => {}
            }
        }
        let Some(g) = self.gfx.as_mut() else { return };
        let (cx, cy) = (g.config.width as f64 / 2.0, g.config.height as f64 / 2.0);
        match key {
            Key::Named(NamedKey::Escape) if g.viewer.menu_open() => g.viewer.close_menu(),
            Key::Named(NamedKey::Escape) => el.exit(),
            Key::Named(NamedKey::ArrowRight | NamedKey::PageDown) => g.viewer.step(1),
            Key::Named(NamedKey::ArrowLeft | NamedKey::PageUp) => g.viewer.step(-1),
            Key::Named(NamedKey::Home) => g.viewer.open_index(0),
            Key::Named(NamedKey::End) => {
                g.viewer.open_index(g.viewer.file_count().saturating_sub(1))
            }
            Key::Character(s) => match s.as_str() {
                "f" | "F" => g.viewer.fit(),
                "1" => g.viewer.actual_size(),
                "i" | "I" => g.viewer.overlay = !g.viewer.overlay,
                "+" | "=" => g.viewer.zoom_at(cx, cy, 1.25),
                "-" | "_" => g.viewer.zoom_at(cx, cy, 0.8),
                "q" | "Q" => el.exit(),
                _ => return,
            },
            _ => return,
        }
        g.window.request_redraw();
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gfx.is_none() && self.files.is_some() {
            if let Err(e) = self.init(el) {
                self.error = Some(e);
                el.exit();
            }
        }
    }

    fn user_event(&mut self, _: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Wake => {}
            AppEvent::Opened(picked) => {
                self.dialog_open = false;
                if let Some(path) = picked {
                    self.open_path(&path);
                }
            }
        }
        if let Some(g) = &self.gfx {
            g.window.request_redraw();
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => {
                if let Some(g) = self
                    .gfx
                    .as_mut()
                    .filter(|_| size.width > 0 && size.height > 0)
                {
                    g.config.width = size.width;
                    g.config.height = size.height;
                    g.surface.configure(&g.viewer.renderer.device, &g.config);
                    g.viewer.resize(size.width, size.height);
                    g.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::CursorMoved { position, .. } => {
                let (dx, dy) = (position.x - self.cursor.0, position.y - self.cursor.1);
                self.cursor = (position.x, position.y);
                if let Some(g) = self.gfx.as_mut() {
                    if self.dragging {
                        g.viewer.pan(dx, dy);
                        g.window.request_redraw();
                    }
                    let over = g.viewer.button_at(position.x, position.y);
                    if g.viewer.set_hover(over) {
                        g.window.set_cursor(if over.is_some() {
                            CursorIcon::Pointer
                        } else {
                            CursorIcon::Default
                        });
                        g.window.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                match button {
                    // Holding the wheel button down pans, same as dragging with the left button.
                    MouseButton::Middle => self.dragging = pressed,
                    MouseButton::Left => {
                        let clicked = self
                            .gfx
                            .as_ref()
                            .and_then(|g| g.viewer.button_at(self.cursor.0, self.cursor.1));
                        match clicked {
                            // Act on release so a click that moves off the button can be abandoned.
                            Some((_, action)) if !pressed => self.run_action(action),
                            Some(_) => {}
                            None => {
                                if pressed {
                                    // A click anywhere else dismisses an open menu.
                                    if let Some(g) =
                                        self.gfx.as_mut().filter(|g| g.viewer.menu_open())
                                    {
                                        g.viewer.close_menu();
                                        g.window.request_redraw();
                                    }
                                }
                                self.dragging = pressed;
                            }
                        }
                        if !pressed {
                            self.dragging = false;
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => f64::from(y),
                    MouseScrollDelta::PixelDelta(p) => p.y / 60.0,
                };
                if let Some(g) = self.gfx.as_mut() {
                    g.viewer
                        .zoom_at(self.cursor.0, self.cursor.1, 1.2f64.powf(lines));
                    g.window.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                self.key(el, &event.logical_key);
            }
            WindowEvent::DroppedFile(path) => self.open_path(&path),
            _ => {}
        }
    }
}
