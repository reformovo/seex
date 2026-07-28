//! Application orchestration for Seex's native client.

#![forbid(unsafe_code)]

pub mod config;
#[cfg(test)]
mod ducklake_test_support;
pub mod engine;
pub mod model;
#[cfg(test)]
mod native_engine_behavior;
