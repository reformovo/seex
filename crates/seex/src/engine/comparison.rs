use crate::engine::EngineError;
use crate::engine::client::NativeClient;
use crate::model::comparison::{
    ComparisonOutcome, ComparisonPreference, ComparisonReport, ComparisonResult,
    EvidenceCompleteness, MetricComparisonResult, ObjectiveEvidence, ObjectiveMetric,
};
use crate::model::metric::MetricKey;
use crate::model::run::RunId;

struct SecondaryEvidenceSet {
    metric_key: MetricKey,
    reference: ObjectiveEvidence,
    candidates: Vec<ObjectiveEvidence>,
}

impl NativeClient {
    /// Builds primary comparison reports in candidate request order.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::DuplicateRunIdentity`] for repeated candidate
    /// IDs or when the reference is also a candidate. Unknown Run IDs and
    /// storage failures are returned from the shared evidence query.
    pub fn comparison_reports(
        &self,
        candidate_run_ids: &[RunId],
        reference_run_id: &RunId,
        objective: &ObjectiveMetric,
        secondary_metric_keys: &[MetricKey],
    ) -> Result<Vec<ComparisonReport>, EngineError> {
        let mut request_run_ids = Vec::with_capacity(candidate_run_ids.len() + 1);
        request_run_ids.push(reference_run_id.clone());
        request_run_ids.extend_from_slice(candidate_run_ids);
        reject_duplicate_run_ids(&request_run_ids)?;

        let mut evidence = self
            .ranking_evidence(&request_run_ids, objective)?
            .into_iter();
        let Some((_, reference)) = evidence.next() else {
            return Ok(Vec::new());
        };
        let secondary = secondary_metric_keys
            .iter()
            .map(|metric_key| {
                let metric = ObjectiveMetric {
                    metric_key: metric_key.clone(),
                    direction: objective.direction,
                };
                let mut evidence = self
                    .ranking_evidence(&request_run_ids, &metric)?
                    .into_iter();
                let Some((_, reference)) = evidence.next() else {
                    return Ok(None);
                };
                Ok(Some(SecondaryEvidenceSet {
                    metric_key: metric_key.clone(),
                    reference,
                    candidates: evidence.map(|(_, candidate)| candidate).collect(),
                }))
            })
            .collect::<Result<Vec<_>, EngineError>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        Ok(build_reports(
            objective,
            reference,
            evidence.map(|(_, candidate)| candidate).collect(),
            &secondary,
        ))
    }

    /// Compares two Runs using their last effective objective values.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::DuplicateRunIdentity`] when both request roles
    /// use the same Run or a lookup/storage error when evidence cannot be read.
    pub fn compare_runs(
        &self,
        candidate_run_id: &RunId,
        reference_run_id: &RunId,
        objective: &ObjectiveMetric,
    ) -> Result<ComparisonResult, EngineError> {
        if candidate_run_id == reference_run_id {
            return Err(EngineError::DuplicateRunIdentity {
                run_id: candidate_run_id.as_str().to_owned(),
            });
        }
        let candidate = self.objective_evidence(candidate_run_id, objective)?;
        let reference = self.objective_evidence(reference_run_id, objective)?;
        Ok(compare_evidence(objective, candidate, reference))
    }
}

fn build_reports(
    objective: &ObjectiveMetric,
    reference: ObjectiveEvidence,
    candidates: Vec<ObjectiveEvidence>,
    secondary: &[SecondaryEvidenceSet],
) -> Vec<ComparisonReport> {
    candidates
        .into_iter()
        .enumerate()
        .map(|(index, candidate)| ComparisonReport {
            primary: compare_evidence(objective, candidate, reference.clone()),
            secondary: secondary
                .iter()
                .map(|set| {
                    compare_metric_evidence(
                        &set.metric_key,
                        set.candidates[index].clone(),
                        set.reference.clone(),
                    )
                })
                .collect(),
        })
        .collect()
}

fn compare_metric_evidence(
    metric_key: &MetricKey,
    candidate: ObjectiveEvidence,
    reference: ObjectiveEvidence,
) -> MetricComparisonResult {
    let completeness = candidate.completeness.max(reference.completeness);
    let (raw_delta, relative_delta) = candidate
        .usable_value()
        .zip(reference.usable_value())
        .map_or((None, None), |(candidate, reference)| {
            let raw = candidate - reference;
            (
                Some(raw),
                (reference != 0.0).then_some(raw / reference.abs()),
            )
        });
    MetricComparisonResult {
        metric_key: metric_key.clone(),
        candidate,
        reference,
        completeness,
        raw_delta,
        relative_delta,
    }
}

fn reject_duplicate_run_ids(run_ids: &[RunId]) -> Result<(), EngineError> {
    let mut seen = std::collections::HashSet::with_capacity(run_ids.len());
    for run_id in run_ids {
        if !seen.insert(run_id) {
            return Err(EngineError::DuplicateRunIdentity {
                run_id: run_id.as_str().to_owned(),
            });
        }
    }
    Ok(())
}

