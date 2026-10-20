//! Explicit opt-in native-window diagnostic. No TestPlatform or user configuration.
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::*;
use gpui::{
    App, AppContext, Application, Bounds, ListOffset, Window, WindowBounds, WindowHandle,
    WindowOptions, px, size,
};

pub(in crate::desktop) fn run(root: PathBuf) {
    assert!(!cfg!(debug_assertions), "native profile requires --release");
    let metric_count = std::env::var("SEEX_NATIVE_PROFILE_METRICS")
        .expect("metric count required")
        .parse::<usize>()
        .unwrap();
    let home = PathBuf::from(
        std::env::var_os("SEEX_NATIVE_PROFILE_HOME").expect("isolated profile home required"),
    );
    assert!(
        home.starts_with(&root),
        "profile home must be inside fixture root"
    );
    Application::new().with_assets(ViewerAssets).run(move |cx| {
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(1200.), px(800.)),
                        cx,
                    ))),
                    ..WindowOptions::default()
                },
                |window, cx| {
                    cx.new(|cx| {
                        ViewerApp::new_with_home(Some(root.clone()), Some(&home), window, cx)
                    })
                },
            )
            .unwrap();
        let reader = seex::Reader::builder(&root).open_local().unwrap();
        let projects = reader.projects().unwrap();
        let project_id = projects[0].project_id.clone();
        let runs = reader.runs(&project_id).unwrap();
        let run_id = runs[0].run_id.clone();
        let source = ConfiguredSource {
            alias: SourceAlias::new("native-profile").unwrap(),
            root_path: root,
            projects: vec![project_id.clone()],
        };
        handle
            .update(cx, |viewer, window, cx| {
                viewer.open_configured_sources(vec![source], cx);
                schedule(
                    handle,
                    State {
                        project_id,
                        run_id,
                        metric_count,
                        stage: 0,
                        since: Instant::now(),
                        last: Instant::now(),
                        frames: Vec::new(),
                        ready_frames: 0,
                        motion_steps: 0,
                        unready_frames: 0,
                        status: String::new(),
                        reported_unready: false,
                        invalid_evidence: false,
                        traces: Vec::new(),
                    },
                    window,
                );
            })
            .unwrap();
        cx.activate(true);
    });
}

struct State {
    metric_count: usize,
    project_id: ProjectId,
    run_id: seex::RunId,
    stage: usize,
    since: Instant,
    last: Instant,
    frames: Vec<f64>,
    ready_frames: usize,
    motion_steps: usize,
    unready_frames: usize,
    status: String,
    reported_unready: bool,
    invalid_evidence: bool,
    traces: Vec<MotionTrace>,
}

struct MotionTrace {
    target: usize,
    started: Instant,
    // First frame observing visible, pending, complete data, and current paint.
    observed: [Option<Duration>; 4],
    done: bool,
}
impl MotionTrace {
    fn observe(&mut self, states: [bool; 4], elapsed: Duration) {
        if self.done {
            return;
        }
        for (i, ready) in states.into_iter().enumerate() {
            if ready && self.observed[i].is_none() {
                self.observed[i] = Some(elapsed);
            }
        }
    }
    fn finish(&mut self, reason: &str) {
        if self.done {
            return;
        }
        self.done = true;
        println!(
            "SEEX_MOTION_TRACE target={} reason={reason} observed_ms={:?} age_ms={}",
            self.target,
            self.observed.map(|v| v.map(|d| d.as_secs_f64() * 1000.)),
            self.started.elapsed().as_millis()
        );
    }
}

