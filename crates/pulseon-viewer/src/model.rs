use pulseon_model::metric::MetricKey;
use pulseon_model::run::{Run, RunId};
use pulseon_model::types::{Project, ProjectId};

/// Catalog state requested for one viewer selection.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DiscoveryRequest {
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
