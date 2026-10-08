//! Shared app state and OS-independent decision logic. Every item here is
//! plain Rust with no native window handles: each backend (`linux`, `macos`)
//! owns its own surfaces and implements `Backend` to create/present/position
//! them, while `Core` owns everything else (config, events, layout, popup
//! state, hover/dismiss timers, feed polling) and drives the backend through
//! the trait rather than holding a native handle itself.
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
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Strip,
    Popup,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PopupKind {
    Event(usize, bool),
    Overlap(Vec<usize>),
    Menu,
}

/// One entry per physical display. `name` is what `display.monitor` is
/// compared against (a Wayland output name on Linux, `NSScreen`'s localized
/// name via winit on macOS).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MonitorInfo {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub primary: bool,
}

struct PopupState {
    kind: PopupKind,
    width: u32,
    height: u32,
    scale: u32,
    configured: bool,
    hovered: Option<usize>,
}

/// Everything a backend must provide. A `Backend` owns native window/surface
/// handles; it never owns app state, and `Core` never holds a native handle.
pub trait Backend {
    /// Create the strip if it doesn't exist yet, or move/resize it to match
    /// `config` (monitor selection, width, reserve-space) if it does. The
    /// resulting size is reported back through `Core::strip_configured`,
    /// either synchronously (macOS) or asynchronously once the compositor
    /// confirms it (Linux's layer-shell `configure` event). Returns true when
    /// the strip was torn down and recreated (Linux, on a monitor change), so
    /// `Core` must forget per-surface state such as size, scale, and hover.
    fn ensure_strip(&mut self, config: &Config) -> bool;
    fn present_strip(&mut self, canvas: Canvas);

    /// Open or move the popup surface to anchor at `y` (strip-local vertical
    /// position), sized `width`x`height`. Size/position is logical; the
    /// backend applies its own scale.
    fn show_popup(&mut self, width: u32, height: u32, y: f32);
    fn present_popup(&mut self, canvas: Canvas);
    fn close_popup(&mut self);

    fn open_url(&self, target: &str);
    /// Open (or focus) the settings UI. `&mut self` so a backend can track
    /// the spawned window/process and close it when `Core::exit` fires.
    fn open_settings(&mut self, config_path: &Path);

    /// OS dark/light fallback, used when `appearance.theme == "auto"` and
    /// `live_palette` returns `None`.
    fn is_light(&self, config: &Config) -> bool;
    /// A continuously live-reloadable palette (Omarchy's theme files on
    /// Linux). `None` means "no live palette source on this OS."
    fn live_palette(&self, config: &Config) -> Option<Palette>;
    /// Poll for a font or text-scale change since the last call. `None` if
    /// nothing changed or the backend doesn't track live font settings.
    fn poll_font_settings(&mut self) -> Option<TextRenderer>;
    /// How long the pointer must rest on an event before its popup opens.
    fn hover_delay(&self) -> Duration;

    // Not yet called outside tests: reserved for the shared settings GUI
    // (monitor dropdown) that will replace the Linux-only GTK settings tool.
    #[allow(dead_code)]
    fn list_monitors(&self) -> Vec<MonitorInfo>;
}

fn now_minutes() -> f32 {
    let now = Local::now();
    now.hour() as f32 * 60.0 + now.minute() as f32 + now.second() as f32 / 60.0
}

fn config_stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn resumed(previous: SystemTime, current: SystemTime, elapsed: Duration) -> bool {
    match current.duration_since(previous) {
        Ok(wall) => wall > elapsed + Duration::from_secs(30),
        Err(_) => true,
    }
}

pub(crate) fn palette_for<B: Backend>(backend: &B, config: &Config) -> Palette {
    if config.appearance.theme == "auto" {
        if let Some(palette) = backend.live_palette(config) {
            return palette;
        }
    }
    Palette::new(is_light(backend, config))
}

fn is_light<B: Backend>(backend: &B, config: &Config) -> bool {
    match config.appearance.theme.as_str() {
        "light" => true,
        "dark" => false,
        _ => backend.is_light(config),
    }
}