#[cfg(test)]
#[test]
fn motion_trace_preserves_missing_events_and_first_observations() {
    let mut trace = MotionTrace {
        target: 4,
        started: Instant::now(),
        observed: [None; 4],
        done: false,
    };
    trace.observe([true, true, false, false], Duration::from_millis(16));
    trace.observe([true, true, false, false], Duration::from_millis(32));
    trace.finish("left_viewport");
    trace.observe([true; 4], Duration::from_millis(48));
    assert_eq!(
        trace.observed,
        [
            Some(Duration::from_millis(16)),
            Some(Duration::from_millis(16)),
            None,
            None
        ]
    );
    let mut cached = MotionTrace {
        target: 0,
        started: Instant::now(),
        observed: [None; 4],
        done: false,
    };
    cached.observe([true, false, true, true], Duration::from_millis(16));
    assert!(
        cached.observed[1].is_none(),
        "must not fabricate a pending time for cached data"
    );
}

fn report_unready(viewer: &ViewerApp, cx: &App) {
    let snapshot = viewer.session_snapshot(cx);
    let active = snapshot.views.active();
    let workspace = viewer.workspace.read(cx);
    let visible = workspace.track_viewport.borrow().visible.clone();
    println!(
        "SEEX_NATIVE_DIAGNOSTIC list_bounds={:?}",
        workspace.metric_scroll.viewport_bounds()
    );
    for index in visible {
        let panel = &active.panels[index];
        let chart = workspace.track_charts.get(&panel.panel_id);
        if chart.is_some_and(|chart| chart.read(cx).painted_current()) {
            continue;
        }
        let points = panel.overview.as_ref().map(|s| {
            s.series
                .iter()
                .map(|c| {
                    (
                        c.run_ref.cache_key(),
                        c.completeness,
                        c.reasons.clone(),
                        c.chart_series.as_ref().map(|s| s.points().len()),
                    )
                })
                .collect::<Vec<_>>()
        });
        println!(
            "SEEX_NATIVE_UNREADY index={index} panel={} metric={} row_bounds={:?} points={points:?} detail_rev={} overview_rev={} chart={}",
            panel.panel_id,
            panel.metric_key.as_str(),
            workspace.metric_scroll.bounds_for_item(index),
            panel.detail_revision,
            panel.overview_revision,
            chart
                .map(|c| c.read(cx).paint_diagnostic())
                .unwrap_or_else(|| "missing".to_owned())
        );
    }
}

#[cfg(test)]
#[test]
fn native_fixture_keeps_dense_and_sparse_metric_timestamps_step_ordered() {
    let root = tempfile::tempdir().unwrap();
    let client = seex::Client::builder(root.path()).open().unwrap();
    let run = client
        .start_run(seex::RunOptions::new("fixture-order").id("run"))
        .unwrap();
    run.log_with(
        (0..4).map(|i| (format!("metric-{i:04}"), 1.)),
        seex::LogOptions::new().step(0),
    )
    .unwrap();
    for step in 1..4096 {
        run.log_with(
            [("metric-0000", (step as f64).sin())],
            seex::LogOptions::new().step(step),
        )
        .unwrap();
    }
    // Add sparse endpoints last, excluding the already-complete dense series.
    run.log_with(
        (1..4).map(|i| (format!("metric-{i:04}"), 2.)),
        seex::LogOptions::new().step(100),
    )
    .unwrap();
    run.finish().unwrap();
    client.shutdown().unwrap();
    let reader = seex::Reader::builder(root.path()).open_local().unwrap();
    let query =
        seex::MetricQuery::new(seex::MetricRange::All(seex::MetricAxis::Step), 1024).unwrap();
    for i in 0..4 {
        let series = reader
            .query_metric_for_desktop(
                run.run_id(),
                &seex::MetricKey::from_string(format!("metric-{i:04}")),
                &query,
            )
            .unwrap();
        assert_eq!(
            series.completeness(),
            seex::EvidenceCompleteness::Complete,
            "{:?}",
            series.reasons()
        );
        assert_eq!(series.source_count(), if i == 0 { 4096 } else { 2 });
        assert!(series.samples().len() >= 2);
    }
}

