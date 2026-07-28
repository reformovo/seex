use chrono::{DateTime, Utc};

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ProjectId(String);

impl ProjectId {
    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Project {
    pub project_id: ProjectId,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::{Project, ProjectId};
    use crate::metric::{
        MetricAggregate, MetricKey, MetricPoint, MetricQuery, ReductionPolicy, Step,
    };
    use crate::run::{Run, RunId, RunStatus};

    #[test]
    fn v1_native_model_keeps_typed_identity_lifecycle_and_metric_shape() {
        // Given
        let project_id = ProjectId::from_string("project-local");
        let run_id = RunId::from_string("run-1");
        let metric_key = MetricKey::from_string("train/loss");
        let created_at = chrono::Utc::now();
        let started_at = created_at + chrono::TimeDelta::seconds(1);
        let ingested_at = started_at + chrono::TimeDelta::milliseconds(3);

        // When
        let project = Project {
            project_id: project_id.clone(),
            name: String::from("local training"),
            created_at,
        };
        let finished = Run {
            run_id: run_id.clone(),
            project_id: project_id.clone(),
            name: String::from("baseline"),
            status: RunStatus::Running,
            created_at,
            started_at,
            finished_at: None,
        }
        .finish(started_at);
        let point = MetricPoint {
            run_id: run_id.clone(),
            metric_key: metric_key.clone(),
            step: Step::new(42),
            timestamp: started_at,
            value_f64: 0.125,
            ingested_at,
        };
        let aggregate = MetricAggregate {
            run_id: run_id.clone(),
            metric_key: metric_key.clone(),
            effective_count: 1,
            last_step: point.step,
            last_value_f64: point.value_f64,
            min_value_f64: point.value_f64,
            max_value_f64: point.value_f64,
        };
        let query = MetricQuery::new(
            run_id,
            metric_key,
            Some(Step::new(10)),
            Some(Step::new(50)),
            ReductionPolicy::lttb(500).expect("test reduction should be valid"),
        )
        .expect("test query should be valid");

        // Then
        assert_eq!(project.project_id, project_id);
        assert_eq!(project.project_id.as_str(), "project-local");
        assert_eq!(finished.status, RunStatus::Finished);
        assert_eq!(finished.finished_at, Some(started_at));
        assert_eq!(finished.run_id.as_str(), "run-1");
        assert_eq!(point.step.value(), 42);
        assert_eq!(point.metric_key.as_str(), "train/loss");
        assert_eq!(aggregate.effective_count, 1);
        assert_eq!(aggregate.last_step, Step::new(42));
        assert_eq!(query.start_step, Some(Step::new(10)));
        assert_eq!(query.reduction.max_points(), Some(500));
    }
}
