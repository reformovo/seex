use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::model::comparison::{
    ComparisonOutcome, ComparisonPreference, ComparisonReport, ComparisonResult,
    EvidenceCompleteness, EvidenceReason, MetricComparisonResult, ObjectiveDirection,
    ObjectiveEvidence, ObjectiveMetric, RankingEntry, RankingResult,
};
use crate::model::metric::MetricKey;
use crate::model::run::RunStatus;

#[derive(Clone)]
#[pyclass(name = "ObjectiveMetric", module = "seex._seex", skip_from_py_object)]
pub struct PyObjectiveMetric {
    #[pyo3(get)]
    metric_key: String,
    #[pyo3(get)]
    direction: String,
}

impl From<ObjectiveMetric> for PyObjectiveMetric {
    fn from(objective: ObjectiveMetric) -> Self {
        Self {
            metric_key: objective.metric_key.as_str().to_owned(),
            direction: direction_value(objective.direction).to_owned(),
        }
    }
}

#[derive(Clone)]
#[pyclass(name = "ObjectiveEvidence", module = "seex._seex", skip_from_py_object)]
pub struct PyObjectiveEvidence {
    #[pyo3(get)]
    run_id: String,
    #[pyo3(get)]
    run_status: String,
    #[pyo3(get)]
    last_step: Option<i64>,
    #[pyo3(get)]
    last_value_f64: Option<f64>,
    #[pyo3(get)]
    completeness: String,
    #[pyo3(get)]
    reasons: Vec<String>,
}

impl From<ObjectiveEvidence> for PyObjectiveEvidence {
    fn from(evidence: ObjectiveEvidence) -> Self {
        Self {
            run_id: evidence.run_id.as_str().to_owned(),
            run_status: status_value(evidence.run_status).to_owned(),
            last_step: evidence.last_step.map(|step| step.value()),
            last_value_f64: evidence.last_value_f64,
            completeness: completeness_value(evidence.completeness).to_owned(),
            reasons: evidence
                .reasons
                .into_iter()
                .map(|reason| reason_value(reason).to_owned())
                .collect(),
        }
    }
}

#[derive(Clone)]
#[pyclass(name = "ComparisonResult", module = "seex._seex", skip_from_py_object)]
pub struct PyComparisonResult {
    objective: PyObjectiveMetric,
    candidate: PyObjectiveEvidence,
    reference: PyObjectiveEvidence,
    #[pyo3(get)]
    completeness: String,
    #[pyo3(get)]
    raw_delta: Option<f64>,
    #[pyo3(get)]
    relative_delta: Option<f64>,
    #[pyo3(get)]
    normalized_improvement: Option<f64>,
    #[pyo3(get)]
    outcome: Option<String>,
    #[pyo3(get)]
    preference: String,
}

#[pymethods]
impl PyComparisonResult {
    #[getter]
    fn objective(&self) -> PyObjectiveMetric {
        self.objective.clone()
    }

    #[getter]
    fn candidate(&self) -> PyObjectiveEvidence {
        self.candidate.clone()
    }

    #[getter]
    fn reference(&self) -> PyObjectiveEvidence {
        self.reference.clone()
    }
}

impl From<ComparisonResult> for PyComparisonResult {
    fn from(result: ComparisonResult) -> Self {
        Self {
            objective: result.objective.into(),
            candidate: result.candidate.into(),
            reference: result.reference.into(),
            completeness: completeness_value(result.completeness).to_owned(),
            raw_delta: result.raw_delta,
            relative_delta: result.relative_delta,
            normalized_improvement: result.normalized_improvement,
            outcome: result.outcome.map(outcome_value).map(str::to_owned),
            preference: preference_value(result.preference).to_owned(),
        }
    }
}

#[derive(Clone)]
#[pyclass(
    name = "_MetricComparisonResult",
    module = "seex._seex",
    skip_from_py_object
)]
pub struct PyMetricComparisonResult {
    #[pyo3(get)]
    metric_key: String,
    candidate: PyObjectiveEvidence,
    reference: PyObjectiveEvidence,
    #[pyo3(get)]
    completeness: String,
    #[pyo3(get)]
    raw_delta: Option<f64>,
    #[pyo3(get)]
    relative_delta: Option<f64>,
}

#[pymethods]
impl PyMetricComparisonResult {
    #[getter]
    fn candidate(&self) -> PyObjectiveEvidence {
        self.candidate.clone()
    }

    #[getter]
    fn reference(&self) -> PyObjectiveEvidence {
        self.reference.clone()
    }
}

impl From<MetricComparisonResult> for PyMetricComparisonResult {
    fn from(result: MetricComparisonResult) -> Self {
        Self {
            metric_key: result.metric_key.as_str().to_owned(),
            candidate: result.candidate.into(),
            reference: result.reference.into(),
            completeness: completeness_value(result.completeness).to_owned(),
            raw_delta: result.raw_delta,
            relative_delta: result.relative_delta,
        }
    }
}

