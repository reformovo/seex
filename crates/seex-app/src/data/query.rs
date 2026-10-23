use crate::data::source::ReadSession;
use crate::domain::{DataSourceId, MAX_SELECTED_RUNS, RunRef};
use seex::Run;
use seex::{AlignmentQueryError, AlignmentViewport};
use seex::{EvidenceCompleteness, EvidenceReason, ObjectiveEvidence};
use seex::{
    MAX_CHART_POINTS, MetricAxis, MetricCoordinate, MetricQuery, MetricQueryError, MetricRange,
    RelativeTime,
};
use seex::{MetricAggregate, MetricKey};
use seex_plot::{DataPoint, Series, SeriesId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CurveAxis {
    Step,
    ElapsedTime,
}

/// Shared series selection for overview and detail queries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CurveSelection {
    pub source_id: DataSourceId,
    pub runs: Vec<RunRef>,
    pub metric_key: MetricKey,
    pub axis: CurveAxis,
}

/// Full non-negative-axis overview query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OverviewRequest {
    pub selection: CurveSelection,
    pub logical_width: u32,
}

/// Closed-viewport detail query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetailRequest {
    pub selection: CurveSelection,
    pub viewport: AlignmentViewport,
    pub logical_width: u32,
}

/// Whole-series product summaries and objective evidence for the inspector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectorRequest {
    pub source_id: DataSourceId,
    pub runs: Vec<RunRef>,
    pub metric_key: MetricKey,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InspectorRunSnapshot {
    pub run_ref: RunRef,
    pub run: Run,
    pub summary: Option<MetricAggregate>,
    pub evidence: ObjectiveEvidence,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InspectorSnapshot {
    pub runs: Vec<InspectorRunSnapshot>,
}

/// One Run's evidence and optional drawable chart series.
#[derive(Clone, Debug, PartialEq)]
pub struct CurveSeriesSnapshot {
    pub run_ref: RunRef,
    pub run: Run,
    pub completeness: EvidenceCompleteness,
    pub reasons: Vec<EvidenceReason>,
    pub source_row_count: u64,
    pub returned_point_count: u64,
    pub chart_series: Option<Series>,
}

impl CurveSeriesSnapshot {
    pub fn downsampled(&self) -> bool {
        self.source_row_count > self.returned_point_count
    }
}

/// Immutable reduced curves returned for one requested viewport.
#[derive(Clone, Debug, PartialEq)]
pub struct CurveSnapshot {
    pub viewport: AlignmentViewport,
    pub point_budget: u32,
    /// Data-derived range, expanded to one rendered axis unit for one coordinate.
    pub real_range: Option<AlignmentViewport>,
    pub series: Vec<CurveSeriesSnapshot>,
}

#[cfg(feature = "test-support")]
impl CurveSnapshot {
    /// Returns shallow point ownership without instrumenting the production path.
    pub fn resource_snapshot(&self) -> crate::performance::CurveResourceSnapshot {
        let returned_points = self
            .series
            .iter()
            .map(|curve| curve.returned_point_count)
            .sum::<u64>();
        let chart_points = self
            .series
            .iter()
            .filter_map(|curve| curve.chart_series.as_ref())
            .map(|series| series.points().len() as u64)
            .sum::<u64>();
        crate::performance::CurveResourceSnapshot {
            requested_budget: u64::from(self.point_budget),
            source_points: self.series.iter().map(|curve| curve.source_row_count).sum(),
            returned_points,
            snapshot_points: chart_points,
            snapshot_bytes: chart_points * std::mem::size_of::<DataPoint>() as u64,
            ..crate::performance::CurveResourceSnapshot::default()
        }
    }
}

/// Failures while planning or executing a viewer curve query.
#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    #[error(transparent)]
    Alignment(#[from] AlignmentQueryError),
    #[error(transparent)]
    Chart(#[from] seex_plot::PlotError),
    #[error(transparent)]
    Sdk(#[from] seex::Error),
    #[error(transparent)]
    MetricQuery(#[from] MetricQueryError),
    #[error("a chart can contain at most {MAX_SELECTED_RUNS} curves; got {count}")]
    TooManyCurves { count: usize },
    #[error("ElapsedTime chart queries are disabled in P0")]
    ElapsedTimeUnsupported,
    #[error("Reader returned a coordinate on the wrong axis")]
    ReaderAxisMismatch,
}

impl ReadSession {
    /// Queries the full non-negative axis at one point per physical pixel.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError`] when the request cannot be planned or executed.
    pub fn query_overview(&self, request: &OverviewRequest) -> Result<CurveSnapshot, QueryError> {
        self.query_overview_until(request, &mut || false, &mut |_| true, &mut |_| {})
            .map(|snapshot| snapshot.expect("non-cancellable query should return a snapshot"))
    }

    pub(crate) fn query_overview_until(
        &self,
        request: &OverviewRequest,
        is_superseded: &mut dyn FnMut() -> bool,
        register: &mut dyn FnMut(seex::ReaderInterrupt) -> bool,
        trace: &mut dyn FnMut(&'static str),
    ) -> Result<Option<CurveSnapshot>, QueryError> {
        let viewport = match request.selection.axis {
            CurveAxis::Step => AlignmentViewport::new(0, i64::MAX)?,
            CurveAxis::ElapsedTime => AlignmentViewport::new(i64::MIN, i64::MAX)?,
        };
        query_curves(
            self,
            &request.selection,
            viewport,
            overview_budget(request.logical_width),
            true,
            is_superseded,
            register,
            trace,
        )
    }

    /// Queries a closed detail viewport at two points per physical pixel.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError`] when the request cannot be planned or executed.
    pub fn query_detail(&self, request: &DetailRequest) -> Result<CurveSnapshot, QueryError> {
        self.query_detail_until(request, &mut || false, &mut |_| true, &mut |_| {})
            .map(|snapshot| snapshot.expect("non-cancellable query should return a snapshot"))
    }

    pub(crate) fn query_detail_until(
        &self,
        request: &DetailRequest,
        is_superseded: &mut dyn FnMut() -> bool,
        register: &mut dyn FnMut(seex::ReaderInterrupt) -> bool,
        trace: &mut dyn FnMut(&'static str),
    ) -> Result<Option<CurveSnapshot>, QueryError> {
        query_curves(
            self,
            &request.selection,
            request.viewport,
            detail_budget(request.logical_width),
            false,
            is_superseded,
            register,
            trace,
        )
    }

    /// Queries whole-series summaries and objective evidence for the inspector.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError`] when native storage cannot execute the query.
    pub fn query_inspector(
        &self,
        request: &InspectorRequest,
    ) -> Result<InspectorSnapshot, QueryError> {
        self.query_inspector_until(request, &mut || false)
            .map(|snapshot| snapshot.expect("non-cancellable query should return a snapshot"))
    }

    pub(crate) fn query_inspector_until(
        &self,
        request: &InspectorRequest,
        is_superseded: &mut dyn FnMut() -> bool,
    ) -> Result<Option<InspectorSnapshot>, QueryError> {
        if is_superseded() {
            return Ok(None);
        }
        let run_ids = request
            .runs
            .iter()
            .map(|run| run.run_id.clone())
            .collect::<Vec<_>>();
        let Some(evidence) = self.reader().inspect_metric_for_desktop(
            &run_ids,
            &request.metric_key,
            is_superseded,
        )?
        else {
            return Ok(None);
        };
        let snapshots = evidence
            .into_iter()
            .filter_map(|item| {
                let run = item.run;
                let run_ref = request.runs.iter().find(|selected| {
                    selected.source_id == request.source_id
                        && selected.project_id == run.project_id
                        && selected.run_id == run.run_id
                })?;
                Some(InspectorRunSnapshot {
                    run_ref: run_ref.clone(),
                    run,
                    summary: item.summary,
                    evidence: item.evidence,
                })
            })
            .collect::<Vec<_>>();
        Ok(Some(InspectorSnapshot { runs: snapshots }))
    }
}

pub(crate) fn overview_budget(logical_width: u32) -> u32 {
    let _ = logical_width;
    MAX_CHART_POINTS as u32
}

pub(crate) fn detail_budget(logical_width: u32) -> u32 {
    let _ = logical_width;
    MAX_CHART_POINTS as u32
}

struct PanelStageProfile {
    request: u64,
    started: std::time::Instant,
    attempt: usize,
    next: usize,
    current: Option<usize>,
}
impl PanelStageProfile {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            request: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            started: std::time::Instant::now(),
            attempt: 0,
            next: 0,
            current: None,
        }
    }
    fn emit(&mut self, phase: &'static str, runs: &[seex::RunId]) {
        if phase == "fresh_open_start" {
            self.attempt += 1;
            self.next = 0;
            self.current = None;
        }
        if phase == "curve_start" {
            assert!(
                self.next < runs.len(),
                "trace curve order exceeds requested Runs"
            );
            self.current = Some(self.next);
            self.next += 1;
        }
        let run = self.current.map(|i| runs[i].as_str());
        eprintln!(
            "SEEX_PANEL_STAGE request={} attempt={} run={run:?} phase={phase} elapsed_ns={}",
            self.request,
            self.attempt,
            self.started.elapsed().as_nanos()
        );
        if matches!(phase, "curve_finish" | "curve_error") {
            self.current = None;
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the read request phases are threaded explicitly to keep call sites visible"
)]
fn query_curves(
    session: &ReadSession,
    selection: &CurveSelection,
    viewport: AlignmentViewport,
    point_budget: u32,
    overview: bool,
    is_superseded: &mut dyn FnMut() -> bool,
    register: &mut dyn FnMut(seex::ReaderInterrupt) -> bool,
    trace: &mut dyn FnMut(&'static str),
) -> Result<Option<CurveSnapshot>, QueryError> {
    if selection.runs.len() > crate::domain::MAX_SELECTED_RUNS {
        return Err(QueryError::TooManyCurves {
            count: selection.runs.len(),
        });
    }
    if selection.axis == CurveAxis::ElapsedTime {
        return Err(QueryError::ElapsedTimeUnsupported);
    }
    let run_ids = selection
        .runs
        .iter()
        .filter(|run| run.source_id == selection.source_id)
        .map(|run| run.run_id.clone())
        .collect::<Vec<_>>();
    let runs = session.reader().runs_for_desktop(&run_ids)?;
    let runs = runs
        .into_iter()
        .filter(|run| {
            selection.runs.iter().any(|selected| {
                selected.source_id == selection.source_id
                    && selected.project_id == run.project_id
                    && selected.run_id == run.run_id
            })
        })
        .collect::<Vec<_>>();
    let query = MetricQuery::new(
        desktop_metric_range(selection.axis, viewport, overview),
        point_budget as usize,
    )?;
    let requested = runs
        .iter()
        .map(|run| run.run_id.clone())
        .collect::<Vec<_>>();
    let mut profile = std::env::var_os("SEEX_PANEL_STAGE_TIMING").map(|_| PanelStageProfile::new());
    if let Some(p) = &mut profile {
        p.emit("reader_start", &requested);
    }
    let result = session.reader().query_metrics_for_desktop_with_interrupt(
        &requested,
        &selection.metric_key,
        &query,
        is_superseded,
        register,
        &mut |phase| {
            trace(phase);
            if let Some(p) = &mut profile {
                p.emit(phase, &requested);
            }
        },
    );
    if let Some(p) = &mut profile {
        p.emit(
            match &result {
                Ok(Some(_)) => "reader_finish",
                Ok(None) => "reader_cancelled",
                Err(_) => "reader_error",
            },
            &requested,
        );
    }
    let Some(evidence) = result? else {
        return Ok(None);
    };
    let mut real_bounds: Option<(i64, i64)> = None;
    let mut series = Vec::with_capacity(selection.runs.len());
    for (run, evidence) in runs.into_iter().zip(evidence) {
        if is_superseded() {
            return Ok(None);
        }
        let Some(run_ref) = selection.runs.iter().find(|selected| {
            selected.source_id == selection.source_id
                && selected.project_id == run.project_id
                && selected.run_id == run.run_id
        }) else {
            continue;
        };
        let drawable = matches!(
            evidence.completeness(),
            EvidenceCompleteness::Complete | EvidenceCompleteness::Partial
        );
        let returned_point_count = evidence.samples().len() as u64;
        let source_row_count = evidence.source_count();
        let completeness = evidence.completeness();
        let reasons = evidence.reasons().to_vec();
        let chart_points = if drawable {
            let mut points = Vec::with_capacity(evidence.samples().len());
            for sample in evidence.samples() {
                let axis_value = desktop_axis_value(selection.axis, sample.coordinate)?;
                if axis_value >= viewport.start() && axis_value <= viewport.end() {
                    real_bounds = Some(match real_bounds {
                        Some((start, end)) => (start.min(axis_value), end.max(axis_value)),
                        None => (axis_value, axis_value),
                    });
                }
                points.push(DataPoint::new(axis_value as f64, sample.point.value_f64));
            }
            Some(points)
        } else {
            None
        };
        let chart_series = chart_points
            .map(|points| Series::new(SeriesId::new(run_ref.cache_key())?, points))
            .transpose()?;
        series.push(CurveSeriesSnapshot {
            run_ref: run_ref.clone(),
            run,
            completeness,
            reasons,
            source_row_count,
            returned_point_count,
            chart_series,
        });
    }
    let real_range = real_bounds.map(brushable_range).transpose()?;
    if is_superseded() {
        return Ok(None);
    }
    Ok(Some(CurveSnapshot {
        viewport,
        point_budget,
        real_range,
        series,
    }))
}

fn desktop_metric_range(
    axis: CurveAxis,
    viewport: AlignmentViewport,
    overview: bool,
) -> MetricRange {
    let end = viewport.end().saturating_add(1);
    match axis {
        // Overview covers the whole Step axis. An open-ended range avoids the
        // `i64::MAX - 1` boundary that a closed-to-half-open conversion implies.
        CurveAxis::Step if overview => MetricRange::StepsFrom {
            start: seex::Step::new(0),
        },
        CurveAxis::Step => MetricRange::Steps {
            start: seex::Step::new(viewport.start()),
            end: seex::Step::new(end),
        },
        CurveAxis::ElapsedTime if viewport.start() == i64::MIN && viewport.end() == i64::MAX => {
            MetricRange::All(MetricAxis::RelativeTime)
        }
        CurveAxis::ElapsedTime => MetricRange::RelativeTime {
            start: RelativeTime::from_millis(viewport.start()),
            end: RelativeTime::from_millis(end),
        },
    }
}

fn desktop_axis_value(axis: CurveAxis, coordinate: MetricCoordinate) -> Result<i64, QueryError> {
    match (axis, coordinate) {
        (CurveAxis::Step, MetricCoordinate::Step(value)) => Ok(value.value()),
        (CurveAxis::ElapsedTime, MetricCoordinate::RelativeTime(value)) => Ok(value.as_millis()),
        _ => Err(QueryError::ReaderAxisMismatch),
    }
}

fn brushable_range(
    (mut start, mut end): (i64, i64),
) -> Result<AlignmentViewport, AlignmentQueryError> {
    let rendered_start = start as f64;
    if (end as f64) - rendered_start < 1.0 {
        if let Some(next) = end
            .checked_add(1)
            .filter(|next| (*next as f64) - rendered_start >= 1.0)
        {
            end = next;
        } else {
            let next = rendered_start.next_up() as i64;
            if next > end {
                end = next;
            } else {
                start = (end as f64).next_down() as i64;
            }
        }
    }
    AlignmentViewport::new(start, end)
}

#[cfg(test)]
mod tests {
    use super::{
        CurveAxis, CurveSelection, DataSourceId, InspectorRequest, MetricKey, OverviewRequest,
        ReadSession, RunRef, brushable_range, detail_budget, overview_budget,
    };
    use seex::{Client, LogOptions, ProjectId, RunOptions};
    use seex_plot::{AxisRange, BrushState};

    #[test]
    fn panel_trace_tracks_filtered_run_order_and_resets_on_retry() {
        let runs = [seex::RunId::from_string("b"), seex::RunId::from_string("a")];
        let mut p = super::PanelStageProfile::new();
        p.emit("fresh_open_start", &runs);
        p.emit("curve_start", &runs);
        assert_eq!(p.current.map(|i| runs[i].as_str()), Some("b"));
        p.emit("curve_finish", &runs);
        assert_eq!(p.current, None);
        p.emit("curve_start", &runs);
        assert_eq!(p.current.map(|i| runs[i].as_str()), Some("a"));
        p.emit("curve_error", &runs);
        p.emit("fresh_open_start", &runs);
        assert_eq!(p.attempt, 2);
        p.emit("curve_start", &runs);
        assert_eq!(p.current.map(|i| runs[i].as_str()), Some("b"));
    }

    #[test]
    fn screen_budgets_clamp_density_and_overflow() {
        assert_eq!(
            [
                overview_budget(1),
                overview_budget(900),
                overview_budget(9_000),
                detail_budget(1),
                detail_budget(2_500),
                detail_budget(u32::MAX)
            ],
            [1_024; 6]
        );
    }

    #[test]
    fn data_ranges_have_a_brushable_rendered_span() {
        let ordinary = brushable_range((7, 7)).expect("ordinary range should be valid");
        let maximum = brushable_range((i64::MAX, i64::MAX))
            .expect("maximum coordinate range should be valid");
        let rounded_maximum = brushable_range((i64::MAX - 1, i64::MAX))
            .expect("rounded maximum range should be valid");

        assert_eq!((ordinary.start(), ordinary.end()), (7, 8));
        for range in [ordinary, maximum, rounded_maximum] {
            let axis = AxisRange::new(range.start() as f64, range.end() as f64)
                .expect("real range should initialize a chart range");
            BrushState::new(axis).expect("real range should initialize a brush");
        }
    }

    #[test]
    fn elapsed_time_chart_entry_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let client = Client::builder(root.path()).open()?;
        let run = client.start_run(RunOptions::new("project").id("run").name("candidate"))?;
        run.log_with([("loss", 1.)], LogOptions::new().step(0))?;
        run.log_with([("loss", 0.5)], LogOptions::new().step(1))?;
        run.finish()?;
        client.shutdown()?;
        let session = ReadSession::open_existing(root.path())?;
        let source_id = DataSourceId::new("source").expect("test alias should be valid");
        let selection = CurveSelection {
            source_id: source_id.clone(),
            runs: vec![RunRef::new(
                source_id,
                ProjectId::from_string("project"),
                run.run_id().clone(),
            )],
            metric_key: MetricKey::from_string("loss"),
            axis: CurveAxis::ElapsedTime,
        };

        let error = session.query_overview(&OverviewRequest {
            selection: selection.clone(),
            logical_width: 500,
        });
        assert!(matches!(
            error,
            Err(super::QueryError::ElapsedTimeUnsupported)
        ));
        Ok(())
    }

    #[test]
    fn inspector_stops_when_superseded_between_storage_queries()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let client = Client::builder(root.path()).open()?;
        let run = client.start_run(RunOptions::new("project").id("run").name("candidate"))?;
        client.shutdown()?;
        let session = ReadSession::open_existing(root.path())?;
        let source_id = DataSourceId::new("source").expect("test alias should be valid");
        let request = InspectorRequest {
            source_id: source_id.clone(),
            runs: vec![RunRef::new(
                source_id,
                ProjectId::from_string("project"),
                run.run_id().clone(),
            )],
            metric_key: MetricKey::from_string("loss"),
        };
        let mut checks = 0;

        let snapshot = session.query_inspector_until(&request, &mut || {
            checks += 1;
            checks == 2
        })?;

        assert!(snapshot.is_none());
        assert_eq!(checks, 2);
        Ok(())
    }
}
