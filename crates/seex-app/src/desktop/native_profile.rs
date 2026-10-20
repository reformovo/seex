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
                2..=5 => {
                    let active = snapshot.views.active();
                    let viewport = viewer.workspace.read(cx).track_viewport.borrow().visible.clone();
                    let ready = !viewport.is_empty() && active.panels[viewport.clone()].iter().all(|p| p.overview.is_some())
                        && !active.panels.iter().any(|p| p.is_pending(crate::data::worker::ReadKind::Overview) || p.is_pending(crate::data::worker::ReadKind::Detail))
                        && viewer.session.read(cx).sources.queue_depths() == [0; 3];
                    if ready {
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
                _ => {}
            }
        });
        if advance {
            state.stage += 1;
            state.since = Instant::now();
        }
        let timed_out = state.since.elapsed() > Duration::from_secs(20);
        if timed_out || state.stage == 6 {
            state.frames.sort_by(f64::total_cmp);
            let n = state.frames.len();
            println!("SEEX_NATIVE_RESULT ok={} stage={} frames={n} callback_p95_ms={} callback_max_ms={}", !timed_out, state.stage, state.frames[(n * 95).div_ceil(100).saturating_sub(1)], state.frames[n - 1]);
            cx.quit();
        } else {
            schedule(handle, state, window);
        }
    });
}
