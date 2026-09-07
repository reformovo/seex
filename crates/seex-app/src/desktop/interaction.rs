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
        let changed = replace_if_changed(&mut self.state.ruler_hover, axis);
        let cleared_track = if axis.is_some() {
            self.state.track_pointer_hover.take().is_some()
        } else {
            false
        };
        changed || cleared_track
    }

    pub fn set_track_pointer_hover(&mut self, hover: Option<(MetricPanelId, f64)>) -> bool {
        let changed = replace_if_changed(&mut self.state.track_pointer_hover, hover);
        let cleared_ruler = if self.state.track_pointer_hover.is_some() {
            self.state.ruler_hover.take().is_some()
        } else {
            false
        };
        changed || cleared_ruler
    }

    pub fn track_pointer_panel(&self) -> Option<&MetricPanelId> {
        self.state
            .track_pointer_hover
            .as_ref()
            .map(|(panel_id, _)| panel_id)
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
    use seex::ProjectId;
    use seex::RunId;

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

    #[test]
    fn pointer_channels_are_mutually_exclusive_in_one_update() {
        let mut interaction = WorkbenchInteraction::default();
        let panel_id = crate::workbench::panel_reads::MetricPanelId::from_string("loss");

        assert!(interaction.set_ruler_hover(Some(1.)));
        assert!(interaction.set_track_pointer_hover(Some((panel_id.clone(), 2.))));
        let track = interaction.snapshot();
        assert_eq!(track.track_pointer_hover, Some((panel_id, 2.)));
        assert_eq!(track.ruler_hover, None);

        assert!(interaction.set_ruler_hover(Some(3.)));
        let ruler = interaction.snapshot();
        assert_eq!(ruler.ruler_hover, Some(3.));
        assert_eq!(ruler.track_pointer_hover, None);
    }
}
