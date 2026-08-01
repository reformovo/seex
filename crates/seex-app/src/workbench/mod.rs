//! Analysis views, coordinated panel reads, and viewer-owned persistence.

pub mod panel_reads;
pub mod toml_document;
mod view;

pub use view::{AnalysisView, AnalysisViews, DEFAULT_METRIC_ROW_HEIGHT, MetricPanel, ProjectRef};
