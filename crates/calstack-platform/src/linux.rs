//! Linux backend: wlr-layer-shell strip and popup surfaces on a Wayland
//! compositor. Owns native surface handles only — all app state and decision
//! logic lives in `crate::app::Core`.
mod theme;
mod typography;

use crate::app::{self, Backend, Core, MonitorInfo, Surface};
use anyhow::{Context, Result};
use calstack_core::config::Config;
use calstack_render::{Canvas, Palette, TextRenderer};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    reexports::{calloop::EventLoop, calloop_wayland_source::WaylandSource},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, QueueHandle,
};

/// Wayland globals and the strip/popup layer surfaces.
struct Wayland {
    registry: RegistryState,
    seats: SeatState,
    outputs: OutputState,
    compositor: CompositorState,
    shell: LayerShell,
    shm: Shm,
    pool: SlotPool,
    qh: QueueHandle<App>,
    output: Option<wl_output::WlOutput>,
    strip: Option<LayerSurface>,
    // Last size/exclusive zone requested of the strip, to skip no-op commits.
    strip_request: (u32, bool),
    // Strip width as configured by the compositor; positions the popup.
    strip_width: u32,
    pointer: Option<wl_pointer::WlPointer>,
    popup: Option<LayerSurface>,
    font_settings: typography::FontSettings,
    // Last strip height the compositor assigned, and a pending re-place. When
    // the strip grows (a bar above/below went away), we re-create it after a
    // short settle so a returning bar keeps its full width instead of being
    // shrunk by our exclusive zone.
    strip_height: u32,
    settle_due: Option<Instant>,
}

/// How long to wait after the strip grows before re-creating it.
const STRIP_SETTLE: Duration = Duration::from_millis(2500);
struct App {
    core: Core,
    wl: Wayland,
}

impl Wayland {
    /// Put the strip on the configured output (or keep it where it is), and
    /// return whether it had to be recreated.
    fn place_strip(&mut self, config: &Config, removed: Option<&wl_output::WlOutput>) -> bool {
        let outputs: Vec<_> = self
            .outputs
            .outputs()
            .filter(|o| Some(o) != removed)
            .collect();
        let desired = outputs
            .iter()
            .find(|o| {
                self.outputs.info(o).and_then(|i| i.name).as_deref()
                    == Some(&config.display.monitor)
            })
            .or_else(|| outputs.iter().find(|o| Some(*o) == self.output.as_ref()))
            .or_else(|| outputs.first())
            .cloned();
        let request = (config.display.width, config.display.reserve_space);
        if self.output == desired {
            if let Some(layer) = &self.strip {
                if self.strip_request != request {
                    self.strip_request = request;
                    Self::size_strip(layer, request);
                    layer.commit();
                }
                return false;
            }
        }
        self.popup = None;
        self.strip = None;
        self.output = desired;
        self.strip_width = config.display.width;
        if self.output.is_some() {
            let layer = self.shell.create_layer_surface(
                &self.qh,
                self.compositor.create_surface(&self.qh),
                Layer::Top,
                Some("calstack"),
                self.output.as_ref(),
            );
            layer.set_anchor(Anchor::TOP | Anchor::RIGHT | Anchor::BOTTOM);
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            self.strip_request = request;
            Self::size_strip(&layer, request);
            layer.commit();
            self.strip = Some(layer);
        }
        true
    }
    fn size_strip(layer: &LayerSurface, (width, reserve_space): (u32, bool)) {
        layer.set_size(width, 0);
        layer.set_exclusive_zone(if reserve_space { width as i32 } else { 0 });
    }
    /// Track the strip's compositor-assigned height. A growth means the layout
    /// above/below changed — typically a top bar disappearing — so schedule a
    /// one-shot re-place. A shrink means the bar is accounted for again.
    fn note_strip_height(&mut self, height: u32) {
        if height == 0 {
            return;
        }
        if height > self.strip_height {
            self.settle_due = Some(Instant::now() + STRIP_SETTLE);
        } else if height < self.strip_height {
            self.settle_due = None;
        }
        self.strip_height = height;
    }
    fn present(&mut self, surface: &wl_surface::WlSurface, canvas: Canvas) {
        let (buffer, pixels) = self
            .pool
            .create_buffer(
                canvas.width as i32,
                canvas.height as i32,
                canvas.width as i32 * 4,
                wl_shm::Format::Argb8888,
            )
            .expect("allocate shared-memory buffer");
        // SlotPool may round its allocation up; only the image bytes are pixels.
        pixels[..canvas.pixels.len()].copy_from_slice(&canvas.pixels);
        surface.set_buffer_scale(canvas.scale.round() as i32);
        surface.damage_buffer(0, 0, canvas.width as i32, canvas.height as i32);
        buffer
            .attach_to(surface)
            .expect("attach shared-memory buffer");
        surface.commit();
    }
    fn surface_of(&self, surface: &wl_surface::WlSurface) -> Option<Surface> {
        if self
            .strip
            .as_ref()
            .is_some_and(|l| l.wl_surface() == surface)
        {
            Some(Surface::Strip)
        } else if self
            .popup
            .as_ref()
            .is_some_and(|l| l.wl_surface() == surface)
        {
            Some(Surface::Popup)
        } else {
            None
        }
    }
}