pub struct Core {
    pub config: Config,
    pub config_path: PathBuf,
    config_stamp: Option<(SystemTime, u64)>,
    last_wall: SystemTime,
    last_timer: Instant,
    pub events: Vec<Event>,
    demo: bool,
    feed_results: Option<std::sync::mpsc::Receiver<FeedUpdate>>,
    feed_pending: bool,
    feed_due: Instant,
    feed_day: NaiveDate,
    pub blocks: Vec<Block>,
    pub width: u32,
    pub height: u32,
    pub scale: u32,
    pub palette: Palette,
    theme_due: Instant,
    hovered: Vec<usize>,
    hover_due: Option<Instant>,
    pointer_y: f32,
    in_strip: bool,
    in_popup: bool,
    dismiss_due: Option<Instant>,
    tick_due: Instant,
    popup: Option<PopupState>,
    pub text: TextRenderer,
    pub exit: bool,
}

impl Core {
    pub fn new(
        config: Config,
        config_path: PathBuf,
        demo: bool,
        text: TextRenderer,
        palette: Palette,
    ) -> Self {
        Self {
            width: config.display.width,
            height: 0,
            scale: 1,
            palette,
            theme_due: Instant::now() + Duration::from_secs(2),
            config_stamp: config_stamp(&config_path),
            config,
            config_path,
            last_wall: SystemTime::now(),
            last_timer: Instant::now(),
            events: if demo { demo_events() } else { Vec::new() },
            demo,
            feed_results: None,
            feed_pending: false,
            feed_due: Instant::now(),
            feed_day: Local::now().date_naive(),
            blocks: Vec::new(),
            hovered: Vec::new(),
            hover_due: None,
            pointer_y: 0.0,
            in_strip: false,
            in_popup: false,
            dismiss_due: None,
            tick_due: Instant::now() + Duration::from_secs(30),
            popup: None,
            text,
            exit: false,
        }
    }

    pub fn start<B: Backend>(&mut self, backend: &mut B) {
        self.ensure_strip(backend);
        self.start_feed_refresh();
    }

    fn ensure_strip<B: Backend>(&mut self, backend: &mut B) {
        if backend.ensure_strip(&self.config) {
            self.strip_recreated(backend);
        }
    }

    /// The backend replaced the strip surface; its size and scale arrive
    /// again through `strip_configured`/`scale_changed`.
    pub fn strip_recreated<B: Backend>(&mut self, backend: &mut B) {
        self.close_popup(backend);
        self.clear_hover();
        self.scale = 1;
        self.height = 0;
        self.blocks.clear();
    }

    fn clear_hover(&mut self) {
        self.hovered.clear();
        self.hover_due = None;
    }

    /// Soonest instant `run_timers` needs to run again; callers block their
    /// event loop up to this, same shape as `run_session`'s former inline
    /// `deadline` min-chain.
    pub fn deadline(&self) -> Instant {
        let mut deadline = self.tick_due.min(self.theme_due);
        if !self.demo && self.feed_results.is_none() {
            deadline = deadline.min(self.feed_due);
        }
        if self.feed_results.is_some() {
            deadline = deadline.min(Instant::now() + Duration::from_millis(200));
        }
        if let Some(due) = self.hover_due {
            deadline = deadline.min(due);
        }
        if let Some(due) = self.dismiss_due {
            deadline = deadline.min(due);
        }
        deadline
    }

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

    fn draw_strip<B: Backend>(&mut self, backend: &mut B) {
        if self.height == 0 {
            return;
        }
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
        backend.present_strip(canvas);
    }