#[pyclass(name = "_ComparisonReport", module = "seex._seex")]
pub struct PyComparisonReport {
    primary: PyComparisonResult,
    secondary: Vec<PyMetricComparisonResult>,
}

#[pymethods]
impl PyComparisonReport {
    #[getter]
    fn primary(&self) -> PyComparisonResult {
        self.primary.clone()
    }

    #[getter]
    fn secondary(&self) -> Vec<PyMetricComparisonResult> {
        self.secondary.clone()
    }
}

impl From<ComparisonReport> for PyComparisonReport {
    fn from(report: ComparisonReport) -> Self {
        Self {
            primary: report.primary.into(),
            secondary: report
                .secondary
                .into_iter()
                .map(PyMetricComparisonResult::from)
                .collect(),
        }
    }
}

#[derive(Clone)]
#[pyclass(name = "RankingEntry", module = "seex._seex", skip_from_py_object)]
pub struct PyRankingEntry {
    evidence: PyObjectiveEvidence,
    #[pyo3(get)]
    rank: Option<u64>,
}

#[pymethods]
impl PyRankingEntry {
    #[getter]
    fn evidence(&self) -> PyObjectiveEvidence {
        self.evidence.clone()
    }
}

impl From<RankingEntry> for PyRankingEntry {
    fn from(entry: RankingEntry) -> Self {
        Self {
            evidence: entry.evidence.into(),
            rank: entry.rank,
        }
    }
}

#[pyclass(name = "RankingResult", module = "seex._seex")]
pub struct PyRankingResult {
    objective: PyObjectiveMetric,
    entries: Vec<PyRankingEntry>,
}

#[pymethods]
impl PyRankingResult {
    #[getter]
    fn objective(&self) -> PyObjectiveMetric {
        self.objective.clone()
    }

    #[getter]
    fn entries(&self) -> Vec<PyRankingEntry> {
        self.entries.clone()
    }
}

impl From<RankingResult> for PyRankingResult {
    fn from(result: RankingResult) -> Self {
        Self {
            objective: result.objective.into(),
            entries: result
                .entries
                .into_iter()
                .map(PyRankingEntry::from)
                .collect(),
        }
    }
}

pub fn objective(metric_key: &str, direction: &str) -> PyResult<ObjectiveMetric> {
    let direction = match direction {
        "minimize" => ObjectiveDirection::Minimize,
        "maximize" => ObjectiveDirection::Maximize,
        other => {
            return Err(PyValueError::new_err(format!(
                "direction must be 'minimize' or 'maximize', got {other:?}"
            )));
        }
    };
    Ok(ObjectiveMetric {
        metric_key: MetricKey::from_string(metric_key),
        direction,
    })
}

fn direction_value(direction: ObjectiveDirection) -> &'static str {
    match direction {
        ObjectiveDirection::Minimize => "minimize",
        ObjectiveDirection::Maximize => "maximize",
    }
}

fn status_value(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "running",
        RunStatus::Finished => "finished",
        RunStatus::Failed => "failed",
    }
}

fn completeness_value(value: EvidenceCompleteness) -> &'static str {
    match value {
        EvidenceCompleteness::Complete => "complete",
        EvidenceCompleteness::Partial => "partial",
        EvidenceCompleteness::Unavailable => "unavailable",
        EvidenceCompleteness::Invalid => "invalid",
    }
}

fn reason_value(reason: EvidenceReason) -> &'static str {
    match reason {
        EvidenceReason::MissingMetric => "missing_metric",
        EvidenceReason::MissingRunStart => "missing_run_start",
        EvidenceReason::NegativeAxis => "negative_axis",
        EvidenceReason::DecreasingAxis => "decreasing_axis",
        EvidenceReason::NonFiniteValue => "non_finite_value",
        EvidenceReason::DiagnosticsUnavailable => "diagnostics_unavailable",
        EvidenceReason::RunRunning => "run_running",
        EvidenceReason::RunFailed => "run_failed",
    }
}

fn outcome_value(outcome: ComparisonOutcome) -> &'static str {
    match outcome {
        ComparisonOutcome::Improved => "improved",
        ComparisonOutcome::Regressed => "regressed",
        ComparisonOutcome::Equal => "equal",
    }
}

fn preference_value(preference: ComparisonPreference) -> &'static str {
    match preference {
        ComparisonPreference::Candidate => "candidate",
        ComparisonPreference::Reference => "reference",
        ComparisonPreference::NoPreference => "no_preference",
        ComparisonPreference::Inconclusive => "inconclusive",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_unavailable_has_a_stable_python_value() {
        assert_eq!(
            reason_value(EvidenceReason::DiagnosticsUnavailable),
            "diagnostics_unavailable"
        );
    }
}
