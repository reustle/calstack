use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub display: Display,
    pub appearance: Appearance,
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
            width: 12,
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
        let config: Self =
            toml::from_str(&std::fs::read_to_string(path).context("read configuration")?)
                .context("parse configuration")?;
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
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
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(b"# Calstack first prototype: one built-in demo calendar.\n# Settings take effect after choosing Refresh.\n\n")?;
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
}
