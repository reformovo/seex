use chrono::{DateTime, Utc};

use crate::types::ProjectId;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct RunId(String);

impl RunId {
    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunStatus {
    Running,
    Finished,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Run {
    pub run_id: RunId,
    pub project_id: ProjectId,
    pub name: String,
    pub status: RunStatus,
    pub created_at: DateTime<Utc>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl Run {
    pub fn finish(self, finished_at: DateTime<Utc>) -> Self {
        Self {
            status: RunStatus::Finished,
            finished_at: Some(finished_at),
            ..self
        }
    }
}
