use std::cmp::Ordering;
#[cfg(test)]
use std::collections::HashSet;

use chrono::{DateTime, Utc};

#[cfg(test)]
use crate::engine::EngineError;
#[cfg(test)]
use crate::engine::client::NativeClient;
use crate::model::comparison::{
    EvidenceCompleteness, ObjectiveDirection, ObjectiveEvidence, ObjectiveMetric, RankingEntry,
    RankingResult,
};
use crate::model::run::Run;
#[cfg(test)]
use crate::model::run::RunId;

struct RankingCandidate {
    evidence: ObjectiveEvidence,
    created_at: DateTime<Utc>,
    ordinal: usize,
}

#[cfg(test)]
impl NativeClient {
    /// Selects the direction-aware best eligible Run from an explicit pool.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::DuplicateRunIdentity`] for repeated Run IDs or a
    /// lookup/storage error when pool evidence cannot be read.
    #[allow(
        dead_code,
        reason = "retained for crate-local ranking acceptance tests"
    )]
    pub fn best_eligible_run(
        &self,
        run_ids: &[RunId],
        objective: &ObjectiveMetric,
    ) -> Result<Option<RunId>, EngineError> {
        Ok(self
            .rank_runs(run_ids, objective)?
            .entries
            .into_iter()
            .find(|entry| entry.rank == Some(1))
            .map(|entry| entry.evidence.run_id))
    }

    /// Ranks Runs by a request-scoped objective while retaining ineligible Runs.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::DuplicateRunIdentity`] for repeated Run IDs or a
    /// lookup/storage error when request evidence cannot be read.
    pub fn rank_runs(
        &self,
        run_ids: &[RunId],
        objective: &ObjectiveMetric,
    ) -> Result<RankingResult, EngineError> {
        let mut seen = HashSet::with_capacity(run_ids.len());
        for run_id in run_ids {
            if !seen.insert(run_id) {
                return Err(EngineError::DuplicateRunIdentity {
                    run_id: run_id.as_str().to_owned(),
                });
            }
        }
        Ok(rank_run_evidence(
            objective,
            self.ranking_evidence(run_ids, objective)?,
        ))
    }
}

/// Ranks already-loaded Run evidence with the canonical product ordering.
///
/// Complete finite evidence is eligible. Equal objective values receive a
/// competition rank and are ordered by creation time, then Run ID. Ineligible
/// entries retain their input order after eligible entries.
pub fn rank_run_evidence(
    objective: &ObjectiveMetric,
    candidates: Vec<(Run, ObjectiveEvidence)>,
) -> RankingResult {
    rank_candidates(
        objective,
        candidates
            .into_iter()
            .enumerate()
            .map(|(ordinal, (run, evidence))| RankingCandidate {
                evidence,
                created_at: run.created_at,
                ordinal,
            })
            .collect(),
    )
}

fn rank_candidates(
    objective: &ObjectiveMetric,
    mut candidates: Vec<RankingCandidate>,
) -> RankingResult {
    candidates.sort_by(|left, right| compare_candidates(left, right, objective.direction));
    let mut previous_value = None;
    let mut previous_rank = 0;
    let entries = candidates
        .into_iter()
        .enumerate()
        .map(|(index, candidate)| {
            let value = eligible_value(&candidate.evidence);
            let rank = value.map(|value| {
                if previous_value != Some(value) {
                    previous_rank = index as u64 + 1;
                    previous_value = Some(value);
                }
                previous_rank
            });
            RankingEntry {
                evidence: candidate.evidence,
                rank,
            }
        })
        .collect();
    RankingResult {
        objective: objective.clone(),
        entries,
    }
}

