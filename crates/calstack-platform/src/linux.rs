use anyhow::{bail, Context, Result};
use calstack_core::{
    config::Config,
    demo_events,
    feeds::{self, FeedUpdate},
    layout::{self, Block, MENU_HEIGHT},
    meeting::recognized_url,
    Event,
};
use calstack_render::{Canvas, Palette, TextRenderer};
use chrono::{Local, NaiveDate, Timelike};
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
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, QueueHandle,
};

#[derive(Clone, Debug, PartialEq)]
enum PopupKind {
    Event(usize, bool),
    Overlap(Vec<usize>),
    Menu,
}
struct Popup {
    layer: LayerSurface,
    kind: PopupKind,
    width: u32,
    height: u32,
    scale: u32,
    configured: bool,
    hovered: Option<usize>,
}
struct App {
    registry: RegistryState,
    seats: SeatState,
    outputs: OutputState,
    compositor: CompositorState,
    shell: LayerShell,
    shm: Shm,
    pool: SlotPool,
    output: Option<wl_output::WlOutput>,
    strip: Option<LayerSurface>,
    pointer: Option<wl_pointer::WlPointer>,
    popup: Option<Popup>,
    text: TextRenderer,
    font_settings: crate::typography::FontSettings,
    config: Config,
    config_path: PathBuf,
    events: Vec<Event>,
    demo: bool,
    feed_results: Option<std::sync::mpsc::Receiver<FeedUpdate>>,
    feed_pending: bool,
    feed_due: Instant,
    feed_day: NaiveDate,
    blocks: Vec<Block>,
    width: u32,
    height: u32,
    scale: u32,
    palette: Palette,
    theme_due: Instant,
    hovered: Vec<usize>,
    pointer_y: f32,
    in_strip: bool,
    in_popup: bool,
    hover_due: Option<Instant>,
    dismiss_due: Option<Instant>,
    tick_due: Instant,
    exit: bool,
}

pub fn run(config: Config, config_path: PathBuf, demo: bool) -> Result<()> {
    let conn = Connection::connect_to_env()
        .context("connect to Wayland; run inside your desktop session")?;
    let (globals, mut queue) = registry_queue_init(&conn)?;
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh)?;
    let shell = LayerShell::bind(&globals, &qh).context("compositor must support layer-shell")?;
    let shm = Shm::bind(&globals, &qh)?;
    let pool = SlotPool::new(1024 * 1024, &shm)?;
    let font_settings = crate::typography::settings()?;
    let text = crate::typography::renderer(&font_settings)?;
    let palette = palette_for(&config);
    let mut app = App {
        registry: RegistryState::new(&globals),
        seats: SeatState::new(&globals, &qh),
        outputs: OutputState::new(&globals, &qh),
        compositor,
        shell,
        shm,
        pool,
        output: None,
        strip: None,
        pointer: None,
        popup: None,
        text,
        font_settings,
        width: config.display.width,
        height: 0,
        scale: 1,
        palette,
        theme_due: Instant::now() + Duration::from_secs(2),
        config,
        config_path,
        events: if demo { demo_events() } else { Vec::new() },
        demo,
        feed_results: None,
        feed_pending: false,
        feed_due: Instant::now(),
        feed_day: Local::now().date_naive(),
        blocks: Vec::new(),
        hovered: Vec::new(),
        pointer_y: 0.0,
        in_strip: false,
        in_popup: false,
        hover_due: None,
        dismiss_due: None,
        tick_due: Instant::now() + Duration::from_secs(30),
        exit: false,
    };
    // Receive output metadata before selecting a named monitor.
    queue.roundtrip(&mut app)?;
    let name = &app.config.display.monitor;
    app.output = app.outputs.outputs().find(|o| {
        name == "primary" || app.outputs.info(o).and_then(|i| i.name).as_deref() == Some(name)
    });
    if app.output.is_none() {
        bail!("no connected output matches monitor {name:?}");
    }
    let layer = app.shell.create_layer_surface(
        &qh,
        app.compositor.create_surface(&qh),
        Layer::Top,
        Some("calstack"),
        app.output.as_ref(),
    );
    layer.set_anchor(Anchor::TOP | Anchor::RIGHT | Anchor::BOTTOM);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer.set_size(app.width, 0);
    layer.set_exclusive_zone(if app.config.display.reserve_space {
        app.width as i32
    } else {
        0
    });
    layer.commit();
    app.strip = Some(layer);
    let mut event_loop: EventLoop<App> = EventLoop::try_new()?;
    WaylandSource::new(conn.clone(), queue)
        .insert(event_loop.handle())
        .map_err(|e| anyhow::anyhow!("Wayland event source: {e}"))?;
    app.start_feed_refresh();
    tracing::info!("Calstack started; bottom ⋮ menu has Refresh / Settings / Quit");
    while !app.exit {
        let mut deadline = app.tick_due.min(app.theme_due);
        if !app.demo && app.feed_results.is_none() {
            deadline = deadline.min(app.feed_due);
        }
        if app.feed_results.is_some() {
            deadline = deadline.min(Instant::now() + Duration::from_millis(200));
        }
        if let Some(due) = app.hover_due {
            deadline = deadline.min(due);
        }
        if let Some(due) = app.dismiss_due {
            deadline = deadline.min(due);
        }
        event_loop.dispatch(
            Some(deadline.saturating_duration_since(Instant::now())),
            &mut app,
        )?;
        app.timers(&qh);
    }
    Ok(())
}

