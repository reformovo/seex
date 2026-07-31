//! Test-support resource accounting for the Viewer pipeline.

/// Resource ownership derived from one immutable curve snapshot.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CurveResourceSnapshot {
    pub requested_budget: u64,
    pub source_points: u64,
    pub returned_points: u64,
    pub snapshot_points: u64,
    pub snapshot_bytes: u64,
    pub projected_points: u64,
    pub compacted_points: u64,
    pub path_vertices: u64,
    pub path_entries: u64,
}

/// Curve resource ownership qualified by its workbench request identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PanelResourceSnapshot {
    pub view_id: String,
    pub panel_id: String,
    pub generation: u64,
    pub read_kind: String,
    pub curves: CurveResourceSnapshot,
    pub concurrent_reads: u64,
    pub peak_concurrent_reads: u64,
    pub superseded_reads: u64,
    pub stale_reads: u64,
    pub stale_retained_snapshots: u64,
}

/// Scheduling counters retained only by test-support workers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReadSchedulingSnapshot {
    pub concurrent_reads: u64,
    pub peak_concurrent_reads: u64,
    pub superseded_reads: u64,
    pub stale_reads: u64,
    pub stale_retained_snapshots: u64,
}
