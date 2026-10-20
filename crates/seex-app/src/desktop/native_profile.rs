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
                    let painted = active.panels[viewport.clone()].iter().filter(|p| workspace.track_charts.get(&p.panel_id).is_some_and(|c| c.read(cx).painted_current())).count();
                    let loaded = active.panels[viewport.clone()].iter().filter(|p| p.overview.is_some()).count();
                    state.status = format!("visible={viewport:?} loaded={loaded} painted={painted} depths={:?}", viewer.session.read(cx).sources.queue_depths());
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
            state.frames.sort_by(f64::total_cmp);
            let n = state.frames.len();
            println!("SEEX_NATIVE_PHASE stage={} elapsed_ms={} frames={n} callback_p95_ms={} callback_max_ms={} motion_steps={} unready_motion_frames={}", state.stage, state.since.elapsed().as_millis(), state.frames[(n * 95).div_ceil(100).saturating_sub(1)], state.frames[n - 1], state.motion_steps, state.unready_frames);
            state.frames.clear();
            state.stage += 1;
            state.since = Instant::now();
            state.ready_frames = 0;
            state.motion_steps = 0;
            state.unready_frames = 0;
        }
        let timed_out = state.since.elapsed() > Duration::from_secs(20);
        if timed_out || state.stage == 9 {
            println!("SEEX_NATIVE_RESULT ok={} stage={} status={}", !timed_out, state.stage, state.status);
            cx.quit();
        } else {
            schedule(handle, state, window);
        }
    });
}
