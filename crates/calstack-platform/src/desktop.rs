//! Freedesktop integration shared by the CLI and the native strip.
use anyhow::{bail, Context, Result};
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    hash::{DefaultHasher, Hash, Hasher},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

const MANAGED: &str = "X-Calstack-Managed=true";

pub fn config_home() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        return Ok(path);
    }
    Ok(PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".config"))
}

/// Synchronize only Calstack's own entry; never touch compositor configuration.
pub fn sync_autostart(enabled: bool, config: &Path) -> Result<()> {
    let entry = config_home()?.join("autostart/calstack.desktop");
    let executable = std::env::current_exe().context("locate Calstack executable")?;
    sync_at(&entry, enabled, &executable, config)
}
fn sync_at(entry: &Path, enabled: bool, executable: &Path, config: &Path) -> Result<()> {
    let existing = match fs::read_to_string(entry) {
        Ok(content) => Some(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("read autostart entry"),
    };
    if existing
        .as_ref()
        .is_some_and(|text| !text.lines().any(|line| line == MANAGED))
    {
        bail!("autostart/calstack.desktop was created outside Calstack; move it aside before using the startup setting");
    }
    if !enabled {
        if existing.is_some() {
            fs::remove_file(entry).context("remove autostart entry")?;
        }
        return Ok(());
    }
    let config = config
        .canonicalize()
        .context("resolve configuration path")?;
    let executable = executable
        .canonicalize()
        .context("resolve executable path")?;
    let content = autostart_entry(&executable, &config)?;
    if existing.as_deref() == Some(&content) {
        return Ok(());
    }
    fs::create_dir_all(entry.parent().context("autostart directory missing")?)?;
    let temporary = entry.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, entry)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.context("write autostart entry")
}
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
fn autostart_entry(executable: &Path, config: &Path) -> Result<String> {
    let raw = path_text(executable)?;
    if raw.contains('=') {
        bail!("desktop executable paths cannot contain '='");
    }
    Ok(format!("[Desktop Entry]\nType=Application\nName=Calstack\nComment=Calendar strip for Wayland\nExec={} --autostart --config {}\nTryExec={}\nIcon=x-office-calendar\nTerminal=false\n{MANAGED}\n", exec_argument(executable)?, exec_argument(config)?, raw.replace('\\', "\\\\")))
}

/// Keep the file open for the application's lifetime. Never unlink the lock:
/// another process could otherwise acquire a different inode during shutdown.
pub fn lock_instance() -> Result<Option<File>> {
    let runtime = PathBuf::from(
        std::env::var_os("XDG_RUNTIME_DIR")
            .context("XDG_RUNTIME_DIR is not set; run inside a Wayland session")?,
    );
    let mut hash = DefaultHasher::new();
    std::env::var_os("WAYLAND_DISPLAY").hash(&mut hash);
    lock_at(&runtime.join(format!("calstack-{:x}.lock", hash.finish())))
}
fn lock_at(path: &Path) -> Result<Option<File>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error).context("lock Calstack instance"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn directory() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "calstack-desktop-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }
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
    #[test]
    fn lock_prevents_duplicates_and_releases_on_exit() {
        let root = directory();
        let path = root.join("instance.lock");
        let first = lock_at(&path).unwrap().unwrap();
        assert!(lock_at(&path).unwrap().is_none());
        drop(first);
        assert!(lock_at(&path).unwrap().is_some());
        fs::remove_dir_all(root).unwrap();
    }
}
