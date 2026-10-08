//! macOS backend: a winit-driven strip and popup window, pinned to the right
//! edge of the configured display, always on top. Owns native window/surface
//! handles only — all app state and decision logic lives in `crate::app::Core`.
mod font;

use crate::app::{self, Backend, Core, MonitorInfo, Surface};
use anyhow::{Context, Result};
use calstack_core::config::Config;
use calstack_render::{Canvas, Palette};
use std::{
    num::NonZeroU32,
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
    time::Duration,
};
use winit::{
    application::ApplicationHandler,
    dpi::{PhysicalPosition, PhysicalSize},
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    monitor::MonitorHandle,
    platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS},
    window::{Window, WindowId, WindowLevel},
};

struct NativeWindow {
    window: Rc<Window>,
    // Must outlive `surface`; never read directly.
    _context: softbuffer::Context<Rc<Window>>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
}
impl NativeWindow {
    fn new(window: Rc<Window>) -> Result<Self> {
        let context = softbuffer::Context::new(window.clone())
            .map_err(|e| anyhow::anyhow!("softbuffer context: {e}"))?;
        let surface = softbuffer::Surface::new(&context, window.clone())
            .map_err(|e| anyhow::anyhow!("softbuffer surface: {e}"))?;
        Ok(Self {
            window,
            _context: context,
            surface,
        })
    }
    fn present(&mut self, canvas: Canvas) {
        let (Some(w), Some(h)) = (
            NonZeroU32::new(canvas.width),
            NonZeroU32::new(canvas.height),
        ) else {
            return;
        };
        if self.surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buffer) = self.surface.buffer_mut() else {
            return;
        };
        for (dst, src) in buffer.iter_mut().zip(canvas.pixels.as_chunks::<4>().0) {
            *dst = (src[2] as u32) << 16 | (src[1] as u32) << 8 | src[0] as u32;
        }
        let _ = buffer.present();
    }
}

/// Height, in physical pixels, of the menu bar on `monitor` — macOS gives
/// every display its own menu bar once it's running, and winit's monitor
/// geometry doesn't exclude it (`NSScreen.frame` vs `.visibleFrame` does).
/// Matches by physical size, which is unambiguous unless two displays of the
/// identical model and resolution are both connected.
fn menu_bar_inset(monitor: &MonitorHandle) -> u32 {
    let Some(mtm) = objc2_foundation::MainThreadMarker::new() else {
        return 0;
    };
    let target = monitor.size();
    for screen in objc2_app_kit::NSScreen::screens(mtm).iter() {
        let scale = screen.backingScaleFactor();
        let frame = screen.frame();
        let physical = PhysicalSize::new(
            (frame.size.width * scale).round() as u32,
            (frame.size.height * scale).round() as u32,
        );
        if physical == target {
            let visible = screen.visibleFrame();
            let inset_points =
                (frame.origin.y + frame.size.height) - (visible.origin.y + visible.size.height);
            return (inset_points * scale).round().max(0.0) as u32;
        }
    }
    0
}

