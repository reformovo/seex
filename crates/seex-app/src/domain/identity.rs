use std::fmt;
use std::path::Path;

use seex_model::run::{Run, RunId, RunStatus};
use seex_model::types::ProjectId;

pub const MAX_SELECTED_RUNS: usize = 20;

/// Stable portable identity assigned to one imported Data source.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SourceAlias(String);

impl SourceAlias {
    /// Creates a lowercase portable alias.
    ///
    /// # Errors
    ///
    /// Returns [`SourceAliasError::Invalid`] unless the alias starts with an
    /// ASCII lowercase letter or digit and contains only lowercase letters,
    /// digits, dots, hyphens, or underscores.
    pub fn new(value: impl Into<String>) -> Result<Self, SourceAliasError> {
        let value = value.into();
        let mut characters = value.chars();
        let starts_portably = characters
            .next()
            .is_some_and(|character| character.is_ascii_lowercase() || character.is_ascii_digit());
        let remains_portable = characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '.' | '-' | '_')
        });
        if !starts_portably || !remains_portable {
            return Err(SourceAliasError::Invalid);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceAlias {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Invalid stable Source alias.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SourceAliasError {
    #[error("Source alias must be a lowercase portable identifier")]
    Invalid,
}

/// Stable viewer-local identity for one imported native source.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DataSourceId(String);

impl DataSourceId {
    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn from_path(path: &Path) -> Self {
        Self(path.to_string_lossy().into_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DataSourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Collision-free identity for one Run selected from an imported source.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RunRef {
    pub source_id: DataSourceId,
    pub project_id: ProjectId,
    pub run_id: RunId,
}

impl RunRef {
    pub const fn new(source_id: DataSourceId, project_id: ProjectId, run_id: RunId) -> Self {
        Self {
            source_id,
            project_id,
            run_id,
        }
    }

    pub fn cache_key(&self) -> String {
        format!(
            "{}:{}{}:{}{}:{}",
            self.source_id.as_str().len(),
            self.source_id.as_str(),
            self.project_id.as_str().len(),
            self.project_id.as_str(),
            self.run_id.as_str().len(),
            self.run_id.as_str(),
        )
    }
}

/// Matches a Run by name, identifier, or lifecycle status.
pub fn run_matches_filter(run: &Run, query: &str) -> bool {
    let status = match run.status {
        RunStatus::Running => "running",
        RunStatus::Finished => "finished",
        RunStatus::Failed => "failed",
    };
    run_fields_match_filter(&run.name, run.run_id.as_str(), status, query)
}

fn run_fields_match_filter(name: &str, run_id: &str, status: &str, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty()
        || name.to_lowercase().contains(&query)
        || run_id.to_lowercase().contains(&query)
        || status.contains(&query)
}

/// Invalid user selection transitions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SelectionError {
    #[error("at most {MAX_SELECTED_RUNS} Runs can be selected")]
    RunLimit,
}

#[cfg(test)]
mod tests {
    use seex_model::run::{Run, RunId, RunStatus};
    use seex_model::types::ProjectId;

    use super::{
        DataSourceId, MAX_SELECTED_RUNS, RunRef, SelectionError, SourceAlias, SourceAliasError,
        run_matches_filter,
    };

    fn run_ref(source: &str, project: &str, run: &str) -> RunRef {
        RunRef::new(
            DataSourceId::from_string(source),
            ProjectId::from_string(project),
            RunId::from_string(run),
        )
    }

    #[test]
    fn selection_limit_error_uses_the_product_limit() {
        assert_eq!(MAX_SELECTED_RUNS, 20);
        assert_eq!(
            SelectionError::RunLimit.to_string(),
            "at most 20 Runs can be selected"
        );
    }

    #[test]
    fn source_alias_accepts_portable_names_and_rejects_paths_or_display_names() {
        let alias = SourceAlias::new("local-benchmarks").expect("portable alias should be valid");

        assert_eq!(alias.as_str(), "local-benchmarks");
        for invalid in ["", "Research", ".hidden", "/tmp/research", "source alias"] {
            assert_eq!(SourceAlias::new(invalid), Err(SourceAliasError::Invalid));
        }
    }

    #[test]
    fn run_identity_distinguishes_identical_native_ids_from_different_sources() {
        let first = run_ref("source-a", "project-1", "run-1");
        let second = run_ref("source-b", "project-1", "run-1");

        assert_ne!(first, second);
        assert_ne!(first.cache_key(), second.cache_key());
    }

    #[test]
    fn run_filter_matches_name_id_and_status_without_case() {
        let timestamp = "2026-01-01T00:00:00Z"
            .parse()
            .expect("test timestamp should parse");
        let run = Run {
            run_id: RunId::from_string("RUN-42"),
            project_id: ProjectId::from_string("project"),
            name: "Loss Baseline".to_owned(),
            status: RunStatus::Failed,
            created_at: timestamp,
            started_at: timestamp,
            finished_at: None,
        };

        assert!(
            ["loss", "run-42", "FAILED", ""]
                .into_iter()
                .all(|query| run_matches_filter(&run, query))
        );
        assert!(!run_matches_filter(&run, "running"));
    }
}
