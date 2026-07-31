//! Domain state and native reads for the Seex desktop viewer.

#![forbid(unsafe_code)]

pub mod data;
#[cfg(all(feature = "desktop", target_os = "macos"))]
pub mod desktop;
pub mod domain;
#[cfg(feature = "test-support")]
pub mod performance;
pub mod workbench;

pub use data::SourceError;
