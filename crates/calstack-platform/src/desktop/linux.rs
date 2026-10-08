//! Freedesktop autostart entry and a per-Wayland-session instance lock.
use anyhow::{bail, Context, Result};
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
};

const MANAGED: &str = "X-Calstack-Managed=true";
pub(super) const ENTRY_KIND: &str = "autostart entry";

pub(super) fn entry_path() -> Result<PathBuf> {
    Ok(super::config_home()?.join("autostart/calstack.desktop"))
}
pub(super) fn is_managed(text: &str) -> bool {
    text.lines().any(|line| line == MANAGED)
}
/// The session reads the entry at next login; nothing to load now.
pub(super) fn activate(_entry: &Path) -> Result<()> {
    Ok(())
}
pub(super) fn deactivate(_entry: &Path) {}

fn path_text(path: &Path) -> Result<&str> {
    let text = path
        .to_str()
        .context("desktop launch paths must be UTF-8")?;
    if text.chars().any(char::is_control) {
        bail!("desktop launch paths cannot contain control characters");
    }
    Ok(text)
}
/// Exec is a desktop-entry command line, not a shell command. Escape both the
/// desktop string layer and its quoted-argument layer, including literal %.
fn exec_argument(path: &Path) -> Result<String> {
    let mut quoted = String::from("\"");
    for c in path_text(path)?.chars() {
        match c {
            '\\' => quoted.push_str("\\\\\\\\"),
            '"' | '`' | '$' => {
                quoted.push_str("\\\\");
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    Ok(quoted)
}
pub(super) fn entry(executable: &Path, config: &Path) -> Result<String> {
    let raw = path_text(executable)?;
    if raw.contains('=') {
        bail!("desktop executable paths cannot contain '='");
    }
    Ok(format!("[Desktop Entry]\nType=Application\nName=Calstack\nComment=Calendar strip for Wayland\nExec={} --autostart --config {}\nTryExec={}\nIcon=x-office-calendar\nTerminal=false\n{MANAGED}\n", exec_argument(executable)?, exec_argument(config)?, raw.replace('\\', "\\\\")))
}

pub(super) fn lock_path() -> Result<PathBuf> {
    let runtime = PathBuf::from(
        std::env::var_os("XDG_RUNTIME_DIR")
            .context("XDG_RUNTIME_DIR is not set; run inside a Wayland session")?,
    );
    let mut hash = DefaultHasher::new();
    std::env::var_os("WAYLAND_DISPLAY").hash(&mut hash);
    Ok(runtime.join(format!("calstack-{:x}.lock", hash.finish())))
}

#[cfg(test)]
mod tests {
    use super::super::{sync_at, tests::directory};
    use super::*;
    use std::fs;
    #[test]
    fn enable_disable_is_idempotent_and_preserves_other_entries() {
        let root = directory();
        let executable = root.join("calstack");
        fs::write(&executable, "").unwrap();
        let config = root.join("config.toml");
        fs::write(&config, "").unwrap();
        let entry = root.join("autostart/calstack.desktop");
        sync_at(&entry, true, &executable, &config).unwrap();
        let content = fs::read_to_string(&entry).unwrap();
        assert!(content.contains("--autostart --config"));
        assert!(!content.contains("--demo"));
        sync_at(&entry, true, &executable, &config).unwrap();
        assert_eq!(fs::read_to_string(&entry).unwrap(), content);
        sync_at(&entry, false, &executable, &config).unwrap();
        sync_at(&entry, false, &executable, &config).unwrap();
        assert!(!entry.exists());
        fs::write(&entry, "[Desktop Entry]\nExec=custom-wrapper\n").unwrap();
        assert!(sync_at(&entry, true, &executable, &config).is_err());
        assert!(sync_at(&entry, false, &executable, &config).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn desktop_arguments_escape_metacharacters() {
        assert_eq!(
            exec_argument(Path::new("/a b/%x/$HOME/\"q\"/\\")).unwrap(),
            "\"/a b/%%x/\\\\$HOME/\\\\\"q\\\\\"/\\\\\\\\\""
        );
        assert!(exec_argument(Path::new("/bad\npath")).is_err());
    }
}
