//! Opt-in real panel scaling diagnostic; not an optimization gate.
use seex::{
    Client, EvidenceCompleteness, LogOptions, MAX_CHART_POINTS, MetricKey, ProjectId, RunId,
    RunOptions,
};
use seex_app::{
    data::{
        ReadSession,
        query::{CurveAxis, CurveSelection, OverviewRequest},
    },
    domain::{DataSourceId, RunRef},
};
use std::{
    env,
    error::Error,
    path::PathBuf,
    time::{Duration, Instant},
};

const POINTS: i64 = 1_000_000;
const CURVES: usize = 16;

fn root() -> Result<PathBuf, Box<dyn Error>> {
    let root = PathBuf::from(env::var("SEEX_PANEL_ROOT")?);
    if !root.ends_with(".tmp/v5-panel-million/store") {
        return Err("panel fixture must be .tmp/v5-panel-million/store".into());
    }
    Ok(root)
}

fn request(curves: usize) -> OverviewRequest {
    let source = DataSourceId::new("panel").expect("valid source id");
    OverviewRequest {
        selection: CurveSelection {
            source_id: source.clone(),
            runs: (0..curves)
                .map(|index| {
                    RunRef::new(
                        source.clone(),
                        ProjectId::from_string("panel"),
                        RunId::from_string(format!("run-{index}")),
                    )
                })
                .collect(),
            metric_key: MetricKey::from_string("loss"),
            axis: CurveAxis::Step,
        },
        logical_width: 1024,
    }
}

fn value(state: &mut u64) -> f64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state >> 11) as f64 / (1_u64 << 53) as f64
}

#[test]
fn panel_contract_and_values() {
    assert_eq!(request(16).selection.runs.len(), 16);
    let mut state = 0x5eec_u64;
    for _ in 0..1000 {
        assert!((0.0..1.0).contains(&value(&mut state)));
    }
}

#[test]
#[ignore = "prepare sixteen million-observation curves outside query measurement"]
fn prepare_panel() -> Result<(), Box<dyn Error>> {
    let root = root()?;
    if root.exists() {
        return Err("refusing to overwrite panel fixture".into());
    }
    let client = Client::builder(&root).open()?;
    let started = Instant::now();
    for index in 0..CURVES {
        let run = client.start_run(RunOptions::new("panel").id(format!("run-{index}")))?;
        let mut state = 0x5eec_u64 + index as u64;
        for step in 0..POINTS {
            run.log_with([("loss", value(&mut state))], LogOptions::new().step(step))?;
            if step == POINTS / 2 {
                run.log_with([("loss", value(&mut state))], LogOptions::new().step(step))?;
            }
            if (step + 1) % 32768 == 0 {
                let deadline = Instant::now() + Duration::from_secs(30);
                while client.diagnostics().pending_reports != 0 {
                    if Instant::now() >= deadline {
                        return Err("panel fixture drain timeout".into());
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
        run.finish()?;
        println!("prepared_curve={index},observations={}", POINTS + 1);
    }
    println!("panel_prepare_ns={}", started.elapsed().as_nanos());
    Ok(())
}

#[test]
#[ignore = "release full-request panel RSS/latency diagnostic; OS cache is not cleared"]
fn query_panel() -> Result<(), Box<dyn Error>> {
    if cfg!(debug_assertions) {
        return Err("panel diagnostic requires release".into());
    }
    let curves: usize = env::var("SEEX_PANEL_CURVES")?.parse()?;
    if !matches!(curves, 1 | 16) {
        return Err("panel curves must be 1 or 16".into());
    }
    let session = ReadSession::open_existing(&root()?)?;
    let request = request(curves);
    for index in 0..4 {
        let started = Instant::now();
        let snapshot = session.query_overview(&request)?;
        let elapsed = started.elapsed().as_nanos();
        assert_eq!(snapshot.series.len(), curves);
        for curve in &snapshot.series {
            assert_eq!(curve.source_row_count, (POINTS + 1) as u64);
            assert_eq!(curve.completeness, EvidenceCompleteness::Complete);
            assert!(curve.reasons.is_empty());
            assert!(
                curve.returned_point_count > 0
                    && curve.returned_point_count <= MAX_CHART_POINTS as u64
            );
            assert_eq!(
                curve
                    .chart_series
                    .as_ref()
                    .expect("Complete curve must be drawable")
                    .points()
                    .len() as u64,
                curve.returned_point_count
            );
        }
        println!(
            "panel_request,curves={curves},cache={},elapsed_ns={elapsed},source_count={},points={}",
            if index == 0 {
                "diagnostics_miss"
            } else {
                "diagnostics_hit"
            },
            snapshot
                .series
                .iter()
                .map(|s| s.source_row_count)
                .sum::<u64>(),
            snapshot
                .series
                .iter()
                .map(|s| s.returned_point_count)
                .sum::<u64>()
        );
    }
    Ok(())
}
