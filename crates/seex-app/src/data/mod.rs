use seex_model::metric::MetricKey;
use seex_model::run::{Run, RunId};
use seex_model::types::{Project, ProjectId};

pub mod query;
pub mod registry;
mod source;
pub mod worker;

pub use source::{ReadSession, SourceError};

/// Catalog state requested for one viewer selection.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DiscoveryRequest {
    pub project_allowlist: Option<Vec<ProjectId>>,
    pub project_id: Option<ProjectId>,
    pub selected_run_ids: Vec<RunId>,
    pub metric_runs: Vec<(ProjectId, RunId)>,
}

/// Immutable catalog metadata returned by a native read session.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogSnapshot {
    pub projects: Vec<Project>,
    pub runs: Vec<Run>,
    pub metric_keys: Vec<MetricKey>,
}
