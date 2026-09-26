//! Linux desktop integration. Calendar/layout/rendering crates stay portable.
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::run;

#[cfg(target_os = "linux")]
mod theme;

#[cfg(target_os = "linux")]
mod typography;
