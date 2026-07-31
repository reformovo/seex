use std::cmp::Ordering;

use gpui::{
    Context, MouseButton, MouseDownEvent, MouseUpEvent, SharedString, Transformation, div,
    prelude::*, px,
};
use seex_model::comparison::EvidenceCompleteness;
use seex_model::run::RunStatus;

use crate::data::query::{InspectorRunSnapshot, InspectorSnapshot};
use crate::domain::RunRef;
use crate::workbench::MetricPanel;

use super::super::components::{self, IconName, ResizeEdge, resize_handle};
use super::super::project_sidebar::run_status;
use super::super::theme::ViewerTheme;
use super::super::{format_signed_delta, reasons_label};
use super::BottomInspector;

#[derive(Clone, Copy, Debug)]
pub(crate) struct InspectorResize {
    pub start_y: gpui::Pixels,
    pub start_height: gpui::Pixels,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct InspectorColumnResize {
    pub column: InspectorColumn,
    pub start_x: gpui::Pixels,
    pub start_width: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InspectorColumn {
    Run,
    LastValue,
    Minimum,
    Maximum,
    Locked,
    Hover,
    Count,
    LastStep,
    Status,
    Evidence,
    Project,
}

pub(crate) const INSPECTOR_COLUMNS: [InspectorColumn; 11] = [
    InspectorColumn::Run,
    InspectorColumn::LastValue,
    InspectorColumn::Minimum,
    InspectorColumn::Maximum,
    InspectorColumn::Locked,
    InspectorColumn::Hover,
    InspectorColumn::Count,
    InspectorColumn::LastStep,
    InspectorColumn::Status,
    InspectorColumn::Evidence,
    InspectorColumn::Project,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InspectorSortDirection {
    Ascending,
    Descending,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InspectorSort {
    pub column: InspectorColumn,
    pub direction: InspectorSortDirection,
}

#[derive(Clone)]
pub(crate) struct InspectorRow {
    pub run_ref: RunRef,
    pub run_label: String,
    pub project_label: String,
    pub status: RunStatus,
    pub evidence: EvidenceCompleteness,
    pub evidence_label: String,
    pub count: Option<u64>,
    pub last_step: Option<i64>,
    pub last_value: Option<f64>,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub locked: Option<(i64, f64)>,
    pub hover: Option<(i64, f64)>,
    pub baseline: bool,
    pub pinned: bool,
    pub original_order: usize,
}

pub(crate) struct InspectorRowsContext<'a> {
    pub visible_runs: &'a [RunRef],
    pub baseline: Option<&'a RunRef>,
    pub pinned: &'a [RunRef],
    pub locked_axis: Option<f64>,
    pub hover_axis: Option<f64>,
    pub sort: Option<InspectorSort>,
}

pub(crate) fn inspector_header_cell(
    column: InspectorColumn,
    width: f32,
    direction: Option<InspectorSortDirection>,
    boundary_active: bool,
    column_resize_active: bool,
    theme: ViewerTheme,
    cx: &mut Context<BottomInspector>,
) -> gpui::Stateful<gpui::Div> {
    let indicator = direction.map(|direction| {
        let icon = components::icon(IconName::ChevronUp, theme).size(px(12.));
        match direction {
            InspectorSortDirection::Ascending => icon,
            InspectorSortDirection::Descending => {
                icon.with_transformation(Transformation::rotate(gpui::percentage(0.5)))
            }
        }
    });
    let resize_id = SharedString::from(format!("inspector-column-resize:{}", column.key()));
    div()
        .id(SharedString::from(format!(
            "inspector-header:{}",
            column.key()
        )))
        .debug_selector(move || format!("inspector-header:{}", column.key()))
        .w(px(width))
        .h(theme.spacing.control_height)
        .flex_none()
        .relative()
        .px_2()
        .flex()
        .items_center()
        .gap_1()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme.colors.text)
        .when(!column_resize_active, |cell| cell.cursor_pointer())
        .when(column_resize_active, |cell| {
            cell.cursor(gpui::CursorStyle::ResizeLeftRight)
        })
        .hover(|style| style.bg(theme.colors.element_hover))
        .when(column.right_aligned(), |cell| cell.justify_end())
        .on_click(cx.listener(move |this, _, _, cx| {
            this.toggle_inspector_sort(column, cx);
        }))
        .child(column.label())
        .children(indicator)
        .children(column.owns_resize_boundary().then(|| {
            resize_handle(resize_id, theme, boundary_active, ResizeEdge::Right)
                .debug_selector(move || format!("inspector-column-resize:{}", column.key()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.begin_inspector_column_resize(column, event, cx);
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                        this.finish_inspector_column_resize(cx);
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                        this.finish_inspector_column_resize(cx);
                    }),
                )
                .on_click(|_, _, cx| cx.stop_propagation())
        }))
}

pub(crate) fn inspector_run_cell(
    row: &InspectorRow,
    width: f32,
    color: gpui::Rgba,
    theme: ViewerTheme,
) -> gpui::Div {
    div()
        .w(px(width))
        .h_full()
        .flex_none()
        .px_2()
        .overflow_hidden()
        .whitespace_nowrap()
        .flex()
        .items_center()
        .gap_1()
        .child(div().size(px(7.)).flex_none().rounded(px(3.5)).bg(color))
        .child(row.run_label.clone())
        .children(
            row.baseline
                .then(|| components::icon(IconName::Baseline, theme).size(px(12.))),
        )
        .children(
            (row.pinned && !row.baseline)
                .then(|| components::icon(IconName::Pin, theme).size(px(12.))),
        )
}

pub(crate) fn inspector_project_width(rows: &[InspectorRow]) -> f32 {
    rows.iter()
        .map(|row| row.project_label.chars().count() as f32 * 7. + 24.)
        .fold(InspectorColumn::Project.default_width(), f32::max)
}

pub(crate) fn inspector_rows(
    panel: &MetricPanel,
    snapshot: &InspectorSnapshot,
    context: InspectorRowsContext<'_>,
) -> Vec<InspectorRow> {
    let mut rows = snapshot
        .runs
        .iter()
        .filter(|run| context.visible_runs.contains(&run.run_ref))
        .enumerate()
        .map(|(original_order, run)| {
            inspector_row(
                panel,
                run,
                context.baseline,
                context.pinned,
                context.locked_axis,
                context.hover_axis,
                original_order,
            )
        })
        .collect::<Vec<_>>();
    sort_inspector_rows(&mut rows, context.sort);
    rows
}

pub(crate) fn sort_inspector_rows(rows: &mut [InspectorRow], sort: Option<InspectorSort>) {
    rows.sort_by(|left, right| {
        inspector_row_group(left)
            .cmp(&inspector_row_group(right))
            .then_with(|| {
                sort.map_or_else(
                    || left.original_order.cmp(&right.original_order),
                    |sort| {
                        compare_inspector_rows(sort, left, right)
                            .then_with(|| left.original_order.cmp(&right.original_order))
                    },
                )
            })
    });
}

pub(crate) fn inspector_row(
    panel: &MetricPanel,
    run: &InspectorRunSnapshot,
    baseline: Option<&RunRef>,
    pinned: &[RunRef],
    locked_axis: Option<f64>,
    hover_axis: Option<f64>,
    original_order: usize,
) -> InspectorRow {
    let summary = run.summary.as_ref();
    InspectorRow {
        run_ref: run.run_ref.clone(),
        run_label: run.run.name.clone(),
        project_label: format!(
            "{} · {}",
            run.run_ref.project_id.as_str(),
            run.run_ref.source_id
        ),
        status: run.evidence.run_status,
        evidence: run.evidence.completeness,
        evidence_label: format!(
            "{:?}{}",
            run.evidence.completeness,
            reasons_label(&run.evidence.reasons)
        ),
        count: summary.map(|summary| summary.effective_count),
        last_step: summary.map(|summary| summary.last_step.value()),
        last_value: summary.map(|summary| summary.last_value_f64),
        minimum: summary.map(|summary| summary.min_value_f64),
        maximum: summary.map(|summary| summary.max_value_f64),
        locked: locked_axis.and_then(|axis| inspector_cursor_point(panel, &run.run_ref, axis)),
        hover: hover_axis.and_then(|axis| inspector_cursor_point(panel, &run.run_ref, axis)),
        baseline: baseline == Some(&run.run_ref),
        pinned: pinned.contains(&run.run_ref),
        original_order,
    }
}

pub(crate) fn inspector_cursor_point(
    panel: &MetricPanel,
    run_ref: &RunRef,
    axis: f64,
) -> Option<(i64, f64)> {
    panel
        .detail
        .as_ref()?
        .series
        .iter()
        .find(|series| &series.run_ref == run_ref)?
        .chart_series
        .as_ref()?
        .points()
        .iter()
        .min_by(|left, right| (left.x - axis).abs().total_cmp(&(right.x - axis).abs()))
        .map(|point| (point.x as i64, point.y))
}

pub(crate) const fn inspector_row_group(row: &InspectorRow) -> u8 {
    if row.baseline {
        0
    } else if row.pinned {
        1
    } else {
        2
    }
}

pub(crate) fn compare_inspector_rows(
    sort: InspectorSort,
    left: &InspectorRow,
    right: &InspectorRow,
) -> Ordering {
    let direction = sort.direction;
    match sort.column {
        InspectorColumn::Run => direction.apply(left.run_label.cmp(&right.run_label)),
        InspectorColumn::LastValue => {
            compare_optional(left.last_value, right.last_value, direction, f64::total_cmp)
        }
        InspectorColumn::Minimum => {
            compare_optional(left.minimum, right.minimum, direction, f64::total_cmp)
        }
        InspectorColumn::Maximum => {
            compare_optional(left.maximum, right.maximum, direction, f64::total_cmp)
        }
        InspectorColumn::Locked => compare_optional(
            left.locked.map(|(_, value)| value),
            right.locked.map(|(_, value)| value),
            direction,
            f64::total_cmp,
        ),
        InspectorColumn::Hover => compare_optional(
            left.hover.map(|(_, value)| value),
            right.hover.map(|(_, value)| value),
            direction,
            f64::total_cmp,
        ),
        InspectorColumn::Count => compare_optional(left.count, right.count, direction, u64::cmp),
        InspectorColumn::LastStep => {
            compare_optional(left.last_step, right.last_step, direction, i64::cmp)
        }
        InspectorColumn::Status => direction
            .apply(inspector_status_order(left.status).cmp(&inspector_status_order(right.status))),
        InspectorColumn::Evidence => direction.apply(
            inspector_evidence_order(left.evidence).cmp(&inspector_evidence_order(right.evidence)),
        ),
        InspectorColumn::Project => direction.apply(left.project_label.cmp(&right.project_label)),
    }
}

pub(crate) fn compare_optional<T: Copy>(
    left: Option<T>,
    right: Option<T>,
    direction: InspectorSortDirection,
    compare: impl FnOnce(&T, &T) -> Ordering,
) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => direction.apply(compare(&left, &right)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

pub(crate) const fn inspector_status_order(status: RunStatus) -> u8 {
    match status {
        RunStatus::Running => 0,
        RunStatus::Finished => 1,
        RunStatus::Failed => 2,
    }
}

pub(crate) const fn inspector_evidence_order(evidence: EvidenceCompleteness) -> u8 {
    match evidence {
        EvidenceCompleteness::Complete => 0,
        EvidenceCompleteness::Partial => 1,
        EvidenceCompleteness::Unavailable => 2,
        EvidenceCompleteness::Invalid => 3,
    }
}

pub(crate) fn inspector_cell_text(
    column: InspectorColumn,
    row: &InspectorRow,
    baseline: Option<&InspectorRow>,
) -> String {
    match column {
        InspectorColumn::Run => row.run_label.clone(),
        InspectorColumn::LastValue => inspector_float(
            row.last_value,
            baseline.and_then(|baseline| baseline.last_value),
            row.baseline,
        ),
        InspectorColumn::Minimum => inspector_float(
            row.minimum,
            baseline.and_then(|baseline| baseline.minimum),
            row.baseline,
        ),
        InspectorColumn::Maximum => inspector_float(
            row.maximum,
            baseline.and_then(|baseline| baseline.maximum),
            row.baseline,
        ),
        InspectorColumn::Locked => inspector_cursor_cell(
            row.locked,
            baseline.and_then(|baseline| baseline.locked),
            row.baseline,
        ),
        InspectorColumn::Hover => inspector_cursor_cell(
            row.hover,
            baseline.and_then(|baseline| baseline.hover),
            row.baseline,
        ),
        InspectorColumn::Count => inspector_integer(row.count.map(i128::from), None, true),
        InspectorColumn::LastStep => inspector_integer(row.last_step.map(i128::from), None, true),
        InspectorColumn::Status => run_status(row.status).to_owned(),
        InspectorColumn::Evidence => row.evidence_label.clone(),
        InspectorColumn::Project => row.project_label.clone(),
    }
}

pub(crate) fn inspector_float(
    value: Option<f64>,
    baseline: Option<f64>,
    is_baseline: bool,
) -> String {
    value.map_or_else(String::new, |value| {
        let value_label = format!("{value:.6}");
        if is_baseline {
            value_label
        } else {
            baseline.map_or(value_label.clone(), |baseline| {
                format!(
                    "{value_label} ({})",
                    format_signed_delta(value - baseline, 6)
                )
            })
        }
    })
}

pub(crate) fn inspector_integer(
    value: Option<i128>,
    baseline: Option<i128>,
    is_baseline: bool,
) -> String {
    value.map_or_else(String::new, |value| {
        let value_label = format_grouped_integer(value);
        if is_baseline {
            value_label
        } else {
            baseline.map_or(value_label.clone(), |baseline| {
                format!(
                    "{value_label} ({})",
                    format_signed_integer(value - baseline)
                )
            })
        }
    })
}

pub(crate) fn inspector_cursor_cell(
    point: Option<(i64, f64)>,
    baseline: Option<(i64, f64)>,
    is_baseline: bool,
) -> String {
    point.map_or_else(String::new, |(_, value)| {
        inspector_float(
            Some(value),
            baseline.map(|(_, baseline)| baseline),
            is_baseline,
        )
    })
}

pub(crate) fn format_signed_integer(value: i128) -> String {
    if value.is_negative() {
        format!("−{}", format_grouped_integer(value.saturating_abs()))
    } else {
        format!("+{}", format_grouped_integer(value))
    }
}

pub(crate) fn format_grouped_integer(value: i128) -> String {
    let negative = value.is_negative();
    let digits = value.saturating_abs().to_string();
    let mut grouped =
        String::with_capacity(digits.len() + digits.len() / 3 + usize::from(negative));
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(character);
    }
    if negative {
        grouped.insert(0, '−');
    }
    grouped
}

impl InspectorColumn {
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::LastValue => "last-value",
            Self::Minimum => "min",
            Self::Maximum => "max",
            Self::Locked => "locked",
            Self::Hover => "hover",
            Self::Count => "count",
            Self::LastStep => "last-step",
            Self::Status => "status",
            Self::Evidence => "evidence",
            Self::Project => "project",
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Run => "Run",
            Self::LastValue => "Last value",
            Self::Minimum => "Min",
            Self::Maximum => "Max",
            Self::Locked => "Locked",
            Self::Hover => "Hover",
            Self::Count => "Count",
            Self::LastStep => "Last step",
            Self::Status => "Status",
            Self::Evidence => "Evidence",
            Self::Project => "Project",
        }
    }

    pub(crate) const fn default_width(self) -> f32 {
        match self {
            Self::Run => 220.,
            Self::LastValue => 160.,
            Self::Minimum | Self::Maximum => 140.,
            Self::Locked | Self::Hover => 210.,
            Self::Count => 130.,
            Self::LastStep => 150.,
            Self::Status => 100.,
            Self::Evidence => 220.,
            Self::Project => 260.,
        }
    }

    pub(crate) const fn minimum_width(self) -> f32 {
        match self {
            Self::Run | Self::Project => 140.,
            Self::Evidence => 120.,
            Self::LastValue | Self::Minimum | Self::Maximum | Self::Locked | Self::Hover => 100.,
            Self::Count | Self::LastStep | Self::Status => 80.,
        }
    }

    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Run => 0,
            Self::LastValue => 1,
            Self::Minimum => 2,
            Self::Maximum => 3,
            Self::Locked => 4,
            Self::Hover => 5,
            Self::Count => 6,
            Self::LastStep => 7,
            Self::Status => 8,
            Self::Evidence => 9,
            Self::Project => 10,
        }
    }

    pub(crate) const fn right_aligned(self) -> bool {
        matches!(
            self,
            Self::LastValue
                | Self::Minimum
                | Self::Maximum
                | Self::Locked
                | Self::Hover
                | Self::Count
                | Self::LastStep
        )
    }

    pub(crate) const fn owns_resize_boundary(self) -> bool {
        !matches!(self, Self::Project)
    }
}

impl InspectorSortDirection {
    pub(crate) const fn apply(self, ordering: Ordering) -> Ordering {
        match self {
            Self::Ascending => ordering,
            Self::Descending => ordering.reverse(),
        }
    }
}