fn palette_for(config: &Config) -> Palette {
    if config.appearance.theme == "auto" {
        if let Ok(palette) = crate::theme::load() {
            return palette;
        }
    }
    Palette::new(is_light(config))
}
fn is_light(config: &Config) -> bool {
    match config.appearance.theme.as_str() {
        "light" => true,
        "dark" => false,
        _ => Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "color-scheme"])
            .output()
            .map(|out| {
                out.status.success()
                    && String::from_utf8_lossy(&out.stdout).contains("prefer-light")
            })
            .unwrap_or(false),
    }
}
fn desktop_open(target: &std::ffi::OsStr) {
    launch_desktop(Command::new("xdg-open").arg(target));
}
fn open_settings(target: &std::ffi::OsStr) {
    // Terminal editors need a visible terminal, not the service's stdin.
    let result = Command::new("omarchy")
        .args(["launch", "editor"])
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn();
    if matches!(&result, Err(error) if error.kind() == std::io::ErrorKind::NotFound) {
        desktop_open(target);
    } else {
        watch_desktop_child(result);
    }
}
fn launch_desktop(command: &mut Command) {
    watch_desktop_child(command.stdin(Stdio::null()).stdout(Stdio::null()).spawn());
}
fn watch_desktop_child(result: std::io::Result<std::process::Child>) {
    match result {
        Ok(mut child) => {
            std::thread::spawn(move || match child.wait() {
                Ok(status) if status.success() => {}
                Ok(status) => tracing::warn!(%status, "desktop opener failed"),
                Err(error) => tracing::warn!(%error, "could not wait for desktop opener"),
            });
        }
        Err(error) => tracing::warn!(%error, "could not start desktop opener"),
    }
}
fn now_minutes() -> f32 {
    let now = Local::now();
    now.hour() as f32 * 60.0 + now.minute() as f32 + now.second() as f32 / 60.0
}