/// winit's cross-platform `WindowLevel::AlwaysOnTop` doesn't set
/// `NSWindow.collectionBehavior`, so without this the strip is pinned to
/// whichever Space (or fullscreen app) was active when it was created,
/// instead of following the user across a three/four-finger swipe.
/// Both the popup (recreated on every hover change, since it's torn down
/// whenever the popup "kind" changes) and the strip need: visible across
/// every Space/fullscreen app, and no implicit open/resize animation —
/// AppKit's default zoom-in-from-~95% window-open effect would otherwise
/// replay each time the popup window is recreated.
fn configure_window(window: &Window) {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    let view = handle.ns_view.as_ptr() as *const objc2_app_kit::NSView;
    let Some(ns_window) = (unsafe { &*view }).window() else {
        return;
    };
    unsafe {
        ns_window.setCollectionBehavior(
            objc2_app_kit::NSWindowCollectionBehavior::CanJoinAllSpaces
                | objc2_app_kit::NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        ns_window.setAnimationBehavior(objc2_app_kit::NSWindowAnimationBehavior::None);
    }
}

/// `NSWindowAnimationBehavior::None` only suppresses AppKit's own
/// minimize/zoom-style transitions; a layer-backed window's first order-in
/// can still pick up Core Animation's implicit "materialize" action on its
/// layer. Wrapping the order-in itself in a transaction with actions
/// disabled suppresses that too.
fn show_without_animation(window: &Window) {
    objc2_quartz_core::CATransaction::begin();
    objc2_quartz_core::CATransaction::setDisableActions(true);
    window.set_visible(true);
    objc2_quartz_core::CATransaction::commit();
}

fn resolve_monitor(window: &Window, wanted: &str) -> Option<MonitorHandle> {
    let monitors: Vec<_> = window.available_monitors().collect();
    monitors
        .iter()
        .find(|m| m.name().as_deref() == Some(wanted))
        .cloned()
        .or_else(|| window.current_monitor())
        .or_else(|| monitors.into_iter().next())
}

struct MacSurfaces {
    strip: NativeWindow,
    popup: Option<NativeWindow>,
    pending_popup_request: Option<(u32, u32, f32)>,
    pending_strip_size: Option<(u32, u32)>,
    pending_popup_size: Option<(u32, u32)>,
    strip_cursor: (f32, f32),
    popup_cursor: (f32, f32),
    // Settings runs as a separate process (its own winit/eframe event loop
    // can't share this one); tracked so Quit closes it too instead of
    // leaving it orphaned after the strip exits.
    settings_child: Option<std::process::Child>,
}
impl MacSurfaces {
    /// Quit should take the settings window down with it, not leave it
    /// running as an orphan once the strip that spawned it is gone.
    fn kill_settings_window(&mut self) {
        if let Some(mut child) = self.settings_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    /// Create the deferred popup window requested by `show_popup`. Window
    /// creation needs an `ActiveEventLoop`, which `Backend::show_popup`
    /// doesn't have access to, so the request is drained here instead, from
    /// inside an `ApplicationHandler` callback.
    fn drain_popup_request(&mut self, event_loop: &ActiveEventLoop) {
        let Some((width, height, y)) = self.pending_popup_request.take() else {
            return;
        };
        let scale = self.strip.window.scale_factor();
        let physical = PhysicalSize::new(
            (width as f64 * scale).round() as u32,
            (height as f64 * scale).round() as u32,
        );
        let strip_pos = self
            .strip
            .window
            .outer_position()
            .unwrap_or(PhysicalPosition::new(0, 0));
        let position = PhysicalPosition::new(
            strip_pos.x - physical.width as i32,
            strip_pos.y + (y as f64 * scale).round() as i32,
        );
        match &mut self.popup {
            Some(native) => {
                let _ = native.window.request_inner_size(physical);
                native.window.set_outer_position(position);
            }
            None => {
                // Configure (in particular, disable the implicit open
                // animation) before the window is ever made visible —
                // AppKit decides whether to animate an order-front at the
                // moment it happens, so setting this after `with_visible(true)`
                // has already shown the window is too late.
                let attrs = Window::default_attributes()
                    .with_title("")
                    .with_decorations(false)
                    .with_resizable(false)
                    .with_visible(false)
                    .with_inner_size(physical)
                    .with_position(position);
                let Ok(window) = event_loop.create_window(attrs) else {
                    return;
                };
                window.set_window_level(WindowLevel::AlwaysOnTop);
                configure_window(&window);
                show_without_animation(&window);
                let window = Rc::new(window);
                let Ok(native) = NativeWindow::new(window) else {
                    return;
                };
                self.popup = Some(native);
            }
        }
        let actual = self
            .popup
            .as_ref()
            .map(|n| n.window.inner_size())
            .unwrap_or(physical);
        self.pending_popup_size = Some((
            (actual.width as f64 / scale).round() as u32,
            (actual.height as f64 / scale).round() as u32,
        ));
    }
}
impl Backend for MacSurfaces {
    fn ensure_strip(&mut self, config: &Config) -> bool {
        let Some(monitor) = resolve_monitor(&self.strip.window, &config.display.monitor) else {
            return false;
        };
        let scale = monitor.scale_factor();
        let width_physical = (config.display.width as f64 * scale).round() as u32;
        let inset = menu_bar_inset(&monitor);
        let size = PhysicalSize::new(width_physical, monitor.size().height.saturating_sub(inset));
        let _ = self.strip.window.request_inner_size(size);
        self.strip.window.set_outer_position(PhysicalPosition::new(
            monitor.position().x + monitor.size().width as i32 - width_physical as i32,
            monitor.position().y + inset as i32,
        ));
        let actual = self.strip.window.inner_size();
        self.pending_strip_size = Some((
            (actual.width as f64 / scale).round() as u32,
            (actual.height as f64 / scale).round() as u32,
        ));
        false
    }
    fn present_strip(&mut self, canvas: Canvas) {
        self.strip.present(canvas);
    }
    fn show_popup(&mut self, width: u32, height: u32, y: f32) {
        self.pending_popup_request = Some((width, height, y));
    }
    fn present_popup(&mut self, canvas: Canvas) {
        if let Some(popup) = &mut self.popup {
            popup.present(canvas);
        }
    }
    fn close_popup(&mut self) {
        self.popup = None;
        self.pending_popup_request = None;
        self.pending_popup_size = None;
    }
    fn open_url(&self, target: &str) {
        app::spawn_detached(Command::new("open").arg(target));
    }
    fn open_settings(&mut self, config_path: &Path) {
        if self
            .settings_child
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
        {
            return; // already open; avoid spawning a second window
        }
        match app::settings_command(config_path).and_then(|mut command| command.spawn()) {
            Ok(child) => self.settings_child = Some(child),
            Err(error) => tracing::warn!(%error, "could not open settings"),
        }
    }
    fn is_light(&self, _config: &Config) -> bool {
        Command::new("defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output()
            .map(|out| {
                !(out.status.success() && String::from_utf8_lossy(&out.stdout).contains("Dark"))
            })
            .unwrap_or(true)
    }
    fn live_palette(&self, _config: &Config) -> Option<Palette> {
        None
    }
    fn poll_font_settings(&mut self) -> Option<calstack_render::TextRenderer> {
        None
    }
    fn hover_delay(&self) -> Duration {
        Duration::ZERO
    }
    fn list_monitors(&self) -> Vec<MonitorInfo> {
        let primary = self.strip.window.primary_monitor();
        self.strip
            .window
            .available_monitors()
            .map(|m| MonitorInfo {
                name: m.name().unwrap_or_default(),
                width: m.size().width,
                height: m.size().height,
                x: m.position().x,
                y: m.position().y,
                primary: primary.as_ref() == Some(&m),
            })
            .collect()
    }
}
fn surface_of(surfaces: &MacSurfaces, id: WindowId) -> Option<Surface> {
    if surfaces.strip.window.id() == id {
        Some(Surface::Strip)
    } else if surfaces.popup.as_ref().is_some_and(|p| p.window.id() == id) {
        Some(Surface::Popup)
    } else {
        None
    }
}

/// Apply the geometry/size changes queued by `Core` calls since the last
/// drain: creating/moving the popup window, and reporting resolved sizes
/// back into `Core` via `strip_configured`/`popup_configured`.
fn drain(core: &mut Core, surfaces: &mut MacSurfaces, event_loop: &ActiveEventLoop) {
    surfaces.drain_popup_request(event_loop);
    if let Some((w, h)) = surfaces.pending_popup_size.take() {
        core.popup_configured(surfaces, w, h);
    }
    if let Some((w, h)) = surfaces.pending_strip_size.take() {
        core.strip_configured(surfaces, w, h);
    }
}

struct MacApp {
    config: Config,
    config_path: PathBuf,
    demo: bool,
    state: Option<(Core, MacSurfaces)>,
}
impl ApplicationHandler for MacApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Calstack")
            .with_decorations(false)
            .with_resizable(false)
            .with_visible(false)
            .with_inner_size(PhysicalSize::new(self.config.display.width.max(1), 1));
        let window = match event_loop.create_window(attrs) {
            Ok(window) => Rc::new(window),
            Err(error) => {
                tracing::error!(%error, "could not create strip window");
                event_loop.exit();
                return;
            }
        };
        window.set_window_level(WindowLevel::AlwaysOnTop);
        configure_window(&window);
        show_without_animation(&window);
        let strip = match NativeWindow::new(window) {
            Ok(native) => native,
            Err(error) => {
                tracing::error!(%error, "could not create strip surface");
                event_loop.exit();
                return;
            }
        };
        let mut surfaces = MacSurfaces {
            strip,
            popup: None,
            pending_popup_request: None,
            pending_strip_size: None,
            pending_popup_size: None,
            strip_cursor: (0.0, 0.0),
            popup_cursor: (0.0, 0.0),
            settings_child: None,
        };
        let palette = app::palette_for(&surfaces, &self.config);
        let text = match font::renderer() {
            Ok(text) => text,
            Err(error) => {
                tracing::error!(%error, "could not load bundled font");
                event_loop.exit();
                return;
            }
        };
        let mut core = Core::new(
            self.config.clone(),
            self.config_path.clone(),
            self.demo,
            text,
            palette,
        );
        core.start(&mut surfaces);
        drain(&mut core, &mut surfaces, event_loop);
        event_loop.set_control_flow(ControlFlow::WaitUntil(core.deadline()));
        self.state = Some((core, surfaces));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some((core, surfaces)) = &mut self.state else {
            return;
        };
        let Some(surface) = surface_of(surfaces, id) else {
            return;
        };
        let scale = surfaces.strip.window.scale_factor();
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                let (x, y) = ((position.x / scale) as f32, (position.y / scale) as f32);
                match surface {
                    Surface::Strip => surfaces.strip_cursor = (x, y),
                    Surface::Popup => surfaces.popup_cursor = (x, y),
                }
                core.pointer_moved(surfaces, surface, x, y);
            }
            WindowEvent::CursorLeft { .. } => core.pointer_left(surface),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                let (x, y) = match surface {
                    Surface::Strip => surfaces.strip_cursor,
                    Surface::Popup => surfaces.popup_cursor,
                };
                core.pointer_pressed(surfaces, surface, x, y);
            }
            WindowEvent::CloseRequested => core.exit = true,
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                core.scale_changed(
                    surfaces,
                    surface,
                    scale_factor.round().clamp(1.0, 4.0) as u32,
                );
                if surface == Surface::Strip {
                    surfaces.ensure_strip(&core.config);
                }
            }
            _ => {}
        }
        drain(core, surfaces, event_loop);
        if core.exit {
            surfaces.kill_settings_window();
            event_loop.exit();
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(core.deadline()));
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some((core, surfaces)) = &mut self.state else {
            return;
        };
        core.run_timers(surfaces);
        drain(core, surfaces, event_loop);
        if core.exit {
            surfaces.kill_settings_window();
            event_loop.exit();
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(core.deadline()));
        }
    }
}

