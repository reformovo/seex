//! Analysis views, coordinated panel reads, and viewer-owned persistence.

pub mod document;
pub mod panel_reads;
mod view;

pub use view::{AnalysisView, AnalysisViews, DEFAULT_METRIC_ROW_HEIGHT, MetricPanel, ProjectRef};