fn compare_evidence(
    objective: &ObjectiveMetric,
    candidate: ObjectiveEvidence,
    reference: ObjectiveEvidence,
) -> ComparisonResult {
    let completeness = candidate.completeness.max(reference.completeness);
    let numeric =
        candidate
            .usable_value()
            .zip(reference.usable_value())
            .map(|(candidate, reference)| {
                let raw_delta = candidate - reference;
                let relative_delta = (reference != 0.0).then_some(raw_delta / reference.abs());
                let normalized = objective.direction.normalize(raw_delta);
                let outcome = ComparisonOutcome::from_normalized_improvement(normalized);
                (raw_delta, relative_delta, normalized, outcome)
            });
    let (raw_delta, relative_delta, normalized_improvement, outcome) = match numeric {
        Some((raw, relative, normalized, outcome)) => {
            (Some(raw), relative, Some(normalized), Some(outcome))
        }
        None => (None, None, None, None),
    };
    let preference = match (completeness, outcome) {
        (EvidenceCompleteness::Complete, Some(ComparisonOutcome::Improved)) => {
            ComparisonPreference::Candidate
        }
        (EvidenceCompleteness::Complete, Some(ComparisonOutcome::Regressed)) => {
            ComparisonPreference::Reference
        }
        (EvidenceCompleteness::Complete, Some(ComparisonOutcome::Equal)) => {
            ComparisonPreference::NoPreference
        }
        _ => ComparisonPreference::Inconclusive,
    };
    ComparisonResult {
        objective: objective.clone(),
        candidate,
        reference,
        completeness,
        raw_delta,
        relative_delta,
        normalized_improvement,
        outcome,
        preference,
    }
}

#[cfg(test)]
mod tests {
    use crate::model::comparison::{EvidenceReason, ObjectiveDirection};
    use crate::model::metric::{MetricKey, Step};
    use crate::model::run::RunStatus;

    use super::*;

    fn evidence(run_id: &str, value: f64, completeness: EvidenceCompleteness) -> ObjectiveEvidence {
        ObjectiveEvidence {
            run_id: RunId::from_string(run_id),
            run_status: RunStatus::Finished,
            last_step: Some(Step::new(4)),
            last_value_f64: Some(value),
            completeness,
            reasons: Vec::new(),
        }
    }

    fn objective(direction: ObjectiveDirection) -> ObjectiveMetric {
        ObjectiveMetric {
            metric_key: MetricKey::from_string("loss"),
            direction,
        }
    }

    #[test]
    fn minimize_comparison_normalizes_delta_and_prefers_candidate() {
        let result = compare_evidence(
            &objective(ObjectiveDirection::Minimize),
            evidence("candidate", 2.0, EvidenceCompleteness::Complete),
            evidence("reference", 4.0, EvidenceCompleteness::Complete),
        );

        assert_eq!(result.raw_delta, Some(-2.0));
        assert_eq!(result.relative_delta, Some(-0.5));
        assert_eq!(result.normalized_improvement, Some(2.0));
        assert_eq!(result.outcome, Some(ComparisonOutcome::Improved));
        assert_eq!(result.preference, ComparisonPreference::Candidate);
    }

    #[test]
    fn zero_reference_omits_relative_delta_but_keeps_outcome() {
        let result = compare_evidence(
            &objective(ObjectiveDirection::Maximize),
            evidence("candidate", 1.0, EvidenceCompleteness::Complete),
            evidence("reference", 0.0, EvidenceCompleteness::Complete),
        );

        assert_eq!(result.relative_delta, None);
        assert_eq!(result.outcome, Some(ComparisonOutcome::Improved));
    }

    #[test]
    fn partial_numeric_evidence_keeps_outcome_but_is_inconclusive() {
        let mut candidate = evidence("candidate", 1.0, EvidenceCompleteness::Partial);
        candidate.reasons.push(EvidenceReason::RunRunning);
        let result = compare_evidence(
            &objective(ObjectiveDirection::Minimize),
            candidate,
            evidence("reference", 2.0, EvidenceCompleteness::Complete),
        );

        assert_eq!(result.outcome, Some(ComparisonOutcome::Improved));
        assert_eq!(result.preference, ComparisonPreference::Inconclusive);
    }

    #[test]
    fn reports_preserve_candidate_order() {
        let reports = build_reports(
            &objective(ObjectiveDirection::Minimize),
            evidence("reference", 4.0, EvidenceCompleteness::Complete),
            vec![
                evidence("candidate-b", 2.0, EvidenceCompleteness::Complete),
                evidence("candidate-a", 3.0, EvidenceCompleteness::Complete),
            ],
            &[],
        );

        assert_eq!(
            reports
                .iter()
                .map(|report| report.primary.candidate.run_id.as_str())
                .collect::<Vec<_>>(),
            vec!["candidate-b", "candidate-a"]
        );
        assert!(reports.iter().all(|report| report.secondary.is_empty()));
    }

    #[test]
    fn secondary_metrics_preserve_order_without_affecting_preference() {
        let secondary = [SecondaryEvidenceSet {
            metric_key: MetricKey::from_string("memory"),
            reference: evidence("reference", 4.0, EvidenceCompleteness::Complete),
            candidates: vec![evidence("candidate", 5.0, EvidenceCompleteness::Partial)],
        }];
        let reports = build_reports(
            &objective(ObjectiveDirection::Minimize),
            evidence("reference", 2.0, EvidenceCompleteness::Complete),
            vec![evidence("candidate", 1.0, EvidenceCompleteness::Complete)],
            &secondary,
        );

        assert_eq!(
            reports[0].primary.preference,
            ComparisonPreference::Candidate
        );
        assert_eq!(reports[0].secondary[0].metric_key.as_str(), "memory");
        assert_eq!(
            reports[0].secondary[0].completeness,
            EvidenceCompleteness::Partial
        );
        assert_eq!(reports[0].secondary[0].raw_delta, Some(1.0));
    }

    #[test]
    fn report_request_rejects_reference_as_candidate() {
        let run_id = RunId::from_string("same");
        let error = reject_duplicate_run_ids(&[run_id.clone(), run_id]).unwrap_err();

        assert!(
            matches!(error, EngineError::DuplicateRunIdentity { ref run_id } if run_id == "same"),
            "expected duplicate Run error, got {error:?}",
        );
    }
}
