use crate::domain::RunRef;
use crate::workbench::panel_reads::MetricPanelId;

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct InteractionSnapshot {
    pub emphasized_run: Option<RunRef>,
    pub ruler_hover: Option<f64>,
    pub track_pointer_hover: Option<(MetricPanelId, f64)>,
    pub locked_cursor: Option<f64>,
}

/// Transient interaction shared by the sidebar, charts, ruler, and inspector.
#[derive(Default)]
pub(super) struct WorkbenchInteraction {
    state: InteractionSnapshot,
}

impl WorkbenchInteraction {
    pub fn snapshot(&self) -> InteractionSnapshot {
        self.state.clone()
    }

    pub fn set_emphasized_run(&mut self, run: Option<RunRef>) -> bool {
        replace_if_changed(&mut self.state.emphasized_run, run)
    }

    pub fn set_ruler_hover(&mut self, axis: Option<f64>) -> bool {
        replace_if_changed(&mut self.state.ruler_hover, axis)
    }

    pub fn set_track_pointer_hover(&mut self, hover: Option<(MetricPanelId, f64)>) -> bool {
        replace_if_changed(&mut self.state.track_pointer_hover, hover)
    }

    pub fn set_locked_cursor(&mut self, axis: Option<f64>) -> bool {
        replace_if_changed(&mut self.state.locked_cursor, axis)
    }

    pub fn clear_cursors(&mut self) -> bool {
        let changed = self.state.ruler_hover.is_some()
            || self.state.track_pointer_hover.is_some()
            || self.state.locked_cursor.is_some();
        self.state.ruler_hover = None;
        self.state.track_pointer_hover = None;
        self.state.locked_cursor = None;
        changed
    }
}

fn replace_if_changed<T: PartialEq>(current: &mut T, next: T) -> bool {
    if *current == next {
        return false;
    }
    *current = next;
    true
}

#[cfg(test)]
mod tests {
    use crate::domain::{DataSourceId, RunRef};
    use seex_model::run::RunId;
    use seex_model::types::ProjectId;

    use super::WorkbenchInteraction;

    #[test]
    fn interaction_changes_are_reported_without_mutating_other_channels() {
        let mut interaction = WorkbenchInteraction::default();
        let run = RunRef::new(
            DataSourceId::new("source").expect("test alias should be valid"),
            ProjectId::from_string("project"),
            RunId::from_string("run"),
        );

        assert!(interaction.set_emphasized_run(Some(run.clone())));
        assert!(!interaction.set_emphasized_run(Some(run.clone())));
        assert!(interaction.set_locked_cursor(Some(42.)));

        let snapshot = interaction.snapshot();
        assert_eq!(snapshot.emphasized_run, Some(run));
        assert_eq!(snapshot.locked_cursor, Some(42.));
    }
}
