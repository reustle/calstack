use std::{fs, process::Command};

#[test]
fn check_loads_relative_ics_and_demo_is_explicit() {
    let root = std::env::temp_dir().join(format!("calstack-cli-test-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let date = chrono::Local::now().format("%Y%m%d");
    fs::write(root.join("calendar.ics"), format!("BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:cli-test\nDTSTART:{date}T140000\nDURATION:PT45M\nSUMMARY:Local ICS integration test\nEND:VEVENT\nEND:VCALENDAR\n")).unwrap();
    let config = root.join("config.toml");
    fs::write(
        &config,
        "[[calendar.feeds]]\nname='Local'\npath='calendar.ics'\n",
    )
    .unwrap();
    let run = |demo: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_calstack"));
        command
            .args(["--check", "--config"])
            .arg(&config)
            .env("XDG_CACHE_HOME", root.join("cache"));
        if demo {
            command.arg("--demo");
        }
        command.output().unwrap()
    };
    let output = run(false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("14:00–14:45  Local ICS integration test")
    );
    let output = run(true);
    assert!(output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Local ICS integration test"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Morning focus"));
    // A bad update retains the last successfully parsed file.
    fs::write(root.join("calendar.ics"), "not an ICS file").unwrap();
    let output = run(false);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Local ICS integration test"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("keeping last good data"));
    fs::remove_dir_all(&root).unwrap();
}
