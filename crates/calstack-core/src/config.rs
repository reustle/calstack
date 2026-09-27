use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub display: Display,
    pub appearance: Appearance,
    pub calendar: Calendar,
    pub startup: Startup,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Startup {
    pub autostart: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Calendar {
    pub refresh_minutes: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub feeds: Vec<Feed>,
}
impl Default for Calendar {
    fn default() -> Self {
        Self {
            refresh_minutes: 10,
            feeds: Vec::new(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Feed {
    pub name: String,
    pub url: Option<String>,
    pub path: Option<std::path::PathBuf>,
    pub color: Option<String>,
    #[serde(default = "enabled")]
    pub enabled: bool,
}
fn enabled() -> bool {
    true
}
impl Feed {
    pub fn color_rgb(&self) -> Result<Option<[u8; 3]>> {
        self.color
            .as_deref()
            .map(|value| {
                let hex = value
                    .strip_prefix('#')
                    .context("calendar color must be #RRGGBB")?;
                if hex.len() != 6 || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
                    bail!("calendar color must be #RRGGBB");
                }
                Ok([
                    u8::from_str_radix(&hex[0..2], 16)?,
                    u8::from_str_radix(&hex[2..4], 16)?,
                    u8::from_str_radix(&hex[4..6], 16)?,
                ])
            })
            .transpose()
    }
    pub fn remote_url(&self) -> Result<url::Url> {
        let raw = self.url.as_deref().context("missing feed URL")?;
        let normalized = raw
            .strip_prefix("webcal://")
            .map(|rest| format!("https://{rest}"));
        let url = url::Url::parse(normalized.as_deref().unwrap_or(raw))
            .map_err(|_| anyhow::anyhow!("invalid feed URL"))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            bail!("feed URL must use HTTP, HTTPS, or webcal, without embedded credentials");
        }
        Ok(url)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Display {
    pub width: u32,
    pub day_start: String,
    pub day_end: String,
    pub monitor: String,
    pub reserve_space: bool,
}
impl Default for Display {
    fn default() -> Self {
        Self {
            width: 13,
            day_start: "06:00".into(),
            day_end: "24:00".into(),
            monitor: "primary".into(),
            reserve_space: true,
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    pub theme: String,
    pub past_opacity: f64,
    pub future_opacity: f64,
    pub active_opacity: f64,
    pub show_now_marker: bool,
}
impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: "auto".into(),
            past_opacity: 0.35,
            future_opacity: 0.78,
            active_opacity: 1.0,
            show_now_marker: true,
        }
    }
}
pub fn minute(value: &str) -> Result<i32> {
    let (h, m) = value.split_once(':').context("time must be HH:MM")?;
    let h: i32 = h.parse()?;
    let m: i32 = m.parse()?;
    if !(0..=24).contains(&h) || !(0..60).contains(&m) || (h == 24 && m != 0) {
        bail!("invalid time: {value}");
    }
    Ok(h * 60 + m)
}
impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let mut config: Self = toml::from_str(
            &std::fs::read_to_string(path).context("read configuration")?,
        )
        .map_err(|_| {
            anyhow::anyhow!("invalid TOML configuration; check field names and value types")
        })?;
        config.validate()?;
        for feed in &mut config.calendar.feeds {
            if let Some(local) = &mut feed.path {
                if local.is_relative() {
                    *local = path.parent().unwrap_or(Path::new(".")).join(&*local);
                }
            }
        }
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        if !(1..=1440).contains(&self.calendar.refresh_minutes) {
            bail!("calendar refresh_minutes must be 1–1440");
        }
        let mut names = std::collections::HashSet::new();
        for feed in &self.calendar.feeds {
            if feed.name.trim().is_empty() || !names.insert(&feed.name) {
                bail!("calendar names must be nonempty and unique");
            }
            if feed.url.is_some() == feed.path.is_some() {
                bail!("each calendar needs exactly one URL or path");
            }
            if feed.url.is_some() {
                feed.remote_url()?;
            }
            feed.color_rgb()?;
        }
        if !(8..=48).contains(&self.display.width) {
            bail!("width must be 8–48 logical pixels");
        }
        if minute(&self.display.day_start)? >= minute(&self.display.day_end)? {
            bail!("day_start must precede day_end");
        }
        if !["auto", "dark", "light"].contains(&self.appearance.theme.as_str()) {
            bail!("theme must be auto, dark, or light");
        }
        for alpha in [
            self.appearance.past_opacity,
            self.appearance.future_opacity,
            self.appearance.active_opacity,
        ] {
            if !alpha.is_finite() || !(0.0..=1.0).contains(&alpha) {
                bail!("opacity must be between 0 and 1");
            }
        }
        Ok(())
    }
    pub fn range(&self) -> (i32, i32) {
        (
            minute(&self.display.day_start).unwrap(),
            minute(&self.display.day_end).unwrap(),
        )
    }
}

pub fn create_default(path: &Path) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(b"# Calstack configuration. Choose Refresh after editing.\n# Add [[calendar.feeds]] entries with a name and URL or local path.\n\n")?;
    file.write_all(toml::to_string_pretty(&Config::default())?.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_validation() {
        Config::default().validate().unwrap();
        assert_eq!(minute("24:00").unwrap(), 1440);
        assert!(minute("24:01").is_err());
        assert!(minute("-1:00").is_err());
        let mut config = Config::default();
        config.display.day_end = "05:00".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn feed_configuration_and_default_file_can_be_extended() {
        let mut text = toml::to_string_pretty(&Config::default()).unwrap();
        text.push_str("\n[[calendar.feeds]]\nname = 'Work'\nurl = 'webcal://example.com/calendar.ics'\ncolor = '#123456'\n");
        let mut config: Config = toml::from_str(&text).unwrap();
        config.validate().unwrap();
        assert_eq!(
            config.calendar.feeds[0].remote_url().unwrap().scheme(),
            "https"
        );
        assert!(config.calendar.feeds[0].enabled);
        config.calendar.feeds[0].path = Some("other.ics".into());
        assert!(config.validate().is_err());
        config.calendar.feeds[0].path = None;
        config.calendar.feeds[0].color = Some("#abcdef0".into());
        assert!(config.validate().is_err());
        config.calendar.feeds[0].color = None;
        config.calendar.feeds[0].url = Some("file:///etc/passwd".into());
        assert!(config.validate().is_err());
    }
}