impl Backend for Wayland {
    fn ensure_strip(&mut self, config: &Config) -> bool {
        self.place_strip(config, None)
    }
    fn present_strip(&mut self, canvas: Canvas) {
        if let Some(layer) = self.strip.clone() {
            self.present(layer.wl_surface(), canvas);
        }
    }
    fn show_popup(&mut self, width: u32, height: u32, y: f32) {
        let layer = self.shell.create_layer_surface(
            &self.qh,
            self.compositor.create_surface(&self.qh),
            Layer::Top,
            Some("calstack-popup"),
            self.output.as_ref(),
        );
        layer.set_anchor(Anchor::TOP | Anchor::RIGHT);
        // The compositor already subtracts the strip's exclusive zone when placing
        // non-exclusive popups. Do not apply its width a second time.
        let right_margin = if self.strip_request.1 {
            0
        } else {
            self.strip_width as i32
        };
        layer.set_margin(y as i32, right_margin, 0, 0);
        layer.set_size(width, height);
        layer.set_exclusive_zone(0);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.commit();
        self.popup = Some(layer);
    }
    fn present_popup(&mut self, canvas: Canvas) {
        if let Some(layer) = self.popup.clone() {
            self.present(layer.wl_surface(), canvas);
        }
    }
    fn close_popup(&mut self) {
        self.popup = None;
    }
    fn open_url(&self, target: &str) {
        app::spawn_detached(Command::new("xdg-open").arg(target));
    }
    fn open_settings(&mut self, config_path: &Path) {
        match app::settings_command(config_path) {
            Ok(mut command) => app::spawn_detached(&mut command),
            Err(error) => tracing::warn!(%error, "could not open settings"),
        }
    }
    fn is_light(&self, _config: &Config) -> bool {
        Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "color-scheme"])
            .output()
            .map(|out| {
                out.status.success()
                    && String::from_utf8_lossy(&out.stdout).contains("prefer-light")
            })
            .unwrap_or(false)
    }
    fn live_palette(&self, _config: &Config) -> Option<Palette> {
        theme::load().ok()
    }
    fn poll_font_settings(&mut self) -> Option<TextRenderer> {
        let settings = typography::settings()
            .ok()
            .filter(|settings| *settings != self.font_settings)?;
        let text = typography::renderer(&settings).ok()?;
        self.font_settings = settings;
        Some(text)
    }
    fn hover_delay(&self) -> Duration {
        Duration::from_millis(150)
    }
    fn list_monitors(&self) -> Vec<MonitorInfo> {
        monitors(&self.outputs)
    }
}

fn monitors(outputs: &OutputState) -> Vec<MonitorInfo> {
    outputs
        .outputs()
        .filter_map(|output| {
            let info = outputs.info(&output)?;
            let (width, height) = info
                .modes
                .iter()
                .find(|m| m.current)
                .map(|m| m.dimensions)
                .unwrap_or((0, 0));
            let (x, y) = info.location;
            Some(MonitorInfo {
                name: info.name.unwrap_or_default(),
                width: width.max(0) as u32,
                height: height.max(0) as u32,
                x,
                y,
                primary: false,
            })
        })
        .collect()
}

/// Standalone output probe for `calstack --list-displays`; doesn't create any
/// surfaces, just connects long enough to read output metadata.
pub fn list_monitors() -> Result<Vec<MonitorInfo>> {
    let conn =
        Connection::connect_to_env().context("run Calstack inside a Wayland desktop session")?;
    let (globals, mut queue) = registry_queue_init(&conn)?;
    let qh = queue.handle();
    struct Probe {
        registry: RegistryState,
        outputs: OutputState,
    }
    impl OutputHandler for Probe {
        fn output_state(&mut self) -> &mut OutputState {
            &mut self.outputs
        }
        fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
        fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        }
        fn output_destroyed(
            &mut self,
            _: &Connection,
            _: &QueueHandle<Self>,
            _: wl_output::WlOutput,
        ) {
        }
    }
    impl ProvidesRegistryState for Probe {
        fn registry(&mut self) -> &mut RegistryState {
            &mut self.registry
        }
        registry_handlers![OutputState];
    }
    delegate_output!(Probe);
    delegate_registry!(Probe);
    let mut probe = Probe {
        registry: RegistryState::new(&globals),
        outputs: OutputState::new(&globals, &qh),
    };
    queue.roundtrip(&mut probe)?;
    Ok(monitors(&probe.outputs))
}

