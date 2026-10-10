//! Native settings window for macOS, replacing the GTK/Python tool that only
//! works on Linux. Mirrors `settings.py`'s fields; writes through the same
//! `settings::apply` (validate, merge preserving comments, atomic write,
//! sync autostart) used by the Linux save path.
use anyhow::Result;
use calstack_core::config::{Config, Feed};
use calstack_platform::MonitorInfo;
use eframe::egui;
use std::path::{Path, PathBuf};

struct FeedForm {
    index: Option<usize>,
    name: String,
    is_path: bool,
    source: String,
    color: String,
    enabled: bool,
    error: String,
}
impl FeedForm {
    fn blank() -> Self {
        Self {
            index: None,
            name: String::new(),
            is_path: false,
            source: String::new(),
            color: String::new(),
            enabled: true,
            error: String::new(),
        }
    }
    fn editing(index: usize, feed: &Feed) -> Self {
        Self {
            index: Some(index),
            name: feed.name.clone(),
            is_path: feed.path.is_some(),
            source: feed
                .path
                .as_ref()
                .map(|p| p.display().to_string())
                .or_else(|| feed.url.clone())
                .unwrap_or_default(),
            color: feed.color.clone().unwrap_or_default(),
            enabled: feed.enabled,
            error: String::new(),
        }
    }
}
fn is_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
}
fn validate_feed(existing: &[Feed], form: &FeedForm) -> Result<Feed, String> {
    let name = form.name.trim().to_string();
    let source = form.source.trim().to_string();
    if name.is_empty() || source.is_empty() {
        return Err("Enter a name and a URL or file path.".into());
    }
    if existing
        .iter()
        .enumerate()
        .any(|(i, f)| Some(i) != form.index && f.name == name)
    {
        return Err("Choose a unique calendar name.".into());
    }
    let color = form.color.trim();
    if !color.is_empty() && !is_hex_color(color) {
        return Err("Use a six-digit color such as #7F9BB3, or leave it blank.".into());
    }
    let color = (!color.is_empty()).then(|| color.to_string());
    if form.is_path {
        Ok(Feed {
            name,
            url: None,
            path: Some(source.into()),
            color,
            enabled: form.enabled,
        })
    } else {
        if !source.starts_with("https://")
            && !source.starts_with("http://")
            && !source.starts_with("webcal://")
        {
            return Err("Use an HTTP, HTTPS, or webcal subscription URL.".into());
        }
        Ok(Feed {
            name,
            url: Some(source),
            path: None,
            color,
            enabled: form.enabled,
        })
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Calendars,
    Display,
}

struct App {
    path: PathBuf,
    original: String,
    config: Config,
    monitors: Vec<MonitorInfo>,
    tab: Tab,
    status: String,
    feed_form: Option<FeedForm>,
}
impl App {
    fn save(&mut self) {
        match crate::settings::apply(&self.path, &self.original, &self.config) {
            Ok(()) => {
                self.original = std::fs::read_to_string(&self.path).unwrap_or_default();
                self.status = "Saved. The running strip will update automatically.".into();
            }
            Err(error) => self.status = error.to_string(),
        }
    }
    fn request_refresh(&mut self) {
        let outcome = std::env::current_exe().and_then(|exe| {
            std::process::Command::new(exe)
                .arg("--config")
                .arg(&self.path)
                .arg("--refresh")
                .output()
        });
        self.status = match outcome {
            Ok(out) if out.status.success() => "Refresh requested.".into(),
            Ok(out) => {
                let message = String::from_utf8_lossy(&out.stderr).trim().to_string();
                if message.is_empty() {
                    "Could not request a refresh.".into()
                } else {
                    message
                }
            }
            Err(error) => format!("Could not request a refresh: {error}"),
        };
    }

    fn calendars_ui(&mut self, ui: &mut egui::Ui) {
        if let Some(form) = &mut self.feed_form {
            ui.heading(if form.index.is_some() {
                "Edit calendar"
            } else {
                "Add calendar"
            });
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut form.name);
            });
            ui.horizontal(|ui| {
                ui.selectable_value(&mut form.is_path, false, "Subscription URL");
                ui.selectable_value(&mut form.is_path, true, "Local ICS file");
            });
            ui.horizontal(|ui| {
                ui.label(if form.is_path { "Path" } else { "URL" });
                ui.text_edit_singleline(&mut form.source);
            });
            ui.horizontal(|ui| {
                ui.label("Color (#RRGGBB, optional)");
                ui.text_edit_singleline(&mut form.color);
            });
            ui.checkbox(&mut form.enabled, "Enabled");
            if !form.error.is_empty() {
                ui.colored_label(egui::Color32::from_rgb(192, 57, 43), &form.error);
            }
            let mut cancel = false;
            let mut done = false;
            ui.horizontal(|ui| {
                cancel = ui.button("Cancel").clicked();
                done = ui.button("Done").clicked();
            });
            if cancel {
                self.feed_form = None;
            } else if done {
                match validate_feed(&self.config.calendar.feeds, form) {
                    Ok(feed) => {
                        match form.index {
                            Some(i) => self.config.calendar.feeds[i] = feed,
                            None => self.config.calendar.feeds.push(feed),
                        }
                        self.status = "Calendar updated in this draft. Save to apply.".into();
                        self.feed_form = None;
                    }
                    Err(message) => form.error = message,
                }
            }
            return;
        }
        ui.label("Add an ICS subscription URL or a local .ics file.");
        ui.add_space(8.0);
        if self.config.calendar.feeds.is_empty() {
            ui.label("No calendars yet. Add one to get started.");
        }
        let mut edit_index = None;
        let mut remove_index = None;
        for (i, feed) in self.config.calendar.feeds.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.checkbox(&mut feed.enabled, "");
                ui.label(&feed.name);
                if ui.button("Edit").clicked() {
                    edit_index = Some(i);
                }
                if ui.button("Remove").clicked() {
                    remove_index = Some(i);
                }
            });
        }
        if let Some(i) = remove_index {
            self.config.calendar.feeds.remove(i);
            self.status =
                "Calendar removed from this draft. Save to apply, or close to discard.".into();
        }
        if let Some(i) = edit_index {
            self.feed_form = Some(FeedForm::editing(i, &self.config.calendar.feeds[i]));
        }
        ui.add_space(8.0);
        if ui.button("Add calendar").clicked() {
            self.feed_form = Some(FeedForm::blank());
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label("Refresh interval (minutes)");
            ui.add(egui::DragValue::new(&mut self.config.calendar.refresh_minutes).range(1..=1440));
        });
        if ui.button("Refresh saved calendars").clicked() {
            self.request_refresh();
        }
    }

    fn display_ui(&mut self, ui: &mut egui::Ui) {
        let cfg = &mut self.config;
        ui.horizontal(|ui| {
            ui.label("Strip width (pixels)");
            ui.add(egui::DragValue::new(&mut cfg.display.width).range(8..=48));
        });
        ui.horizontal(|ui| {
            ui.label("Day starts (HH:MM)");
            ui.text_edit_singleline(&mut cfg.display.day_start);
        });
        ui.horizontal(|ui| {
            ui.label("Day ends (HH:MM)");
            ui.text_edit_singleline(&mut cfg.display.day_end);
        });
        ui.horizontal(|ui| {
            ui.label("Monitor");
            egui::ComboBox::from_id_salt("monitor")
                .selected_text(cfg.display.monitor.clone())
                .show_ui(ui, |ui| {
                    for monitor in &self.monitors {
                        let label = if monitor.primary {
                            format!("{} (primary)", monitor.name)
                        } else {
                            monitor.name.clone()
                        };
                        ui.selectable_value(&mut cfg.display.monitor, monitor.name.clone(), label);
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label("Appearance");
            egui::ComboBox::from_id_salt("theme")
                .selected_text(match cfg.appearance.theme.as_str() {
                    "dark" => "Dark",
                    "light" => "Light",
                    _ => "System",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut cfg.appearance.theme, "auto".into(), "System");
                    ui.selectable_value(&mut cfg.appearance.theme, "dark".into(), "Dark");
                    ui.selectable_value(&mut cfg.appearance.theme, "light".into(), "Light");
                });
        });
        ui.checkbox(&mut cfg.appearance.show_now_marker, "Show current time");
        ui.horizontal(|ui| {
            ui.label("Past event opacity");
            ui.add(egui::Slider::new(
                &mut cfg.appearance.past_opacity,
                0.0..=1.0,
            ));
        });
        ui.horizontal(|ui| {
            ui.label("Future event opacity");
            ui.add(egui::Slider::new(
                &mut cfg.appearance.future_opacity,
                0.0..=1.0,
            ));
        });
        ui.horizontal(|ui| {
            ui.label("Active event opacity");
            ui.add(egui::Slider::new(
                &mut cfg.appearance.active_opacity,
                0.0..=1.0,
            ));
        });
        ui.add_space(8.0);
        ui.checkbox(&mut cfg.startup.autostart, "Start at login");
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Calendars, "Calendars");
                ui.selectable_value(&mut self.tab, Tab::Display, "Display & Startup");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Save").clicked() {
                        self.save();
                    }
                });
            });
            ui.add_space(4.0);
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.label(&self.status);
            ui.add_space(4.0);
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| match self.tab {
                Tab::Calendars => self.calendars_ui(ui),
                Tab::Display => self.display_ui(ui),
            });
        });
    }
}