fn schedule(handle: WindowHandle<ViewerApp>, mut state: State, window: &mut Window) {
    window.refresh();
    window.on_next_frame(move |window, cx: &mut App| {
        let now = Instant::now();
        state.frames.push(now.duration_since(state.last).as_secs_f64() * 1000.);
        state.last = now;
        let mut advance = false;
        window.root::<ViewerApp>().flatten().unwrap().update(cx, |viewer, cx| {
            let snapshot = viewer.session_snapshot(cx);
            match state.stage {
                0 if snapshot.sources.iter().any(|s| !s.catalog.projects.is_empty()) => {
                    let source = snapshot.sources[0].source_id.clone();
                    viewer.dispatch_workbench_command(command::WorkbenchCommand::ToggleRun(RunRef::new(source, state.project_id.clone(), state.run_id.clone())), cx);
                    advance = true;
                }
                1 if viewer.available_metric_keys(cx).len() == state.metric_count => {
                    viewer.dispatch_workbench_command(command::WorkbenchCommand::SelectMetrics(viewer.available_metric_keys(cx)), cx);
                    advance = true;
                }
                2..=5 | 8 => {
                    let active = snapshot.views.active();
                    let viewport = viewer.workspace.read(cx).track_viewport.borrow().visible.clone();
                    let workspace = viewer.workspace.read(cx);
                    let expected = match state.stage { 3 => active.panels.len() - 1, 4 => active.panels.len() / 2, _ => 0 };
                    let ready = viewport.contains(&expected) && !viewport.is_empty() && active.panels[viewport.clone()].iter().all(|p| {
                        p.source_errors.is_empty() && p.overview.as_ref().is_some_and(|s| active.runs.iter().all(|run| {
                            s.series.iter().any(|c| &c.run_ref == run && c.completeness == seex::EvidenceCompleteness::Complete && c.chart_series.as_ref().is_some_and(|s| s.points().len() >= 2))
                        })) && workspace.track_charts.get(&p.panel_id).is_some_and(|c| c.read(cx).painted_current())
                    }) && !active.panels.iter().any(|p| p.is_pending(crate::data::worker::ReadKind::Overview) || p.is_pending(crate::data::worker::ReadKind::Detail))
                        && viewer.session.read(cx).sources.queue_depths() == [0; 3];
                    state.invalid_evidence = active.panels[viewport.clone()].iter().filter_map(|p| p.overview.as_ref())
                        .any(|s| s.series.iter().any(|c| c.completeness == seex::EvidenceCompleteness::Invalid));
                    let painted = active.panels[viewport.clone()].iter().filter(|p| workspace.track_charts.get(&p.panel_id).is_some_and(|c| c.read(cx).painted_current())).count();
                    let loaded = active.panels[viewport.clone()].iter().filter(|p| p.overview.is_some()).count();
                    state.status = format!("visible={viewport:?} loaded={loaded} painted={painted} depths={:?}", viewer.session.read(cx).sources.queue_depths());
                    if loaded == viewport.len() && painted < loaded && !state.reported_unready {
                        state.reported_unready = true;
                        report_unready(viewer, cx);
                    }
                    state.ready_frames = if ready { state.ready_frames + 1 } else { 0 };
                    if state.ready_frames >= 2 {
                        let n = active.panels.len();
                        let target = match state.stage { 2 => n - 1, 3 => n / 2, _ => 0 };
                        if state.stage > 2 {
                            let previous = match state.stage { 3 => n - 1, 4 => n / 2, _ => 0 };
                            if !viewport.contains(&previous) { return; }
                        }
                        println!("SEEX_NATIVE stage={} panels={n} visible={viewport:?} elapsed_ms={}", state.stage, state.since.elapsed().as_millis());
                        if state.stage < 5 {
                            viewer.workspace.update(cx, |workspace, cx| {
                                workspace.metric_scroll.scroll_to(ListOffset { item_ix: target, offset_in_item: px(0.) });
                                cx.notify();
                            });
                        }
                        advance = true;
                    }
                }
                6 | 7 => {
                    let workspace = viewer.workspace.read(cx);
                    let viewport = workspace.track_viewport.borrow().visible.clone();
                    for trace in state.traces.iter_mut().filter(|t| !t.done) {
                        if !viewport.contains(&trace.target) {
                            if trace.observed[0].is_some() { trace.finish("left_viewport"); }
                            continue;
                        }
                        let panel = &snapshot.views.active().panels[trace.target];
                        let pending = panel.is_pending(crate::data::worker::ReadKind::Overview);
                        let loaded = panel.source_errors.is_empty() && panel.overview.as_ref().is_some_and(|s|
                            snapshot.views.active().runs.iter().all(|r| s.series.iter().any(|c|
                                &c.run_ref == r && c.completeness == seex::EvidenceCompleteness::Complete
                                && c.chart_series.as_ref().is_some_and(|s| s.points().len() >= 2))));
                        let painted = workspace.track_charts.get(&panel.panel_id).is_some_and(|c| c.read(cx).painted_current());
                        trace.observe([true, pending, loaded, painted], trace.started.elapsed());
                        if loaded && painted { trace.finish("painted"); }
                    }
                    let painted = !viewport.is_empty() && snapshot.views.active().panels[viewport].iter().all(|p|
                        workspace.track_charts.get(&p.panel_id).is_some_and(|c| c.read(cx).painted_current()));
                    state.unready_frames += usize::from(!painted);
                    let elapsed = state.since.elapsed();
                    let n = snapshot.views.active().panels.len();
                    if elapsed >= Duration::from_secs(3) {
                        advance = true;
                    } else if state.frames.len() % 3 == 0 {
                        state.motion_steps += 1;
                        let forward = state.motion_steps * 4 % n.saturating_sub(14).max(1);
                        let target = if state.stage == 6 || state.motion_steps % 2 == 0 { forward } else { n - 1 - forward };
                        state.traces.push(MotionTrace { target, started: Instant::now(), observed: [None; 4], done: false });
                        viewer.workspace.update(cx, |workspace, cx| {
                            workspace.metric_scroll.scroll_to(ListOffset { item_ix: target, offset_in_item: px(0.) });
                            cx.notify();
                        });
                    }
                    if advance && state.stage == 7 {
                        viewer.workspace.update(cx, |workspace, cx| { workspace.metric_scroll.scroll_to(ListOffset::default()); cx.notify(); });
                    }
                }
                _ => {}
            }
        });
        if advance {
            if matches!(state.stage, 6 | 7) {
                for trace in &mut state.traces { trace.finish("stage_end"); }
                let counts: [usize; 4] = std::array::from_fn(|i| state.traces.iter().filter(|t| t.observed[i].is_some()).count());
                println!("SEEX_MOTION_SUMMARY stage={} targets={} observed_visible_pending_data_paint={counts:?}", state.stage, state.traces.len());
                state.traces.clear();
            }
            state.frames.sort_by(f64::total_cmp);
            let n = state.frames.len();
            println!("SEEX_NATIVE_PHASE stage={} elapsed_ms={} frames={n} callback_p95_ms={} callback_max_ms={} motion_steps={} unready_motion_frames={}", state.stage, state.since.elapsed().as_millis(), state.frames[(n * 95).div_ceil(100).saturating_sub(1)], state.frames[n - 1], state.motion_steps, state.unready_frames);
            state.frames.clear();
            state.stage += 1;
            state.since = Instant::now();
            state.ready_frames = 0;
            state.motion_steps = 0;
            state.unready_frames = 0;
            state.reported_unready = false;
        }
        let timed_out = state.since.elapsed() > Duration::from_secs(20) || state.invalid_evidence;
        if timed_out {
            window.root::<ViewerApp>().flatten().unwrap().read_with(cx, report_unready);
        }
        if timed_out || state.stage == 9 {
            println!("SEEX_NATIVE_RESULT ok={} stage={} status={}", !timed_out, state.stage, state.status);
            cx.quit();
        } else {
            schedule(handle, state, window);
        }
    });
}