pub fn run(config: Config, config_path: PathBuf, demo: bool) -> Result<()> {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("WAYLAND_SOCKET").is_none()
    {
        anyhow::bail!("run Calstack inside a Wayland desktop session");
    }
    let mut waiting = false;
    loop {
        let conn = match Connection::connect_to_env() {
            Ok(conn) => conn,
            Err(_) => {
                if !waiting {
                    tracing::warn!("waiting for the Wayland compositor");
                }
                waiting = true;
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
        };
        waiting = false;
        let current = Config::load(&config_path).unwrap_or_else(|_| config.clone());
        match run_session(conn.clone(), current, config_path.clone(), demo) {
            Ok(()) => return Ok(()),
            Err(_) if conn.backend().last_error().is_some() => {
                tracing::warn!("Wayland connection lost; reconnecting");
                std::thread::sleep(Duration::from_secs(2));
            }
            Err(error) => return Err(error),
        }
    }
}
fn run_session(conn: Connection, config: Config, config_path: PathBuf, demo: bool) -> Result<()> {
    let (globals, mut queue) = registry_queue_init(&conn)?;
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh)?;
    let shell = LayerShell::bind(&globals, &qh).context("compositor must support layer-shell")?;
    let shm = Shm::bind(&globals, &qh)?;
    let pool = SlotPool::new(1024 * 1024, &shm)?;
    let font_settings = typography::settings()?;
    let text = typography::renderer(&font_settings)?;
    let wl = Wayland {
        registry: RegistryState::new(&globals),
        seats: SeatState::new(&globals, &qh),
        outputs: OutputState::new(&globals, &qh),
        compositor,
        shell,
        shm,
        pool,
        qh: qh.clone(),
        output: None,
        strip: None,
        strip_request: (0, false),
        strip_width: config.display.width,
        pointer: None,
        popup: None,
        font_settings,
        strip_height: 0,
        settle_due: None,
    };
    let palette = app::palette_for(&wl, &config);
    let mut app = App {
        core: Core::new(config, config_path, demo, text, palette),
        wl,
    };
    // Receive output metadata before selecting a named monitor.
    queue.roundtrip(&mut app)?;
    let mut event_loop: EventLoop<App> = EventLoop::try_new()?;
    WaylandSource::new(conn.clone(), queue)
        .insert(event_loop.handle())
        .map_err(|e| anyhow::anyhow!("Wayland event source: {e}"))?;
    app.core.start(&mut app.wl);
    tracing::info!("Calstack started; bottom ⋮ menu has Refresh / Settings / Quit");
    while !app.core.exit {
        let mut due = app.core.deadline();
        if let Some(settle) = app.wl.settle_due {
            due = due.min(settle);
        }
        event_loop.dispatch(
            Some(due.saturating_duration_since(Instant::now())),
            &mut app,
        )?;
        app.core.run_timers(&mut app.wl);
        app.run_settle_timer();
    }
    Ok(())
}

impl App {
    fn ensure_output(&mut self, removed: Option<&wl_output::WlOutput>) {
        if self.wl.place_strip(&self.core.config, removed) {
            self.core.strip_recreated(&mut self.wl);
        }
    }
    /// Re-create the strip once the layout has settled after a growth, so the
    /// compositor places it below a returning bar instead of shrinking the bar.
    fn run_settle_timer(&mut self) {
        if self.wl.settle_due.is_some_and(|due| Instant::now() >= due) {
            self.wl.settle_due = None;
            self.wl.strip = None;
            self.ensure_output(None);
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        if let Some(surface) = self.wl.surface_of(surface) {
            self.core
                .scale_changed(&mut self.wl, surface, factor.clamp(1, 4) as u32);
        }
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}
impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.wl.outputs
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.ensure_output(None);
    }
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.ensure_output(None);
    }
    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.ensure_output(Some(&output));
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if self.wl.strip.as_ref() == Some(layer) {
            self.wl.strip = None;
            self.ensure_output(None);
        } else if self.wl.popup.as_ref() == Some(layer) {
            self.core.close_popup(&mut self.wl);
        }
    }
    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let (width, height) = configure.new_size;
        if self.wl.strip.as_ref() == Some(layer) {
            if width > 0 {
                self.wl.strip_width = width;
            }
            self.wl.note_strip_height(height);
            self.core.strip_configured(&mut self.wl, width, height);
        } else if self.wl.popup.as_ref() == Some(layer) {
            self.core.popup_configured(&mut self.wl, width, height);
        }
    }
}
impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.wl.seats
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.wl.pointer.is_none() {
            self.wl.pointer = Some(self.wl.seats.get_pointer(qh, &seat).expect("get pointer"));
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(p) = self.wl.pointer.take() {
                p.release();
            }
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}
impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        use PointerEventKind::*;
        for event in events {
            let Some(surface) = self.wl.surface_of(&event.surface) else {
                continue;
            };
            let (x, y) = (event.position.0 as f32, event.position.1 as f32);
            match event.kind {
                Enter { .. } | Motion { .. } => {
                    self.core.pointer_moved(&mut self.wl, surface, x, y)
                }
                Leave { .. } => self.core.pointer_left(surface),
                Press { button: 0x110, .. } => {
                    self.core.pointer_pressed(&mut self.wl, surface, x, y)
                }
                _ => {}
            }
        }
    }
}
impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.wl.shm
    }
}
impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.wl.registry
    }
    registry_handlers![OutputState, SeatState];
}
delegate_compositor!(App);
delegate_output!(App);
delegate_shm!(App);
delegate_seat!(App);
delegate_pointer!(App);
delegate_layer!(App);
delegate_registry!(App);