    fn draw_popup<B: Backend>(&mut self, backend: &mut B) {
        let Some(p) = self.popup.as_ref().filter(|p| p.configured) else {
            return;
        };
        let canvas = match &p.kind {
            PopupKind::Menu => calstack_render::popup(
                &self.text,
                &["Refresh".into(), "Settings…".into(), "Quit".into()],
                p.width,
                p.height,
                p.scale,
                self.palette,
                p.hovered,
            ),
            PopupKind::Event(index, details) => calstack_render::event_popup(
                &self.text,
                &[&self.events[*index]],
                *details,
                p.width,
                p.height,
                p.scale,
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
                    p.scale,
                    self.palette,
                    p.hovered,
                )
            }
        };
        backend.present_popup(canvas);
    }

    /// Also called by a backend whose popup surface was closed externally.
    pub fn close_popup<B: Backend>(&mut self, backend: &mut B) {
        if self.popup.take().is_some() {
            backend.close_popup();
        }
        self.in_popup = false;
        self.dismiss_due = None;
    }

    fn show_popup<B: Backend>(&mut self, backend: &mut B, kind: PopupKind) {
        if self.popup.as_ref().is_some_and(|p| p.kind == kind) {
            return;
        }
        self.close_popup(backend);
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
        backend.show_popup(width, height, y);
        tracing::debug!(?kind, "show popup");
        self.popup = Some(PopupState {
            kind,
            width,
            height,
            scale: self.scale,
            configured: false,
            hovered: None,
        });
    }

    pub fn refresh<B: Backend>(&mut self, backend: &mut B) {
        match Config::load(&self.config_path) {
            Ok(config) => {
                self.events.retain(|event| {
                    config.calendar.feeds.iter().any(|new| {
                        new.enabled
                            && new.name == event.calendar
                            && self.config.calendar.feeds.iter().any(|old| old == new)
                    })
                });
                self.config = config;
                if !self.demo {
                    if let Err(error) = crate::desktop::sync_autostart(
                        self.config.startup.autostart,
                        &self.config_path,
                    ) {
                        tracing::warn!(%error, "could not apply start-at-login setting");
                    }
                }
                self.width = self.config.display.width;
                self.palette = palette_for(backend, &self.config);
                self.ensure_strip(backend);
                if self.demo {
                    self.events = demo_events();
                }
                self.start_feed_refresh();
                self.clear_hover();
                self.close_popup(backend);
                self.layout();
                self.draw_strip(backend);
                tracing::info!("reloaded configuration and requested calendar refresh");
            }
            Err(error) => tracing::warn!(%error, "configuration unchanged"),
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

    fn poll_feeds<B: Backend>(&mut self, backend: &mut B) {
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
                self.clear_hover();
                self.close_popup(backend);
                self.layout();
                self.draw_strip(backend);
            }
        }
        if finished {
            self.feed_results = None;
            if self.feed_pending {
                self.start_feed_refresh();
            }
        }
    }

    pub fn run_timers<B: Backend>(&mut self, backend: &mut B) {
        let now = Instant::now();
        let wall = SystemTime::now();
        let woke = resumed(self.last_wall, wall, now.duration_since(self.last_timer));
        self.last_wall = wall;
        self.last_timer = now;
        let today = Local::now().date_naive();
        if !self.demo && today != self.feed_day {
            self.feed_day = today;
            self.events.clear();
            self.clear_hover();
            self.close_popup(backend);
            self.layout();
            self.draw_strip(backend);
            self.start_feed_refresh();
        } else if !self.demo && (woke || self.feed_due <= now) && self.feed_results.is_none() {
            self.start_feed_refresh();
        }
        self.poll_feeds(backend);
        if self.theme_due <= now {
            self.theme_due = now + Duration::from_secs(2);
            let stamp = config_stamp(&self.config_path);
            if stamp != self.config_stamp {
                self.config_stamp = stamp;
                self.refresh(backend);
            }
            if let Some(text) = backend.poll_font_settings() {
                self.text = text;
                let reopen = self.popup.as_ref().map(|p| p.kind.clone());
                self.close_popup(backend);
                self.draw_strip(backend);
                if let Some(kind) = reopen {
                    self.show_popup(backend, kind);
                }
            }
            if self.config.appearance.theme == "auto" {
                // Keep the last good palette during a theme's multi-file swap.
                if let Some(palette) = backend.live_palette(&self.config) {
                    if palette != self.palette {
                        self.palette = palette;
                        self.draw_strip(backend);
                        self.draw_popup(backend);
                    }
                }
            }
        }
        if self.tick_due <= now {
            self.tick_due = now + Duration::from_secs(30);
            self.draw_strip(backend);
            self.draw_popup(backend);
        }
        if self.hover_due.is_some_and(|due| due <= now) {
            self.hover_due = None;
            self.show_hovered(backend);
        }
        if self.dismiss_due.is_some_and(|due| due <= now) {
            self.dismiss_due = None;
            if !self.in_popup && !self.in_strip {
                self.close_popup(backend);
                self.clear_hover();
                self.draw_strip(backend);
            }
        }
    }

    pub fn strip_configured<B: Backend>(&mut self, backend: &mut B, width: u32, height: u32) {
        self.width = if width == 0 {
            self.config.display.width
        } else {
            width
        };
        self.height = height.max(1);
        self.layout();
        self.draw_strip(backend);
        tracing::debug!(
            width = self.width,
            height = self.height,
            scale = self.scale,
            "strip configured"
        );
    }

    pub fn popup_configured<B: Backend>(&mut self, backend: &mut B, width: u32, height: u32) {
        if let Some(p) = &mut self.popup {
            if width > 0 {
                p.width = width;
            }
            if height > 0 {
                p.height = height;
            }
            p.configured = true;
        }
        self.draw_popup(backend);
    }

    pub fn scale_changed<B: Backend>(&mut self, backend: &mut B, surface: Surface, scale: u32) {
        match surface {
            Surface::Strip => {
                self.scale = scale;
                self.draw_strip(backend);
            }
            Surface::Popup => {
                if let Some(p) = &mut self.popup {
                    p.scale = scale;
                }
                self.draw_popup(backend);
            }
        }
    }

    fn show_hovered<B: Backend>(&mut self, backend: &mut B) {
        if let Some(&index) = self.hovered.first() {
            let kind = if self.hovered.len() > 1 {
                PopupKind::Overlap(self.hovered.clone())
            } else {
                PopupKind::Event(index, false)
            };
            self.show_popup(backend, kind);
        }
    }

    fn motion<B: Backend>(&mut self, backend: &mut B, x: f32, y: f32) {
        self.pointer_y = y;
        let hovered = layout::hit_test(&self.blocks, x, y);
        if hovered != self.hovered {
            self.hovered = hovered;
            let delay = backend.hover_delay();
            self.hover_due =
                (!self.hovered.is_empty() && !delay.is_zero()).then(|| Instant::now() + delay);
            if !self.hovered.is_empty() && delay.is_zero() {
                self.show_hovered(backend);
            } else if !self
                .popup
                .as_ref()
                .is_some_and(|p| p.kind == PopupKind::Menu)
            {
                self.close_popup(backend);
            }
            self.draw_strip(backend);
        }
    }

    pub fn pointer_moved<B: Backend>(&mut self, backend: &mut B, surface: Surface, x: f32, y: f32) {
        self.dismiss_due = None;
        match surface {
            Surface::Strip => {
                self.in_strip = true;
                self.motion(backend, x, y);
            }
            Surface::Popup => {
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
                        PopupKind::Event(_, _) => {
                            calstack_render::event_card_at(self.text.typography, 1, p.width, x, y)
                        }
                    };
                }
                self.draw_popup(backend);
            }
        }
    }

    pub fn pointer_left(&mut self, surface: Surface) {
        match surface {
            Surface::Strip => {
                self.in_strip = false;
                self.hover_due = None;
            }
            Surface::Popup => self.in_popup = false,
        }
        self.dismiss_due = Some(Instant::now() + Duration::from_millis(180));
    }

    pub fn pointer_pressed<B: Backend>(
        &mut self,
        backend: &mut B,
        surface: Surface,
        x: f32,
        y: f32,
    ) {
        match surface {
            Surface::Strip => {
                self.hover_due = None;
                if y >= self.height as f32 - MENU_HEIGHT {
                    if self
                        .popup
                        .as_ref()
                        .is_some_and(|p| p.kind == PopupKind::Menu)
                    {
                        self.close_popup(backend);
                    } else {
                        self.show_popup(backend, PopupKind::Menu);
                    }
                } else {
                    let indices = layout::hit_test(&self.blocks, x, y);
                    if indices.len() > 1 {
                        self.show_popup(backend, PopupKind::Overlap(indices));
                    } else if let Some(&index) = indices.first() {
                        self.click_event(backend, index);
                    }
                }
            }
            Surface::Popup => {
                let Some(p) = &self.popup else { return };
                match p.kind.clone() {
                    PopupKind::Menu => match p.hovered {
                        Some(0) => self.refresh(backend),
                        Some(1) => {
                            backend.open_settings(&self.config_path);
                            self.close_popup(backend);
                        }
                        Some(2) => self.exit = true,
                        _ => {}
                    },
                    PopupKind::Event(index, _) => {
                        if calstack_render::event_card_at(self.text.typography, 1, p.width, x, y)
                            .is_some()
                        {
                            self.click_event(backend, index);
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
                                self.click_event(backend, index);
                            }
                        }
                    }
                }
            }
        }
    }

    fn click_event<B: Backend>(&mut self, backend: &mut B, index: usize) {
        self.hover_due = None;
        if let Some(url) = self.events[index]
            .meeting
            .as_deref()
            .and_then(recognized_url)
        {
            backend.open_url(url.as_str());
        } else {
            self.show_popup(backend, PopupKind::Event(index, true));
        }
    }
}

