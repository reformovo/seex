//! Domain state and native reads for the PulseOn desktop viewer.

#![forbid(unsafe_code)]

pub mod data;
#[cfg(all(feature = "desktop", target_os = "macos"))]
pub mod desktop;
pub mod domain;
pub mod workbench;

pub use data::SourceError;
