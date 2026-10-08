//! Login-item and single-instance integration shared by the CLI and the
//! native strip. The skeleton (ownership check, idempotent atomic write,
//! instance lock) is shared; each OS supplies where its start-at-login entry
//! lives, what it contains, and how to (de)activate it.
use anyhow::{bail, Context, Result};
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

#[cfg(target_os = "linux")]
#[path = "desktop/linux.rs"]
mod os;
#[cfg(target_os = "macos")]
#[path = "desktop/macos.rs"]
mod os;

pub fn config_home() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        return Ok(path);
    }
    Ok(home()?.join(".config"))
}
fn home() -> Result<PathBuf> {
    Ok(PathBuf::from(
        std::env::var_os("HOME").context("HOME is not set")?,
    ))
}

/// Synchronize only Calstack's own entry; never touch anything else.
pub fn sync_autostart(enabled: bool, config: &Path) -> Result<()> {
    let entry = os::entry_path()?;
    let executable = std::env::current_exe().context("locate Calstack executable")?;
    sync_at(&entry, enabled, &executable, config)
}
fn sync_at(entry: &Path, enabled: bool, executable: &Path, config: &Path) -> Result<()> {
    let existing = match fs::read_to_string(entry) {
        Ok(content) => Some(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context(format!("read {}", os::ENTRY_KIND)),
    };
    if existing
        .as_deref()
        .is_some_and(|text| !os::is_managed(text))
    {
        bail!(
            "{} was created outside Calstack; move it aside before using the startup setting",
            entry.display()
        );
    }
    if !enabled {
        if existing.is_some() {
            os::deactivate(entry);
            fs::remove_file(entry).context(format!("remove {}", os::ENTRY_KIND))?;
        }
        return Ok(());
    }
    let config = config
        .canonicalize()
        .context("resolve configuration path")?;
    let executable = executable
        .canonicalize()
        .context("resolve executable path")?;
    let content = os::entry(&executable, &config)?;
    if existing.as_deref() == Some(&content) {
        return Ok(());
    }
    fs::create_dir_all(entry.parent().context("autostart directory missing")?)?;
    write_private(entry, &content).context(format!("write {}", os::ENTRY_KIND))?;
    os::activate(entry)
}

/// Atomically replace `path` with `content`, readable only by the user.
pub fn write_private(path: &Path, content: &str) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Keep the file open for the application's lifetime. Never unlink the lock:
/// another process could otherwise acquire a different inode during shutdown.
pub fn lock_instance() -> Result<Option<File>> {
    lock_at(&os::lock_path()?)
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
    pub(super) fn directory() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "calstack-desktop-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        root
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