struct MonitorProbe(Vec<MonitorInfo>);
impl ApplicationHandler for MonitorProbe {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let primary = event_loop.primary_monitor();
        self.0 = event_loop
            .available_monitors()
            .map(|m| MonitorInfo {
                name: m.name().unwrap_or_default(),
                width: m.size().width,
                height: m.size().height,
                x: m.position().x,
                y: m.position().y,
                primary: primary.as_ref() == Some(&m),
            })
            .collect();
        event_loop.exit();
    }
    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}
pub fn list_monitors() -> Result<Vec<MonitorInfo>> {
    let event_loop = EventLoop::builder()
        .with_activation_policy(ActivationPolicy::Prohibited)
        .build()
        .context("create event loop")?;
    let mut probe = MonitorProbe(Vec::new());
    event_loop.run_app(&mut probe).context("run event loop")?;
    Ok(probe.0)
}

pub fn run(config: Config, config_path: PathBuf, demo: bool) -> Result<()> {
    let event_loop = EventLoop::builder()
        .with_activation_policy(ActivationPolicy::Accessory)
        .build()
        .context("create event loop")?;
    let mut app = MacApp {
        config,
        config_path,
        demo,
        state: None,
    };
    tracing::info!("Calstack started; bottom \u{22ee} menu has Refresh / Settings / Quit");
    event_loop.run_app(&mut app).context("run event loop")
}