fn compare_candidates(
    left: &RankingCandidate,
    right: &RankingCandidate,
    direction: ObjectiveDirection,
) -> Ordering {
    match (
        eligible_value(&left.evidence),
        eligible_value(&right.evidence),
    ) {
        (Some(left_value), Some(right_value)) => {
            let objective_order = if left_value == right_value {
                Ordering::Equal
            } else {
                match direction {
                    ObjectiveDirection::Minimize => left_value.total_cmp(&right_value),
                    ObjectiveDirection::Maximize => right_value.total_cmp(&left_value),
                }
            };
            objective_order
                .then_with(|| left.created_at.cmp(&right.created_at))
                .then_with(|| {
                    left.evidence
                        .run_id
                        .as_str()
                        .cmp(right.evidence.run_id.as_str())
                })
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => left.ordinal.cmp(&right.ordinal),
    }
}

fn eligible_value(evidence: &ObjectiveEvidence) -> Option<f64> {
    (evidence.completeness == EvidenceCompleteness::Complete)
        .then(|| evidence.usable_value())
        .flatten()
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;

    use crate::model::comparison::{EvidenceReason, ObjectiveDirection};
    use crate::model::metric::{MetricKey, Step};
    use crate::model::run::RunStatus;

    use super::*;

    fn candidate(
        run_id: &str,
        value: Option<f64>,
        completeness: EvidenceCompleteness,
        created_at: DateTime<Utc>,
        ordinal: usize,
    ) -> RankingCandidate {
        RankingCandidate {
            evidence: ObjectiveEvidence {
                run_id: RunId::from_string(run_id),
                run_status: RunStatus::Finished,
                last_step: value.map(|_| Step::new(1)),
                last_value_f64: value,
                completeness,
                reasons: (value.is_none())
                    .then_some(EvidenceReason::MissingMetric)
                    .into_iter()
                    .collect(),
            },
            created_at,
            ordinal,
        }
    }

    #[test]
    fn ranking_uses_competition_ranks_and_keeps_ineligible_entries() {
        let now = Utc::now();
        let objective = ObjectiveMetric {
            metric_key: MetricKey::from_string("loss"),
            direction: ObjectiveDirection::Minimize,
        };
        let result = rank_candidates(
            &objective,
            vec![
                candidate(
                    "late-tie",
                    Some(1.0),
                    EvidenceCompleteness::Complete,
                    now,
                    0,
                ),
                candidate("missing", None, EvidenceCompleteness::Unavailable, now, 1),
                candidate("best", Some(0.5), EvidenceCompleteness::Complete, now, 2),
                candidate(
                    "early-tie",
                    Some(1.0),
                    EvidenceCompleteness::Complete,
                    now - TimeDelta::seconds(1),
                    3,
                ),
            ],
        );

        assert_eq!(
            result
                .entries
                .iter()
                .map(|entry| (entry.evidence.run_id.as_str(), entry.rank))
                .collect::<Vec<_>>(),
            vec![
                ("best", Some(1)),
                ("early-tie", Some(2)),
                ("late-tie", Some(2)),
                ("missing", None)
            ]
        );
    }

    #[test]
    fn ranking_maximizes_and_breaks_exact_ties_by_run_id() {
        let now = Utc::now();
        let objective = ObjectiveMetric {
            metric_key: MetricKey::from_string("accuracy"),
            direction: ObjectiveDirection::Maximize,
        };
        let result = rank_candidates(
            &objective,
            vec![
                candidate("zeta", Some(2.0), EvidenceCompleteness::Complete, now, 0),
                candidate("lower", Some(1.0), EvidenceCompleteness::Complete, now, 1),
                candidate("alpha", Some(2.0), EvidenceCompleteness::Complete, now, 2),
            ],
        );

        assert_eq!(
            result
                .entries
                .iter()
                .map(|entry| (entry.evidence.run_id.as_str(), entry.rank))
                .collect::<Vec<_>>(),
            vec![("alpha", Some(1)), ("zeta", Some(1)), ("lower", Some(3))]
        );
    }

    #[test]
    fn best_selection_returns_none_without_eligible_evidence() {
        let objective = ObjectiveMetric {
            metric_key: MetricKey::from_string("loss"),
            direction: ObjectiveDirection::Minimize,
        };
        let result = rank_candidates(
            &objective,
            vec![candidate(
                "failed",
                Some(0.5),
                EvidenceCompleteness::Partial,
                Utc::now(),
                0,
            )],
        );

        assert!(result.entries.iter().all(|entry| entry.rank.is_none()));
    }
}
