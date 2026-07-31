//! Viewer identities, selection rules, and shared navigation state.

mod identity;
mod navigation;

pub use identity::{
    DataSourceId, MAX_SELECTED_RUNS, RunRef, SelectionError, SourceAlias, SourceAliasError,
    run_matches_filter,
};
pub use navigation::ViewNavigation;
