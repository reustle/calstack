//! Read the same theme and user overrides as Omarchy's Color.qml.
use anyhow::{Context, Result};
use calstack_render::{mix, Palette};
use std::path::{Path, PathBuf};
use toml::Value;

fn hex(value: &str) -> Option<[u8; 3]> {
    let s = value.strip_prefix('#')?;
    if s.len() != 6 || !s.is_ascii() {
        return None;
    }
    Some([
        u8::from_str_radix(&s[0..2], 16).ok()?,
        u8::from_str_radix(&s[2..4], 16).ok()?,
        u8::from_str_radix(&s[4..6], 16).ok()?,
    ])
}
fn at<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .try_fold(value, |current, key| current.get(key))
}
fn resolve(
    name: &str,
    colors: &Value,
    shell: &Value,
    user: &Value,
    depth: usize,
) -> Option<[u8; 3]> {
    if depth > 8 {
        return None;
    }
    if let Some(color) = hex(name) {
        return Some(color);
    }
    let value = at(user, name)
        .or_else(|| at(shell, name))
        .or_else(|| at(colors, name))?
        .as_str()?;
    resolve(value, colors, shell, user, depth + 1)
}
fn optional_file(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.into()),
    }
}
pub fn load() -> Result<Palette> {
    // These paths intentionally match Omarchy's shell, which currently uses HOME
    // rather than XDG_STATE_HOME for its current-theme directory.
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME not set")?);
    let theme = home.join(".local/state/omarchy/current/theme");
    let colors = std::fs::read_to_string(theme.join("colors.toml"))?;
    let shell = optional_file(&theme.join("shell.toml"))?;
    let user = optional_file(&home.join(".config/omarchy/shell.toml"))?;
    parse(&colors, &shell, &user)
}
fn parse(colors: &str, shell: &str, user: &str) -> Result<Palette> {
    let colors: Value = toml::from_str(colors)?;
    let shell: Value = toml::from_str(shell)?;
    let user: Value = toml::from_str(user)?;
    let get = |name| resolve(name, &colors, &shell, &user, 0);
    let background = get("bar.background")
        .or_else(|| get("background"))
        .context("missing theme background")?;
    let text = get("bar.text")
        .or_else(|| get("foreground"))
        .context("missing theme foreground")?;
    let event = get("muted").unwrap_or(text);
    let normal = get("controls.normal-color").unwrap_or(text);
    let fill = at(&user, "controls.normal-fill-alpha")
        .or_else(|| at(&shell, "controls.normal-fill-alpha"))
        .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|n| n as f64)))
        .unwrap_or(0.04)
        .clamp(0.0, 1.0) as f32;
    Ok(Palette {
        background,
        text,
        muted: text,
        event,
        border: get("border").unwrap_or_else(|| mix(background, text, 0.16)),
        card: mix(background, normal, fill),
        now: get("bar.active").or_else(|| get("red")).unwrap_or(text),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    const COLORS: &str="background='#1f1f28'\nforeground='#dcd7ba'\nmuted='#54546d'\nselection='#363646'\nred='#c34043'";
    #[test]
    fn uses_real_palette_and_user_bar_overrides() {
        let base = parse(COLORS, "", "").unwrap();
        assert_eq!(base.background, [31, 31, 40]);
        assert_eq!(base.text, [220, 215, 186]);
        assert_eq!(base.event, [84, 84, 109]);
        let custom = parse(
            COLORS,
            "[bar]\nbackground='#123456'\ntext='foreground'",
            "[bar]\nbackground='#abcdef'",
        )
        .unwrap();
        assert_eq!(custom.background, [171, 205, 239]);
        assert_eq!(custom.text, base.text);
    }
    #[test]
    fn updated_theme_changes_all_roles_and_bad_input_is_rejected() {
        let old = parse(COLORS, "", "").unwrap();
        let updated = parse(
            "background='#ffffff'\nforeground='#222222'\nmuted='#999999'",
            "",
            "",
        )
        .unwrap();
        assert_ne!(old, updated);
        assert_eq!(updated.event, [153; 3]);
        assert!(parse("broken TOML", "", "").is_err());
        assert!(parse("background='bad'\nforeground='#ffffff'", "", "").is_err());
    }
}
