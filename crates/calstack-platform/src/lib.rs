//! Linux and macOS desktop integration. Calendar/layout/rendering crates stay
//! portable; `app` holds the state/decision logic shared by every backend,
//! and `desktop` the shared login-item/instance-lock skeleton.
mod app;
pub use app::MonitorInfo;

pub mod desktop;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{list_monitors, run};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{list_monitors, run};