fn fetch_monitors() -> Vec<MonitorInfo> {
    std::env::current_exe()
        .and_then(|exe| {
            std::process::Command::new(exe)
                .arg("--list-displays-json")
                .output()
        })
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| serde_json::from_slice(&out.stdout).ok())
        .unwrap_or_default()
}

pub fn run(path: &Path, original: String, config: Config) -> Result<()> {
    let app = App {
        path: path.to_path_buf(),
        original,
        config,
        monitors: fetch_monitors(),
        tab: Tab::Calendars,
        status: "Changes apply to the running strip after saving.".into(),
        feed_form: None,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            // The primary display's logical origin is always (0,0), regardless
            // of how other monitors are arranged; an explicit position keeps
            // this from landing on a smaller secondary display (where the
            // default placement can clamp the window down to fit) or at a
            // negative offset if that display sits above/left of the primary.
            .with_position([160.0, 160.0])
            .with_inner_size([620.0, 650.0])
            .with_min_inner_size([480.0, 450.0])
            .with_title("Calstack Settings"),
        ..Default::default()
    };
    eframe::run_native(
        "Calstack Settings",
        options,
        Box::new(|_cc| Ok(Box::new(app))),
    )
    .map_err(|error| anyhow::anyhow!("could not open settings window: {error}"))
}
