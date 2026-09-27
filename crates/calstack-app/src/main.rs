use anyhow::Result;
use calstack_core::config::{create_default, Config};
use clap::Parser;
use std::path::PathBuf;

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
    let config = Config::load(&path)?;
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
    calstack_platform::run(config, path, args.demo)
}
