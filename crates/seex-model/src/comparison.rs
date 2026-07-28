use crate::metric::{MetricKey, Step};
use crate::run::{RunId, RunStatus};

/// Request-scoped direction for an objective metric.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectiveDirection {
    Minimize,
    Maximize,
}

impl ObjectiveDirection {
    pub fn normalize(self, raw_delta: f64) -> f64 {
        match self {
            Self::Minimize => -raw_delta,
            Self::Maximize => raw_delta,
        }
    }
}

/// Primary metric and direction used to compare Runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectiveMetric {
    pub metric_key: MetricKey,
    pub direction: ObjectiveDirection,
}

/// Product-level assessment of comparison evidence.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EvidenceCompleteness {
    Complete,
    Partial,
    Unavailable,
    Invalid,
}

/// Structured qualification attached to comparison evidence.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EvidenceReason {
    MissingMetric,
    MissingRunStart,
    NegativeAxis,
    DecreasingAxis,
    NonFiniteValue,
    RunRunning,
    RunFailed,
}

/// Last effective objective value and its evidence qualifications.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectiveEvidence {
    pub run_id: RunId,
    pub run_status: RunStatus,
    pub last_step: Option<Step>,
    pub last_value_f64: Option<f64>,
    pub completeness: EvidenceCompleteness,
    pub reasons: Vec<EvidenceReason>,
}

impl ObjectiveEvidence {
    pub fn usable_value(&self) -> Option<f64> {
        self.last_value_f64.filter(|value| value.is_finite())
    }
}

/// Numeric relationship between a candidate and reference objective.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComparisonOutcome {
    Improved,
    Regressed,
    Equal,
}

impl ComparisonOutcome {
    pub fn from_normalized_improvement(improvement: f64) -> Self {
        if improvement > 0.0 {
            Self::Improved
        } else if improvement < 0.0 {
            Self::Regressed
        } else {
            Self::Equal
        }
    }
}

/// Compute-only advice derived from complete comparison evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComparisonPreference {
    Candidate,
    Reference,
    NoPreference,
    Inconclusive,
}

/// Typed scalar comparison result shared by Core consumers.
#[derive(Clone, Debug, PartialEq)]
pub struct ComparisonResult {
    pub objective: ObjectiveMetric,
    pub candidate: ObjectiveEvidence,
    pub reference: ObjectiveEvidence,
    pub completeness: EvidenceCompleteness,
    pub raw_delta: Option<f64>,
    pub relative_delta: Option<f64>,
    pub normalized_improvement: Option<f64>,
    pub outcome: Option<ComparisonOutcome>,
    pub preference: ComparisonPreference,
}

/// One candidate's primary comparison plus ordered secondary evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct ComparisonReport {
    pub primary: ComparisonResult,
    pub secondary: Vec<MetricComparisonResult>,
}

/// Direction-free scalar comparison for a secondary metric.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricComparisonResult {
    pub metric_key: MetricKey,
    pub candidate: ObjectiveEvidence,
    pub reference: ObjectiveEvidence,
    pub completeness: EvidenceCompleteness,
    pub raw_delta: Option<f64>,
    pub relative_delta: Option<f64>,
}

/// One Run's objective evidence and optional competition rank.
#[derive(Clone, Debug, PartialEq)]
pub struct RankingEntry {
    pub evidence: ObjectiveEvidence,
    pub rank: Option<u64>,
}

/// Complete typed ranking result before presentation pagination.
#[derive(Clone, Debug, PartialEq)]
pub struct RankingResult {
    pub objective: ObjectiveMetric,
    pub entries: Vec<RankingEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completeness_order_matches_aggregate_precedence() {
        assert!(EvidenceCompleteness::Invalid > EvidenceCompleteness::Unavailable);
        assert!(EvidenceCompleteness::Unavailable > EvidenceCompleteness::Partial);
        assert!(EvidenceCompleteness::Partial > EvidenceCompleteness::Complete);
    }

    #[test]
    fn objective_direction_normalizes_improvement() {
        assert_eq!(ObjectiveDirection::Maximize.normalize(2.0), 2.0);
        assert_eq!(ObjectiveDirection::Minimize.normalize(2.0), -2.0);
    }

    #[test]
    fn outcome_uses_exact_sign_without_tolerance() {
        assert_eq!(
            ComparisonOutcome::from_normalized_improvement(f64::MIN_POSITIVE),
            ComparisonOutcome::Improved
        );
        assert_eq!(
            ComparisonOutcome::from_normalized_improvement(0.0),
            ComparisonOutcome::Equal
        );
    }
}
