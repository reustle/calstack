use anyhow::{Context, Result};
use calstack_render::{TextRenderer, Typography};
use std::{path::PathBuf, process::Command};
use toml::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct FontSettings {
    pub family: String,
    pub sizes: Typography,
    // Omarchy font set changes this alias file. Tracking contents avoids running
    // fontconfig or rebuilding font rasterizers on every idle poll.
    fontconfig: Vec<Option<Vec<u8>>>,
}
fn text_file(path: PathBuf) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.into()),
    }
}
fn gsetting(key: &str) -> Option<String> {
    let out = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", key])
        .output()
        .ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .trim_matches('\'')
            .to_owned()
    })
}
fn gtk_font(description: &str, scale: f32) -> (String, Typography) {
    let (family, size) = description.rsplit_once(' ').unwrap_or((description, "11"));
    let (number, pixel_size) = size.strip_suffix("px").map_or((size, false), |n| (n, true));
    let size = number
        .parse::<f32>()
        .ok()
        .filter(|n| n.is_finite() && *n > 0.0)
        .unwrap_or(11.0);
    // Pango descriptions default to points. Output scaling is applied separately.
    let base = size * if pixel_size { 1.0 } else { 96.0 / 72.0 } * scale;
    (family.to_owned(), Typography::from_base(base))
}
fn omarchy_sizes(shell: &str, user: &str) -> Result<Typography> {
    let theme: Value = toml::from_str(shell)?;
    let user: Value = toml::from_str(user)?;
    let number = |key: &str| {
        user.get("font")
            .and_then(|v| v.get(key))
            .or_else(|| theme.get("font").and_then(|v| v.get(key)))
            .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|n| n as f64)))
            .filter(|n| n.is_finite() && *n > 0.0)
            .map(|n| (n as f32).round().max(1.0))
    };
    let mut t = Typography::from_base(number("base-size").unwrap_or(12.0));
    t.caption = number("caption").unwrap_or(t.caption);
    t.small = number("body-small").unwrap_or(t.small);
    t.body = number("body").unwrap_or(t.body);
    t.subtitle = number("subtitle").unwrap_or(t.subtitle);
    t.title = number("title").unwrap_or(t.title);
    t.icon = number("icon").unwrap_or(t.title);
    Ok(t)
}
pub fn settings() -> Result<FontSettings> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME not set")?);
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let theme = home.join(".local/state/omarchy/current/theme");
    let (family, sizes) = if theme.join("colors.toml").exists() {
        (
            "monospace".to_owned(),
            omarchy_sizes(
                &text_file(theme.join("shell.toml"))?,
                &text_file(home.join(".config/omarchy/shell.toml"))?,
            )?,
        )
    } else {
        let mut font = None;
        for version in ["gtk-4.0", "gtk-3.0"] {
            let content = text_file(config.join(version).join("settings.ini"))?;
            font = content
                .lines()
                .filter_map(|line| line.split_once('='))
                .find(|(key, _)| key.trim() == "gtk-font-name")
                .map(|(_, value)| value.trim().to_owned());
            if font.is_some() {
                break;
            }
        }
        let font = font
            .or_else(|| gsetting("font-name"))
            .unwrap_or_else(|| "sans-serif 11".into());
        let scale = gsetting("text-scaling-factor")
            .and_then(|s| s.parse::<f32>().ok())
            .filter(|n| n.is_finite() && *n > 0.0)
            .unwrap_or(1.0);
        gtk_font(&font, scale)
    };
    let fontconfig = [
        home.join(".config/fontconfig/fonts.conf"),
        config.join("fontconfig/fonts.conf"),
        home.join(".fonts.conf"),
        PathBuf::from("/etc/fonts/local.conf"),
    ]
    .iter()
    .map(std::fs::read)
    .map(Result::ok)
    .collect();
    Ok(FontSettings {
        family,
        sizes,
        fontconfig,
    })
}
pub fn renderer(settings: &FontSettings) -> Result<TextRenderer> {
    let load = |pattern: &str| -> Result<Vec<u8>> {
        let out = Command::new("fc-match")
            .args(["-f", "%{file}", pattern])
            .output()
            .context("resolve desktop font")?;
        anyhow::ensure!(out.status.success(), "fontconfig failed");
        std::fs::read(String::from_utf8(out.stdout)?.trim()).context("read desktop font")
    };
    let mut renderer = TextRenderer::new(
        load(&settings.family)?,
        load(&format!("{}:weight=bold", settings.family))?,
    )
    .map_err(anyhow::Error::msg)?;
    renderer.typography = settings.sizes;
    Ok(renderer)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn theme_base_and_individual_user_overrides() {
        let t = omarchy_sizes("[font]\nbase-size=18\ntitle=25", "[font]\nbody-small=17").unwrap();
        assert_eq!(
            (t.body, t.title, t.small, t.caption),
            (18.0, 25.0, 17.0, 15.0)
        );
        let t = omarchy_sizes("[font]\nbase-size=18", "[font]\nbase-size=12").unwrap();
        assert_eq!(t, Typography::default());
    }
    #[test]
    fn gtk_points_pixels_and_text_scaling() {
        let (family, t) = gtk_font("Adwaita Sans 12", 1.5);
        assert_eq!(family, "Adwaita Sans");
        assert_eq!(t.body, 24.0);
        assert_eq!(gtk_font("Sans 12px", 1.0).1.body, 12.0);
    }
}
