//! launchd LaunchAgent for start-at-login, and a per-user instance lock.
use anyhow::{Context, Result};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const LABEL: &str = "org.calstack.autostart";
pub(super) const ENTRY_KIND: &str = "launch agent";

pub(super) fn entry_path() -> Result<PathBuf> {
    Ok(super::home()?
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist")))
}
pub(super) fn is_managed(text: &str) -> bool {
    text.contains(&format!(">{LABEL}<"))
}
/// Writing the plist alone only takes effect at next login; load it now.
/// A prior bootout failure (nothing was loaded yet) is expected and silent.
pub(super) fn activate(entry: &Path) -> Result<()> {
    let domain = format!("gui/{}", gui_domain()?);
    let path = entry.display().to_string();
    let _ = quiet(Command::new("launchctl").args(["bootout", &domain, &path]));
    quiet(Command::new("launchctl").args(["bootstrap", &domain, &path]))
        .context("launchctl bootstrap")?;
    Ok(())
}
pub(super) fn deactivate(entry: &Path) {
    if let Ok(uid) = gui_domain() {
        let _ = quiet(Command::new("launchctl").args([
            "bootout",
            &format!("gui/{uid}"),
            &entry.display().to_string(),
        ]));
    }
}
fn quiet(command: &mut Command) -> Result<std::process::ExitStatus> {
    Ok(command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?)
}
fn gui_domain() -> Result<String> {
    let out = Command::new("id")
        .arg("-u")
        .output()
        .context("resolve current user id")?;
    anyhow::ensure!(out.status.success(), "id -u failed");
    Ok(String::from_utf8(out.stdout)?.trim().to_owned())
}
fn plist_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
pub(super) fn entry(executable: &Path, config: &Path) -> Result<String> {
    let exe = plist_escape(
        executable
            .to_str()
            .context("desktop launch paths must be UTF-8")?,
    );
    let config = plist_escape(
        config
            .to_str()
            .context("desktop launch paths must be UTF-8")?,
    );
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{exe}</string>
		<string>--autostart</string>
		<string>--config</string>
		<string>{config}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<false/>
</dict>
</plist>
"#
    ))
}

pub(super) fn lock_path() -> Result<PathBuf> {
    let dir = super::home()?.join("Library/Application Support/calstack");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("calstack.lock"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plist_escapes_reserved_characters() {
        let plist = entry(Path::new("/a&b/<x>"), Path::new("/c.toml")).unwrap();
        assert!(plist.contains("/a&amp;b/&lt;x&gt;"));
    }
}
