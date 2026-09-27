use anyhow::Result;
use calstack_core::config::{create_default, Config};
use clap::Parser;
use std::path::PathBuf;
mod settings;

#[derive(Parser)]
#[command(version, about = "Calstack: a tiny Wayland calendar strip")]
struct Args {
    /// Configuration file; defaults to $XDG_CONFIG_HOME/calstack/config.toml.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Use the built-in fictional calendar instead of configured feeds.
    #[arg(long)]
    demo: bool,
    /// Logging filter, e.g. info or calstack_platform=debug.
    #[arg(long, default_value = "info")]
    log: String,
    /// Validate config, load calendars, and print today’s events without Wayland.
    #[arg(long)]
    check: bool,
    /// Open the graphical calendar and display settings.
    #[arg(long, conflicts_with_all = ["check", "demo", "sync_autostart", "autostart", "refresh"])]
    settings: bool,
    /// Ask the running app to reload its config and refresh calendars.
    #[arg(long, conflicts_with_all = ["check", "demo", "sync_autostart", "autostart"])]
    refresh: bool,
    #[arg(long, hide = true)]
    save_settings: bool,
    /// Apply startup.autostart from config without launching the app.
    #[arg(long, conflicts_with_all = ["check", "demo", "autostart"])]
    sync_autostart: bool,
    /// Internal entry point for the desktop session's autostart launcher.
    #[arg(long, hide = true, conflicts_with_all = ["check", "demo"])]
    autostart: bool,
}
fn main() -> Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_new(&args.log)?)
        .init();
    let path = args.config.unwrap_or_else(|| {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
            })
            .join("calstack/config.toml")
    });
    if !path.exists() {
        create_default(&path)?;
    }
    if args.save_settings {
        return settings::save(&path);
    }
    let config = Config::load(&path)?;
    if args.settings {
        return settings::open(&path, &config);
    }
    if args.refresh {
        std::fs::File::open(&path)?.set_modified(std::time::SystemTime::now())?;
        println!("Calendar refresh requested");
        return Ok(());
    }
    if args.sync_autostart {
        calstack_platform::desktop::sync_autostart(config.startup.autostart, &path)?;
        println!(
            "Start at login: {}",
            if config.startup.autostart {
                "enabled"
            } else {
                "disabled"
            }
        );
        return Ok(());
    }
    // A stale desktop entry must not override a subsequently disabled setting,
    // or try to start this Wayland app inside an X11 session.
    if args.autostart
        && (!config.startup.autostart || std::env::var_os("WAYLAND_DISPLAY").is_none())
    {
        return Ok(());
    }
    if args.check {
        println!("Configuration valid: {}", path.display());
        let events = if args.demo {
            calstack_core::demo_events()
        } else {
            let update = calstack_core::feeds::load(
                &config.calendar.feeds,
                &calstack_core::feeds::cache_dir(),
                chrono::Local::now().date_naive(),
                true,
            );
            for warning in &update.warnings {
                eprintln!("{warning}");
            }
            let expected = config.calendar.feeds.iter().filter(|f| f.enabled).count();
            if update.calendars.len() != expected {
                anyhow::bail!("one or more calendars could not be loaded and have no usable cache");
            }
            update
                .calendars
                .into_iter()
                .flat_map(|(_, events)| events)
                .collect()
        };
        for event in events {
            println!(
                "{}–{}  {}",
                calstack_core::time_label(event.start),
                calstack_core::time_label(event.end),
                event.title
            );
        }
        return Ok(());
    }
    let Some(_instance) = calstack_platform::desktop::lock_instance()? else {
        tracing::info!("Calstack is already running in this session");
        return Ok(());
    };
    if !args.demo {
        if let Err(error) =
            calstack_platform::desktop::sync_autostart(config.startup.autostart, &path)
        {
            tracing::warn!(%error, "could not apply start-at-login setting");
        }
    }
    calstack_platform::run(config, path, args.demo)
}
