use pulseon_chart_core::{AxisRange, BrushState};
use pulseon_model::alignment::{AlignmentAxis, AlignmentViewport};

/// Per-View navigation state shared by the brush, ruler, and Metric tracks.
#[derive(Clone)]
pub struct ViewNavigation {
    axis: AlignmentAxis,
    brush: Option<BrushState>,
}

impl Default for ViewNavigation {
    fn default() -> Self {
        Self {
            axis: AlignmentAxis::Step,
            brush: None,
        }
    }
}

impl ViewNavigation {
    pub const fn axis(&self) -> AlignmentAxis {
        self.axis
    }

    pub const fn brush(&self) -> Option<BrushState> {
        self.brush
    }

    pub fn brush_mut(&mut self) -> Option<&mut BrushState> {
        self.brush.as_mut()
    }

    pub fn zoom_at(&mut self, anchor: f64, factor: f64) -> bool {
        self.brush
            .as_mut()
            .is_some_and(|brush| brush.zoom_at(anchor, factor).is_ok())
    }

    pub fn pan_by(&mut self, delta: f64) -> bool {
        self.brush
            .as_mut()
            .is_some_and(|brush| brush.pan_by(delta).is_ok())
    }

    pub fn resize_start(&mut self, position: f64) -> bool {
        self.brush
            .as_mut()
            .is_some_and(|brush| brush.resize_start(position).is_ok())
    }

    pub fn resize_end(&mut self, position: f64) -> bool {
        self.brush
            .as_mut()
            .is_some_and(|brush| brush.resize_end(position).is_ok())
    }

    pub fn selected_viewport(&self) -> Option<AlignmentViewport> {
        let selected = self.brush?.selected();
        AlignmentViewport::new(
            selected.start().floor() as i64,
            selected.end().ceil() as i64,
        )
        .ok()
    }

    pub fn select_axis(&mut self, axis: AlignmentAxis) {
        if self.axis != axis {
            self.axis = axis;
            self.brush = None;
        }
    }

    pub fn reset_view(&mut self) -> bool {
        let Some(brush) = self.brush.as_mut() else {
            return false;
        };
        brush.reset();
        true
    }

    pub fn set_timeline_home(&mut self, range: AlignmentViewport) {
        let Ok(home) = AxisRange::new(range.start() as f64, range.end() as f64) else {
            return;
        };
        let previous = self.brush.map(BrushState::selected);
        let Ok(mut brush) = BrushState::new(home) else {
            return;
        };
        if let Some(previous) = previous {
            let start = previous.start().clamp(home.start(), home.end());
            let end = previous.end().clamp(home.start(), home.end());
            if end - start >= 1. {
                let _ = brush.resize_start(start);
                let _ = brush.resize_end(end);
            }
        }
        self.brush = Some(brush);
    }

    pub fn clear_timeline(&mut self) {
        self.brush = None;
    }
}

#[cfg(test)]
mod tests {
    use pulseon_model::alignment::{AlignmentAxis, AlignmentViewport};

    use super::ViewNavigation;

    #[test]
    fn view_commands_switch_axis_and_reset_the_brush() {
        let mut navigation = ViewNavigation::default();
        navigation.set_timeline_home(
            AlignmentViewport::new(0, 10).expect("test viewport should be valid"),
        );
        navigation
            .brush_mut()
            .expect("timeline should initialize brush")
            .resize_start(4.)
            .expect("test brush should resize");

        navigation.select_axis(AlignmentAxis::ElapsedTime);
        assert_eq!(navigation.axis(), AlignmentAxis::ElapsedTime);
        assert!(navigation.brush().is_none());

        navigation.set_timeline_home(
            AlignmentViewport::new(0, 10).expect("test viewport should be valid"),
        );
        navigation
            .brush_mut()
            .expect("timeline should initialize brush")
            .resize_start(4.)
            .expect("test brush should resize");
        assert!(navigation.reset_view());
        let brush = navigation.brush().expect("reset should retain brush");
        assert_eq!(brush.selected(), brush.home());
    }
}