impl App {
    fn layout(&mut self) {
        let (start, end) = self.config.range();
        self.blocks = layout::layout(
            &self.events,
            start,
            end,
            self.width as f32,
            (self.height as f32 - MENU_HEIGHT).max(1.0),
        );
    }
    fn present(&mut self, surface: &wl_surface::WlSurface, canvas: Canvas, scale: u32) {
        let (buffer, pixels) = self
            .pool
            .create_buffer(
                canvas.width as i32,
                canvas.height as i32,
                canvas.width as i32 * 4,
                wl_shm::Format::Argb8888,
            )
            .expect("allocate shared-memory buffer");
        pixels.copy_from_slice(&canvas.pixels);
        surface.set_buffer_scale(scale as i32);
        surface.damage_buffer(0, 0, canvas.width as i32, canvas.height as i32);
        buffer
            .attach_to(surface)
            .expect("attach shared-memory buffer");
        surface.commit();
    }
    fn draw_strip(&mut self) {
        if self.height == 0 {
            return;
        }
        if let Some(layer) = self.strip.clone() {
            let canvas = calstack_render::strip(
                &self.text,
                &self.config,
                &self.events,
                &self.blocks,
                self.width,
                self.height,
                self.scale,
                now_minutes(),
                &self.hovered,
                self.palette,
            );
            self.present(layer.wl_surface(), canvas, self.scale);
        }
    }
    fn draw_popup(&mut self) {
        let Some(p) = self.popup.as_ref().filter(|p| p.configured) else {
            return;
        };
        let surface = p.layer.wl_surface().clone();
        let scale = p.scale;
        let canvas = match &p.kind {
            PopupKind::Menu => calstack_render::popup(
                &self.text,
                &["Refresh".into(), "Settings…".into(), "Quit".into()],
                p.width,
                p.height,
                scale,
                self.palette,
                p.hovered,
            ),
            PopupKind::Event(index, details) => calstack_render::event_popup(
                &self.text,
                &[&self.events[*index]],
                *details,
                p.width,
                p.height,
                scale,
                self.palette,
                p.hovered,
            ),
            PopupKind::Overlap(indices) => {
                let events: Vec<_> = indices.iter().map(|&index| &self.events[index]).collect();
                calstack_render::event_popup(
                    &self.text,
                    &events,
                    false,
                    p.width,
                    p.height,
                    scale,
                    self.palette,
                    p.hovered,
                )
            }
        };
        self.present(&surface, canvas, scale);
    }
    fn close_popup(&mut self) {
        self.popup = None;
        self.in_popup = false;
        self.dismiss_due = None;
    }
    fn show_popup(&mut self, kind: PopupKind, qh: &QueueHandle<Self>) {
        if self.popup.as_ref().is_some_and(|p| p.kind == kind) {
            return;
        }
        self.close_popup();
        let typography = self.text.typography;
        let unit = typography.layout_scale();
        let (width, height) = match &kind {
            PopupKind::Overlap(indices) => (
                (360.0 * unit).ceil() as u32,
                calstack_render::event_popup_height(typography, indices.len(), false),
            ),
            PopupKind::Menu => ((220.0 * unit).ceil() as u32, (96.0 * unit).ceil() as u32),
            PopupKind::Event(_, details) => (
                (360.0 * unit).ceil() as u32,
                calstack_render::event_popup_height(typography, 1, *details),
            ),
        };
        let y = if kind == PopupKind::Menu {
            self.height.saturating_sub(height) as f32
        } else {
            (self.pointer_y - 30.0).clamp(0.0, self.height.saturating_sub(height) as f32)
        };
        let layer = self.shell.create_layer_surface(
            qh,
            self.compositor.create_surface(qh),
            Layer::Top,
            Some("calstack-popup"),
            self.output.as_ref(),
        );
        layer.set_anchor(Anchor::TOP | Anchor::RIGHT);
        // The compositor already subtracts the strip's exclusive zone when placing
        // non-exclusive popups. Do not apply its width a second time.
        let right_margin = if self.config.display.reserve_space {
            0
        } else {
            self.width as i32
        };
        layer.set_margin(y as i32, right_margin, 0, 0);
        layer.set_size(width, height);
        layer.set_exclusive_zone(0);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.commit();
        tracing::debug!(?kind, "show popup");
        self.popup = Some(Popup {
            layer,
            kind,
            width,
            height,
            scale: self.scale,
            configured: false,
            hovered: None,
        });
    }
    fn refresh(&mut self) {
        match Config::load(&self.config_path) {
            Ok(mut config) => {
                if config.display.monitor != self.config.display.monitor {
                    tracing::warn!("monitor changes require restarting Calstack");
                    config.display.monitor = self.config.display.monitor.clone();
                }
                self.events.retain(|event| {
                    config.calendar.feeds.iter().any(|new| {
                        new.enabled
                            && new.name == event.calendar
                            && self.config.calendar.feeds.iter().any(|old| old == new)
                    })
                });
                self.config = config;
                self.width = self.config.display.width;
                self.palette = palette_for(&self.config);
                if let Some(layer) = &self.strip {
                    layer.set_size(self.width, 0);
                    layer.set_exclusive_zone(if self.config.display.reserve_space {
                        self.width as i32
                    } else {
                        0
                    });
                    layer.commit();
                }
                if self.demo {
                    self.events = demo_events();
                }
                self.start_feed_refresh();
                self.hovered.clear();
                self.hover_due = None;
                self.close_popup();
                self.layout();
                self.draw_strip();
                tracing::info!("reloaded configuration and requested calendar refresh");
            }
            Err(error) => tracing::warn!(%error,"configuration unchanged"),
        }
    }
    fn start_feed_refresh(&mut self) {
        if self.demo {
            return;
        }
        self.feed_due =
            Instant::now() + Duration::from_secs(self.config.calendar.refresh_minutes * 60);
        if self.feed_results.is_some() {
            self.feed_pending = true;
            return;
        }
        self.feed_pending = false;
        let feeds = self.config.calendar.feeds.clone();
        let date = self.feed_day;
        let cache = feeds::cache_dir();
        let (sender, receiver) = std::sync::mpsc::channel();
        self.feed_results = Some(receiver);
        std::thread::spawn(move || {
            if sender
                .send(feeds::load(&feeds, &cache, date, false))
                .is_ok()
            {
                for feed in &feeds {
                    if sender
                        .send(feeds::load(std::slice::from_ref(feed), &cache, date, true))
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });
    }
    fn poll_feeds(&mut self) {
        let mut updates = Vec::new();
        let mut finished = false;
        if let Some(receiver) = &self.feed_results {
            loop {
                match receiver.try_recv() {
                    Ok(update) => updates.push(update),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                }
            }
        }
        if !self.feed_pending {
            for update in updates {
                for warning in update.warnings {
                    tracing::warn!("{warning}");
                }
                if update.calendars.is_empty() {
                    continue;
                }
                let previous = self.events.clone();
                for (name, events) in update.calendars {
                    self.events.retain(|event| event.calendar != name);
                    self.events.extend(events);
                }
                self.events.sort_by_key(|event| (event.start, event.end));
                if self.events == previous {
                    continue;
                }
                self.hovered.clear();
                self.hover_due = None;
                self.close_popup();
                self.layout();
                self.draw_strip();
            }
        }
        if finished {
            self.feed_results = None;
            if self.feed_pending {
                self.start_feed_refresh();
            }
        }
    }
    fn timers(&mut self, qh: &QueueHandle<Self>) {
        let now = Instant::now();
        let today = Local::now().date_naive();
        if !self.demo && today != self.feed_day {
            self.feed_day = today;
            self.events.clear();
            self.hovered.clear();
            self.hover_due = None;
            self.close_popup();
            self.layout();
            self.draw_strip();
            self.start_feed_refresh();
        } else if !self.demo && self.feed_due <= now && self.feed_results.is_none() {
            self.start_feed_refresh();
        }
        self.poll_feeds();
        if self.theme_due <= now {
            self.theme_due = now + Duration::from_secs(2);
            if let Ok(settings) = crate::typography::settings() {
                if settings != self.font_settings {
                    if let Ok(text) = crate::typography::renderer(&settings) {
                        self.text = text;
                        self.font_settings = settings;
                        let reopen = self.popup.as_ref().map(|p| p.kind.clone());
                        self.close_popup();
                        self.draw_strip();
                        if let Some(kind) = reopen {
                            self.show_popup(kind, qh);
                        }
                    }
                }
            }
            if self.config.appearance.theme == "auto" {
                // Keep the last good palette during a theme's multi-file swap.
                if let Ok(palette) = crate::theme::load() {
                    if palette != self.palette {
                        self.palette = palette;
                        self.draw_strip();
                        self.draw_popup();
                    }
                }
            }
        }
        if self.tick_due <= now {
            self.tick_due = now + Duration::from_secs(30);
            self.draw_strip();
            self.draw_popup();
        }
        if self.hover_due.is_some_and(|due| due <= now) {
            self.hover_due = None;
            if let Some(&index) = self.hovered.first() {
                let kind = if self.hovered.len() > 1 {
                    PopupKind::Overlap(self.hovered.clone())
                } else {
                    PopupKind::Event(index, false)
                };
                self.show_popup(kind, qh);
            }
        }
        if self.dismiss_due.is_some_and(|due| due <= now) {
            self.dismiss_due = None;
            if !self.in_popup && !self.in_strip {
                self.close_popup();
                self.hovered.clear();
                self.draw_strip();
            }
        }
    }
    fn motion(&mut self, x: f32, y: f32) {
        self.pointer_y = y;
        let hovered = layout::hit_test(&self.blocks, x, y);
        if hovered != self.hovered {
            self.hovered = hovered;
            self.hover_due =
                (!self.hovered.is_empty()).then(|| Instant::now() + Duration::from_millis(150));
            if !self
                .popup
                .as_ref()
                .is_some_and(|p| p.kind == PopupKind::Menu)
            {
                self.close_popup();
            }
            self.draw_strip();
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
        let scale = factor.clamp(1, 4) as u32;
        if self
            .strip
            .as_ref()
            .is_some_and(|l| l.wl_surface() == surface)
        {
            self.scale = scale;
            self.draw_strip();
        }
        if let Some(p) = &mut self.popup {
            if p.layer.wl_surface() == surface {
                p.scale = scale;
                self.draw_popup();
            }
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
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        if self.output.as_ref() == Some(&output) {
            self.exit = true;
        }
    }
}
impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if self.strip.as_ref() == Some(layer) {
            self.exit = true;
        } else if self.popup.as_ref().is_some_and(|p| p.layer == *layer) {
            self.close_popup();
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
        if self.strip.as_ref() == Some(layer) {
            self.width = if configure.new_size.0 == 0 {
                self.config.display.width
            } else {
                configure.new_size.0
            };
            self.height = configure.new_size.1.max(1);
            self.layout();
            self.draw_strip();
            tracing::debug!(
                width = self.width,
                height = self.height,
                scale = self.scale,
                "strip configured"
            );
        } else if let Some(p) = &mut self.popup {
            if p.layer == *layer {
                if configure.new_size.0 > 0 {
                    p.width = configure.new_size.0;
                }
                if configure.new_size.1 > 0 {
                    p.height = configure.new_size.1;
                }
                p.configured = true;
                self.draw_popup();
            }
        }
    }
}
impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = Some(self.seats.get_pointer(qh, &seat).expect("get pointer"));
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
            if let Some(p) = self.pointer.take() {
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
        qh: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        use PointerEventKind::*;
        for event in events {
            let strip = self
                .strip
                .as_ref()
                .is_some_and(|l| l.wl_surface() == &event.surface);
            let popup = self
                .popup
                .as_ref()
                .is_some_and(|p| p.layer.wl_surface() == &event.surface);
            if !strip && !popup {
                continue;
            }
            let (x, y) = (event.position.0 as f32, event.position.1 as f32);
            match event.kind {
                Enter { .. } | Motion { .. } => {
                    self.dismiss_due = None;
                    if strip {
                        self.in_strip = true;
                        self.motion(x, y);
                    }
                    if popup {
                        self.in_popup = true;
                        self.hover_due = None;
                        if let Some(p) = &mut self.popup {
                            let unit = self.text.typography.layout_scale();
                            let row = if y >= 8.0 * unit {
                                Some(((y / unit - 8.0) / 26.0) as usize)
                            } else {
                                None
                            };
                            p.hovered = match &p.kind {
                                PopupKind::Menu => row.filter(|r| *r < 3),
                                PopupKind::Overlap(indices) => calstack_render::event_card_at(
                                    self.text.typography,
                                    indices.len(),
                                    p.width,
                                    x,
                                    y,
                                ),
                                PopupKind::Event(_, _) => calstack_render::event_card_at(
                                    self.text.typography,
                                    1,
                                    p.width,
                                    x,
                                    y,
                                ),
                            };
                            self.draw_popup();
                        }
                    }
                }
                Leave { .. } => {
                    if strip {
                        self.in_strip = false;
                        self.hover_due = None;
                    }
                    if popup {
                        self.in_popup = false;
                    }
                    self.dismiss_due = Some(Instant::now() + Duration::from_millis(180));
                }
                Press { button: 0x110, .. } => {
                    if strip {
                        if y >= self.height as f32 - MENU_HEIGHT {
                            self.hover_due = None;
                            if self
                                .popup
                                .as_ref()
                                .is_some_and(|p| p.kind == PopupKind::Menu)
                            {
                                self.close_popup();
                            } else {
                                self.show_popup(PopupKind::Menu, qh);
                            }
                        } else {
                            let indices = layout::hit_test(&self.blocks, x, y);
                            if indices.len() > 1 {
                                self.hover_due = None;
                                self.show_popup(PopupKind::Overlap(indices), qh);
                            } else if let Some(&index) = indices.first() {
                                self.click_event(index, qh);
                            }
                        }
                    } else if let Some(p) = &self.popup {
                        match p.kind.clone() {
                            PopupKind::Menu => match p.hovered {
                                Some(0) => self.refresh(),
                                Some(1) => {
                                    open_settings(self.config_path.as_os_str());
                                    self.close_popup();
                                }
                                Some(2) => self.exit = true,
                                _ => {}
                            },
                            PopupKind::Event(index, _) => {
                                if calstack_render::event_card_at(
                                    self.text.typography,
                                    1,
                                    p.width,
                                    x,
                                    y,
                                )
                                .is_some()
                                {
                                    self.click_event(index, qh);
                                }
                            }
                            PopupKind::Overlap(indices) => {
                                if let Some(card) = calstack_render::event_card_at(
                                    self.text.typography,
                                    indices.len(),
                                    p.width,
                                    x,
                                    y,
                                ) {
                                    if let Some(&index) = indices.get(card) {
                                        self.click_event(index, qh);
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
impl App {
    fn click_event(&mut self, index: usize, qh: &QueueHandle<Self>) {
        self.hover_due = None;
        if let Some(url) = self.events[index]
            .meeting
            .as_deref()
            .and_then(recognized_url)
        {
            desktop_open(std::ffi::OsStr::new(url.as_str()));
        } else {
            self.show_popup(PopupKind::Event(index, true), qh);
        }
    }
}
impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}
impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
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
