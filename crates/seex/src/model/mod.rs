//! Shared product and query types for Seex.

#![expect(
    dead_code,
    reason = "private query contracts remain exercised by crate-local parity tests"
)]

pub mod alignment;
pub mod comparison;
pub mod metric;
pub mod run;
pub mod types;
