use anyhow::Result;
use calstack_core::config::{create_default, Config};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "Calstack: a tiny Wayland calendar strip (demo prototype)"
)]
struct Args {
    /// Configuration file; defaults to $XDG_CONFIG_HOME/calstack/config.toml.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Use the built-in fictional calendar (currently the only calendar source).
    #[arg(long)]
    demo: bool,
    /// Logging filter, e.g. info or calstack_platform=debug.
    #[arg(long, default_value = "info")]
    log: String,
    /// Validate config and print the demo schedule without connecting to Wayland.
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
        for event in calstack_core::demo_events() {
            println!(
                "{}–{}  {}",
                calstack_core::time_label(event.start),
                calstack_core::time_label(event.end),
                event.title
            );
        }
        return Ok(());
    }
    calstack_platform::run(config, path)
}
