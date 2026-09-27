use anyhow::{bail, Context, Result};
use calstack_core::config::Config;
use serde::Deserialize;
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    process::{Command, Stdio},
};

pub fn open(path: &Path, config: &Config) -> Result<()> {
    let original = fs::read_to_string(path)?;
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(include_str!("settings.py"))
        .arg(std::env::current_exe()?)
        .arg(path.canonicalize()?)
        .stdin(Stdio::piped())
        .spawn()
        .context("graphical settings require Python 3, PyGObject, and GTK 4")?;
    let input = serde_json::json!({"config": config, "original": original});
    child
        .stdin
        .take()
        .context("settings input unavailable")?
        .write_all(serde_json::to_string(&input)?.as_bytes())?;
    if !child.wait()?.success() {
        bail!("graphical settings could not open; install Python 3, PyGObject, and GTK 4, or edit the config directly");
    }
    Ok(())
}
#[derive(Deserialize)]
struct Update {
    config: Config,
    original: String,
}
pub fn save(path: &Path) -> Result<()> {
    let mut input = String::new();
    std::io::stdin()
        .take(1024 * 1024 + 1)
        .read_to_string(&mut input)?;
    if input.len() > 1024 * 1024 {
        bail!("settings payload is too large");
    }
    let update: Update =
        serde_json::from_str(&input).map_err(|_| anyhow::anyhow!("invalid settings payload"))?;
    update.config.validate()?;
    let text = merge(&update.original, &update.config)?;
    if fs::read_to_string(path)? != update.original {
        bail!("configuration changed outside this window; close and reopen Settings before saving");
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.context("save configuration")?;
    if let Err(error) =
        calstack_platform::desktop::sync_autostart(update.config.startup.autostart, path)
    {
        eprintln!("Settings saved, but startup could not be updated: {error}");
    }
    Ok(())
}
fn merge(original: &str, config: &Config) -> Result<String> {
    let mut doc: toml_edit::DocumentMut = original.parse().context("read existing settings")?;
    let replacement: toml_edit::DocumentMut = toml::to_string(config)?.parse()?;
    for section in ["display", "appearance", "startup", "calendar"] {
        if doc.get(section).is_none() {
            doc[section] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let table = replacement[section]
            .as_table()
            .context("invalid settings section")?;
        for (key, value) in table.iter() {
            if key == "feeds" {
                continue;
            }
            let mut item = value.clone();
            if let (Some(old), Some(new)) = (
                doc[section].get(key).and_then(|i| i.as_value()),
                item.as_value_mut(),
            ) {
                *new.decor_mut() = old.decor().clone();
            }
            doc[section][key] = item;
        }
    }
    let old = doc["calendar"]
        .get("feeds")
        .and_then(|item| item.as_array_of_tables());
    let mut feeds = toml_edit::ArrayOfTables::new();
    if let Some(new) = replacement["calendar"]
        .get("feeds")
        .and_then(|item| item.as_array_of_tables())
    {
        for replacement in new.iter() {
            let name = replacement.get("name").and_then(|item| item.as_str());
            let mut table = old
                .and_then(|feeds| {
                    feeds
                        .iter()
                        .find(|feed| feed.get("name").and_then(|item| item.as_str()) == name)
                })
                .cloned()
                .unwrap_or_default();
            for key in ["name", "url", "path", "color", "enabled"] {
                if let Some(value) = replacement.get(key) {
                    table.insert(key, value.clone());
                } else {
                    table.remove(key);
                }
            }
            feeds.push(table);
        }
    }
    doc["calendar"]["feeds"] = toml_edit::Item::ArrayOfTables(feeds);
    Ok(doc.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_preserve_comments_and_private_feed_values() {
        let text = "# My calendar\n[display]\nwidth = 13 # comfortable\n\n[[calendar.feeds]]\nname='Work'\nurl='https://example.com/private?token=secret'\n";
        let mut config: Config = toml::from_str(text).unwrap();
        config.startup.autostart = true;
        config.display.width = 14;
        let saved = merge(text, &config).unwrap();
        assert!(saved.contains("# My calendar"));
        assert!(saved.contains("# comfortable"));
        let read: Config = toml::from_str(&saved).unwrap();
        assert_eq!(read.display.width, 14);
        assert!(read.startup.autostart);
        assert_eq!(read.calendar.feeds[0].url, config.calendar.feeds[0].url);
        config.calendar.feeds.clear();
        let cleared: Config = toml::from_str(&merge(text, &config).unwrap()).unwrap();
        assert!(cleared.calendar.feeds.is_empty());
    }
}