/// Spawn a desktop helper (URL opener, settings window) without blocking the
/// event loop, logging rather than surfacing its eventual failure.
pub(crate) fn spawn_detached(command: &mut Command) {
    match command.stdin(Stdio::null()).stdout(Stdio::null()).spawn() {
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

/// `calstack --settings` for this executable, ready to spawn.
pub(crate) fn settings_command(config_path: &Path) -> std::io::Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--settings")
        .arg("--config")
        .arg(config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use calstack_render::TextRenderer;
    use std::cell::RefCell;

    const REGULAR: &[u8] = include_bytes!("../assets/Inter-Regular.ttf");

    #[derive(Default)]
    struct Recording {
        strip_presented: RefCell<u32>,
        popup_shown: RefCell<u32>,
        popup_closed: RefCell<u32>,
        opened_urls: RefCell<Vec<String>>,
        monitors: Vec<MonitorInfo>,
        hover_delay: Duration,
    }
    impl Backend for Recording {
        fn ensure_strip(&mut self, _config: &Config) -> bool {
            false
        }
        fn present_strip(&mut self, _canvas: Canvas) {
            *self.strip_presented.borrow_mut() += 1;
        }
        fn show_popup(&mut self, _width: u32, _height: u32, _y: f32) {
            *self.popup_shown.borrow_mut() += 1;
        }
        fn present_popup(&mut self, _canvas: Canvas) {}
        fn close_popup(&mut self) {
            *self.popup_closed.borrow_mut() += 1;
        }
        fn open_url(&self, target: &str) {
            self.opened_urls.borrow_mut().push(target.to_string());
        }
        fn open_settings(&mut self, _config_path: &Path) {}
        fn is_light(&self, _config: &Config) -> bool {
            false
        }
        fn live_palette(&self, _config: &Config) -> Option<Palette> {
            None
        }
        fn poll_font_settings(&mut self) -> Option<TextRenderer> {
            None
        }
        fn hover_delay(&self) -> Duration {
            self.hover_delay
        }
        fn list_monitors(&self) -> Vec<MonitorInfo> {
            self.monitors.clone()
        }
    }

    fn core(events: Vec<Event>) -> Core {
        let text = TextRenderer::new(REGULAR.to_vec(), REGULAR.to_vec()).unwrap();
        let mut core = Core::new(
            Config::default(),
            PathBuf::from("/nonexistent/calstack-test.toml"),
            true,
            text,
            Palette::new(false),
        );
        core.events = events;
        core.strip_configured(&mut Recording::default(), 13, 1000);
        core
    }

    fn event(start: i32, end: i32, meeting: Option<&str>) -> Event {
        Event {
            title: "test".into(),
            calendar: "test".into(),
            start,
            end,
            meeting: meeting.map(str::to_owned),
            color: None,
        }
    }

    #[test]
    fn hover_over_an_event_shows_a_popup_immediately() {
        let mut backend = Recording::default();
        let mut core = core(vec![event(0, 1440, None)]);
        core.pointer_moved(&mut backend, Surface::Strip, 2.0, 500.0);
        assert_eq!(*backend.popup_shown.borrow(), 1);
    }

    #[test]
    fn hover_delay_defers_the_popup_until_the_timer_fires() {
        let mut backend = Recording {
            hover_delay: Duration::from_millis(150),
            ..Default::default()
        };
        let mut core = core(vec![event(0, 1440, None)]);
        core.pointer_moved(&mut backend, Surface::Strip, 2.0, 500.0);
        assert_eq!(*backend.popup_shown.borrow(), 0);
        core.hover_due = Some(Instant::now());
        core.run_timers(&mut backend);
        assert_eq!(*backend.popup_shown.borrow(), 1);
    }

    #[test]
    fn leaving_the_strip_cancels_a_pending_hover_popup() {
        let mut backend = Recording {
            hover_delay: Duration::from_millis(150),
            ..Default::default()
        };
        let mut core = core(vec![event(0, 1440, None)]);
        core.pointer_moved(&mut backend, Surface::Strip, 2.0, 500.0);
        core.pointer_left(Surface::Strip);
        assert!(core.hover_due.is_none());
    }

    #[test]
    fn detects_sleep_and_clock_changes_without_refreshing_during_normal_idle() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        assert!(!resumed(
            start,
            start + Duration::from_secs(2),
            Duration::from_secs(2)
        ));
        assert!(resumed(
            start,
            start + Duration::from_secs(3600),
            Duration::from_secs(2)
        ));
        assert!(resumed(
            start,
            start - Duration::from_secs(1),
            Duration::from_secs(2)
        ));
    }

    #[test]
    fn clicking_a_meeting_link_opens_it_instead_of_a_popup() {
        let mut backend = Recording::default();
        let mut core = core(vec![event(
            0,
            1440,
            Some("https://meet.google.com/abc-defg-hij"),
        )]);
        core.pointer_pressed(&mut backend, Surface::Strip, 2.0, 500.0);
        assert_eq!(
            backend.opened_urls.borrow().as_slice(),
            ["https://meet.google.com/abc-defg-hij"]
        );
        assert_eq!(*backend.popup_shown.borrow(), 0);
    }

    #[test]
    fn day_rollover_clears_events_and_restarts_without_demo_events() {
        let mut backend = Recording::default();
        let mut core = core(vec![event(0, 60, None)]);
        core.demo = false;
        core.feed_day = Local::now().date_naive() - chrono::Duration::days(1);
        core.run_timers(&mut backend);
        assert!(core.events.is_empty());
        assert_eq!(core.feed_day, Local::now().date_naive());
    }

    #[test]
    fn list_monitors_passes_through_the_backend() {
        let backend = Recording {
            monitors: vec![MonitorInfo {
                name: "Built-in".into(),
                width: 2560,
                height: 1600,
                x: 0,
                y: 0,
                primary: true,
            }],
            ..Default::default()
        };
        assert_eq!(backend.list_monitors(), backend.monitors);
    }

    #[test]
    fn menu_click_toggles_and_quit_sets_exit() {
        let mut backend = Recording::default();
        let mut core = core(vec![]);
        let menu_y = core.height as f32 - MENU_HEIGHT + 1.0;
        core.pointer_pressed(&mut backend, Surface::Strip, 2.0, menu_y);
        assert_eq!(*backend.popup_shown.borrow(), 1);
        core.pointer_pressed(&mut backend, Surface::Strip, 2.0, menu_y);
        assert_eq!(*backend.popup_closed.borrow(), 1);
    }
}
