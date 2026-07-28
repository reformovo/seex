use std::sync::Arc;

use gpui::{
    AnyView, Context, Entity, IntoElement, ParentElement, Pixels, Point, Render, StyleRefinement,
    Styled, Window,
};
use seex_chart_core::{AxisRange, BrushState, CanvasSize, Viewport};

use crate::data::query::CurveSnapshot;
use crate::domain::RunRef;

use super::{ChartAdapter, detail_canvas, timeline_canvas};

#[derive(Clone, Debug)]
pub(in crate::desktop::app) struct HoverPoint {
    pub run_ref: RunRef,
    pub axis_value: i64,
    pub value: f64,
    pub canvas_position: Point<Pixels>,
    pub align_left: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::desktop::app) enum BrushDragTarget {
    Start,
    End,
    Window,
}

pub(in crate::desktop::app) struct DetailChart {
    adapter: std::rc::Rc<std::cell::RefCell<ChartAdapter>>,
    snapshot: Arc<CurveSnapshot>,
    revision: u64,
    viewport: Viewport,
    baseline: Option<RunRef>,
    emphasized_run: Option<RunRef>,
    visible_runs: std::rc::Rc<[RunRef]>,
}

#[derive(Default)]
pub(in crate::desktop::app) struct OverviewChart {
    adapter: std::rc::Rc<std::cell::RefCell<ChartAdapter>>,
    brush: Option<BrushState>,
    snapshot: Option<Arc<CurveSnapshot>>,
    revision: u64,
    emphasized_run: Option<RunRef>,
    visible_runs: std::rc::Rc<[RunRef]>,
}

impl OverviewChart {
    pub fn update(
        &mut self,
        brush: BrushState,
        snapshot: Option<Arc<CurveSnapshot>>,
        revision: u64,
        emphasized_run: Option<RunRef>,
        visible_runs: std::rc::Rc<[RunRef]>,
    ) -> bool {
        if self.brush == Some(brush)
            && option_arc_ptr_eq(&self.snapshot, &snapshot)
            && self.revision == revision
            && self.emphasized_run == emphasized_run
            && self.visible_runs == visible_runs
        {
            return false;
        }
        self.brush = Some(brush);
        self.snapshot = snapshot;
        self.revision = revision;
        self.emphasized_run = emphasized_run;
        self.visible_runs = visible_runs;
        true
    }

    pub fn brush_target(
        &self,
        brush: BrushState,
        cursor: Point<Pixels>,
    ) -> Option<BrushDragTarget> {
        self.adapter.borrow().brush_target(brush, cursor)
    }

    pub fn axis_at(&self, brush: BrushState, cursor: Point<Pixels>) -> Option<f64> {
        self.adapter.borrow().overview_axis_at(brush, cursor)
    }
}

impl Render for OverviewChart {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let brush = self.brush;
        gpui::div().size_full().children(brush.map(|brush| {
            timeline_canvas(
                std::rc::Rc::clone(&self.adapter),
                brush,
                self.snapshot.clone(),
                self.revision,
                self.emphasized_run.clone(),
                std::rc::Rc::clone(&self.visible_runs),
            )
            .size_full()
        }))
    }
}

fn option_arc_ptr_eq<T>(left: &Option<Arc<T>>, right: &Option<Arc<T>>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => Arc::ptr_eq(left, right),
        (None, None) => true,
        _ => false,
    }
}

impl DetailChart {
    pub fn new(
        snapshot: Arc<CurveSnapshot>,
        revision: u64,
        viewport: Viewport,
        baseline: Option<RunRef>,
        emphasized_run: Option<RunRef>,
        visible_runs: std::rc::Rc<[RunRef]>,
    ) -> Self {
        Self {
            adapter: std::rc::Rc::new(std::cell::RefCell::new(ChartAdapter::default())),
            snapshot,
            revision,
            viewport,
            baseline,
            emphasized_run,
            visible_runs,
        }
    }

    pub fn update(
        &mut self,
        snapshot: Arc<CurveSnapshot>,
        revision: u64,
        viewport: Viewport,
        baseline: Option<RunRef>,
        emphasized_run: Option<RunRef>,
        visible_runs: std::rc::Rc<[RunRef]>,
    ) -> bool {
        if Arc::ptr_eq(&self.snapshot, &snapshot)
            && self.revision == revision
            && self.viewport == viewport
            && self.baseline == baseline
            && self.emphasized_run == emphasized_run
            && self.visible_runs == visible_runs
        {
            return false;
        }
        self.snapshot = snapshot;
        self.revision = revision;
        self.viewport = viewport;
        self.baseline = baseline;
        self.emphasized_run = emphasized_run;
        self.visible_runs = visible_runs;
        true
    }

    pub fn warm_projection(&self, canvas: CanvasSize) {
        self.adapter.borrow_mut().warm_projection(
            &self.snapshot,
            self.revision,
            self.viewport,
            canvas,
            &self.visible_runs,
        );
    }

    pub fn axis_at(&self, range: AxisRange, cursor: Point<Pixels>) -> Option<f64> {
        self.adapter.borrow().detail_axis_at(range, cursor)
    }

    pub fn pan_delta(
        &self,
        range: AxisRange,
        previous_x: f64,
        cursor: Point<Pixels>,
    ) -> Option<f64> {
        self.adapter
            .borrow()
            .detail_pan_delta(range, previous_x, cursor)
    }

    pub fn points_at_axis(&self, axis: f64) -> Vec<HoverPoint> {
        self.adapter.borrow().points_at_axis(
            &self.snapshot,
            self.viewport,
            axis,
            &self.visible_runs,
        )
    }

    pub fn hit_test(&self, cursor: Point<Pixels>) -> (Option<f64>, Option<HoverPoint>) {
        let adapter = self.adapter.borrow();
        let pointer_axis = adapter.detail_axis_at(self.viewport.x, cursor);
        let pointer_anchor = adapter.detail_pointer_anchor(cursor);
        let mut hover = adapter.hit_test(&self.snapshot, self.viewport, cursor, &self.visible_runs);
        if let (Some(hover), Some((x, align_left))) = (&mut hover, pointer_anchor) {
            hover.canvas_position.x = x;
            hover.align_left = align_left;
        }
        (pointer_axis, hover)
    }

    #[cfg(all(test, feature = "test-support"))]
    pub(in crate::desktop::app) fn prepare_count(&self) -> usize {
        self.adapter.borrow().detail_prepare_count()
    }

    #[cfg(all(test, feature = "test-support"))]
    pub(in crate::desktop::app) const fn viewport(&self) -> Viewport {
        self.viewport
    }
}

impl Render for DetailChart {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        detail_canvas(
            std::rc::Rc::clone(&self.adapter),
            Arc::clone(&self.snapshot),
            self.revision,
            self.viewport,
            self.baseline.clone(),
            self.emphasized_run.clone(),
            std::rc::Rc::clone(&self.visible_runs),
        )
        .size_full()
    }
}

pub(in crate::desktop::app) fn cached_detail_chart(chart: Entity<DetailChart>) -> AnyView {
    AnyView::from(chart).cached(StyleRefinement::default().size_full())
}
