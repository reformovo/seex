//! Opt-in real panel scaling diagnostic; not an optimization gate.
use seex::{
    Client, EvidenceCompleteness, LogOptions, MAX_CHART_POINTS, MetricKey, ProjectId, RunId,
    RunOptions,
};
use seex_app::{
    data::{
        ReadSession,
        query::{CurveAxis, CurveSelection, DetailRequest, OverviewRequest},
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

fn expected_source_count(start: i64, end: i64) -> u64 {
    // Fixture has Steps 0..POINTS-1 plus a repeated midpoint. Desktop Detail is
    // viewport-only, so out-of-view neighbors are not counted.
    let inside = (i128::from(end.min(POINTS - 1)) - i128::from(start.max(0)) + 1).max(0) as u64;
    inside + u64::from(start <= POINTS / 2 && POINTS / 2 <= end)
}

fn detail_case(name: &str) -> Result<(i64, i64, u64), Box<dyn Error>> {
    let (start, end) = match name {
        "full" => (0, POINTS),
        "prefix" => (0, 2048),
        "middle" => (450_000, 550_000),
        "window" => (500_000, 502_048),
        "small" => (0, 511),
        "small-middle" => (500_000, 500_511),
        "tail" => (999_000, 1_100_000),
        "empty" => (2_000_000, 3_000_000),
        value => return Err(format!("unsupported panel detail range: {value}").into()),
    };
    Ok((start, end, expected_source_count(start, end)))
}

fn detail_range() -> Result<(i64, i64, u64), Box<dyn Error>> {
    detail_case(&env::var("SEEX_PANEL_DETAIL_RANGE").unwrap_or("middle".into()))
}

fn panel_curves() -> Result<usize, Box<dyn Error>> {
    let curves = env::var("SEEX_PANEL_CURVES")
        .unwrap_or("16".into())
        .parse()?;
    if !matches!(curves, 1 | 16) {
        return Err("panel curves must be 1 or 16".into());
    }
    Ok(curves)
}

fn is_detail(value: &str) -> Result<bool, Box<dyn Error>> {
    match value {
        "overview" => Ok(false),
        "detail" => Ok(true),
        _ => Err("panel range must be overview or detail".into()),
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
    assert_eq!(is_detail("overview").unwrap(), false);
    assert_eq!(is_detail("detail").unwrap(), true);
    assert!(is_detail("typo").is_err());
    for (case, count) in [
        ("full", 1_000_001),
        ("prefix", 2049),
        ("middle", 100_002),
        ("window", 2050),
        ("small", 512),
        ("small-middle", 513),
        ("tail", 1000),
        ("empty", 0),
    ] {
        assert_eq!(detail_case(case).unwrap().2, count, "{case}");
    }
    assert!(detail_case("typo").is_err());
    assert_eq!(expected_source_count(-100, -1), 0);
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
#[ignore = "release bounded panel performance gate; OS cache is not cleared"]
fn panel_gate() -> Result<(), Box<dyn Error>> {
    if cfg!(debug_assertions) {
        return Err("panel gate requires release".into());
    }
    let session = ReadSession::open_existing(&root()?)?;
    let detail = is_detail(&env::var("SEEX_PANEL_RANGE").unwrap_or("overview".into()))?;
    let (range_start, range_end, expected_source_count) = if detail {
        detail_range()?
    } else {
        (0, POINTS, (POINTS + 1) as u64)
    };
    let curves = panel_curves()?;
    let request = request(curves);
    let run = || -> Result<(), Box<dyn Error>> {
        let snapshot = if detail {
            session.query_detail(&DetailRequest {
                selection: request.selection.clone(),
                viewport: seex::AlignmentViewport::new(range_start, range_end)?,
                logical_width: 1024,
            })?
        } else {
            session.query_overview(&request)?
        };
        assert_eq!(snapshot.series.len(), curves);
        for curve in &snapshot.series {
            assert_eq!(curve.source_row_count, expected_source_count);
            assert_eq!(curve.completeness, EvidenceCompleteness::Complete);
            assert!(curve.reasons.is_empty());
            assert_eq!(
                curve.returned_point_count,
                expected_source_count.min(MAX_CHART_POINTS as u64)
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
        Ok(())
    };
    let first = Instant::now();
    run()?;
    println!(
        "\npanel_first_request,curves={curves},range=[{range_start},{range_end}],source_per_curve={expected_source_count},elapsed_ns={}",
        first.elapsed().as_nanos()
    );
    let mut iterations = 1_u64;
    loop {
        let started = Instant::now();
        for _ in 0..iterations {
            run()?;
        }
        if started.elapsed() >= Duration::from_millis(25) {
            break;
        }
        iterations = iterations
            .checked_mul(2)
            .ok_or("panel calibration overflow")?;
    }
    let mut samples = Vec::with_capacity(10);
    for _ in 0..10 {
        let started = Instant::now();
        for _ in 0..iterations {
            run()?;
        }
        let elapsed = started.elapsed();
        if elapsed < Duration::from_millis(25) {
            return Err("panel sample below 25ms".into());
        }
        samples.push(elapsed.as_nanos() as f64 / iterations as f64);
    }
    println!(
        "\nSEEX_BENCH {{\"schema_version\":3,\"record_type\":\"metric\",\"domain\":\"query\",\"metric\":\"panel.curves{curves}.{}\",\"unit\":\"ns/op\",\"direction\":\"lower\",\"batch_iterations\":{iterations},\"samples\":{samples:?}}}",
        if detail { "detail" } else { "overview" }
    );
    Ok(())
}

#[test]
#[ignore = "release full-request panel RSS/latency diagnostic; OS cache is not cleared"]
fn query_panel() -> Result<(), Box<dyn Error>> {
    if cfg!(debug_assertions) {
        return Err("panel diagnostic requires release".into());
    }
    let curves = panel_curves()?;
    let session = ReadSession::open_existing(&root()?)?;
    let request = request(curves);
    let detail = is_detail(&env::var("SEEX_PANEL_RANGE").unwrap_or("overview".into()))?;
    let (range_start, range_end, expected_source_count) = if detail {
        detail_range()?
    } else {
        (0, POINTS, (POINTS + 1) as u64)
    };
    for index in 0..4 {
        let started = Instant::now();
        let snapshot = if detail {
            session.query_detail(&DetailRequest {
                selection: request.selection.clone(),
                viewport: seex::AlignmentViewport::new(range_start, range_end)?,
                logical_width: 1024,
            })?
        } else {
            session.query_overview(&request)?
        };
        let elapsed = started.elapsed().as_nanos();
        assert_eq!(snapshot.series.len(), curves);
        for curve in &snapshot.series {
            assert_eq!(curve.source_row_count, expected_source_count);
            assert_eq!(curve.completeness, EvidenceCompleteness::Complete);
            assert!(curve.reasons.is_empty());
            assert_eq!(
                curve.returned_point_count,
                expected_source_count.min(MAX_CHART_POINTS as u64)
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
