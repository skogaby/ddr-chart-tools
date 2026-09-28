//! Calibration of `sync::TARGET_OFFSET_MS` against stock DDR World.
//!
//! Unlike the `{from}_to_{to}` integration tests, this validates a
//! constant rather than a conversion. It needs local data that is not in
//! the repository, so it is `#[ignore]`d and skips when the data is not
//! configured:
//!
//! - `DDR_WORLD_INSTALL`: a DDR World install whose `data/` holds
//!   `mdb_apx/ssq/<code>.ssq` and `sound/win/dance/<code>.xwb`.
//! - `DDR_SYNC_OFFSETS_CSV`: a community per-song offset CSV
//!   (`code,p1_offset,p2_offset`), whose value `c` is the play-validated
//!   amount to add to a song's `#OFFSET`, in ms.
//!
//! With `m` the estimator's measured offset, an in-sync chart measures
//! `T − c`, so the target is `T = median(m + c)`. Only TPS 1000 charts
//! are used: DDR World rounds every tempo anchor to whole milliseconds,
//! which only exact TPS 1000 timing reproduces.
//!
//! Run (about 3 minutes):
//!
//! ```text
//! cargo test --release --test auto_sync_calibration -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use ddr_chart_tools::sync::{self, Outcome, Params, TimeMap, TARGET_OFFSET_MS};
use ddr_chart_tools::{ssq, xwb};

/// Largest tolerated difference between the measured and shipped target.
const TARGET_TOLERANCE_MS: f64 = 0.1;
/// Required share of songs within 1 ms / 2 ms of the community value.
const MIN_WITHIN_1_MS: f64 = 0.90;
const MIN_WITHIN_2_MS: f64 = 0.96;
/// Required share of songs the estimator accepts (applies or leaves
/// unchanged) rather than refuses.
const MIN_ACCEPTED: f64 = 0.93;
/// Fewest measured songs for the result to mean anything.
const MIN_SONGS: usize = 500;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// One measured stock song.
struct Measured {
    code: String,
    community_ms: f64,
    measured_ms: f64,
    outcome: Outcome,
}

#[derive(Default)]
struct Skipped {
    missing: usize,
    not_tps_1000: usize,
    unreadable: usize,
    not_measured: usize,
}

#[test]
#[ignore = "needs a local DDR World install and community offset CSV"]
fn calibrate_target_offset_against_stock_catalogue() -> TestResult {
    let (Some(install), Some(csv)) = (
        env::var_os("DDR_WORLD_INSTALL"),
        env::var_os("DDR_SYNC_OFFSETS_CSV"),
    ) else {
        println!("skipping: set DDR_WORLD_INSTALL and DDR_SYNC_OFFSETS_CSV to calibrate");
        return Ok(());
    };
    let data = PathBuf::from(install).join("data");
    let offsets = community_offsets(&fs::read_to_string(csv)?);

    let mut songs = Vec::new();
    let mut skipped = Skipped::default();
    for (code, community_ms) in offsets {
        match measure(&data, &code) {
            Ok(Some((measured_ms, outcome))) => songs.push(Measured {
                code,
                community_ms,
                measured_ms,
                outcome,
            }),
            Ok(None) => skipped.not_measured += 1,
            Err(Skip::Missing) => skipped.missing += 1,
            Err(Skip::NotTps1000) => skipped.not_tps_1000 += 1,
            Err(Skip::Unreadable(why)) => {
                println!("  skip {code}: {why}");
                skipped.unreadable += 1;
            }
        }
    }

    let target = median(
        songs
            .iter()
            .map(|s| s.measured_ms + s.community_ms)
            .collect(),
    );
    let residual = |s: &Measured| s.measured_ms + s.community_ms - target;
    let share = |pred: &dyn Fn(&Measured) -> bool| {
        songs.iter().filter(|s| pred(s)).count() as f64 / songs.len().max(1) as f64
    };
    let within = |ms: f64| share(&|s| residual(s).abs() <= ms);
    let accepted = share(&|s| matches!(s.outcome, Outcome::Apply { .. } | Outcome::Unchanged));

    println!("\nauto-sync calibration (TPS 1000 stock songs)");
    println!(
        "  measured {}; skipped: {} missing, {} not TPS 1000, {} unreadable, {} not measured",
        songs.len(),
        skipped.missing,
        skipped.not_tps_1000,
        skipped.unreadable,
        skipped.not_measured
    );
    println!("  T = median(m + c) = {target:+.3} ms (shipped {TARGET_OFFSET_MS:+.2} ms)");
    println!(
        "  |m + c - T| within 0.5 / 1 / 2 / 3 ms: {:.1}% / {:.1}% / {:.1}% / {:.1}%",
        100.0 * within(0.5),
        100.0 * within(1.0),
        100.0 * within(2.0),
        100.0 * within(3.0)
    );
    let mut outcomes: BTreeMap<String, usize> = BTreeMap::new();
    for s in &songs {
        let key = match s.outcome {
            Outcome::Apply { .. } => "apply".to_string(),
            Outcome::Unchanged => "unchanged".to_string(),
            Outcome::Refused(r) => format!("refused {r:?}"),
        };
        *outcomes.entry(key).or_insert(0) += 1;
    }
    println!("  accepted {:.1}%: {outcomes:?}", 100.0 * accepted);
    let mut worst: Vec<&Measured> = songs.iter().collect();
    worst.sort_by(|a, b| residual(b).abs().total_cmp(&residual(a).abs()));
    println!("  largest disagreements (our correction vs community, ms):");
    for s in worst.iter().take(10) {
        println!(
            "    {:8} ours {:+7.2}  community {:+4.0}  diff {:+7.2}  {:?}",
            s.code,
            s.measured_ms - target,
            -s.community_ms,
            residual(s),
            s.outcome
        );
    }

    assert!(
        songs.len() >= MIN_SONGS,
        "only {} songs measured; is the install path right?",
        songs.len()
    );
    assert!(
        (target - TARGET_OFFSET_MS).abs() <= TARGET_TOLERANCE_MS,
        "TARGET_OFFSET_MS is stale: set it to {target:.2}"
    );
    assert!(
        within(1.0) >= MIN_WITHIN_1_MS && within(2.0) >= MIN_WITHIN_2_MS,
        "agreement regressed: {:.1}% within 1 ms, {:.1}% within 2 ms",
        100.0 * within(1.0),
        100.0 * within(2.0)
    );
    assert!(
        accepted >= MIN_ACCEPTED,
        "acceptance regressed: {:.1}%",
        100.0 * accepted
    );
    Ok(())
}

enum Skip {
    Missing,
    NotTps1000,
    Unreadable(String),
}

/// Measure one stock song: `Ok(None)` when the estimator refused before
/// measuring.
fn measure(data: &Path, code: &str) -> Result<Option<(f64, Outcome)>, Skip> {
    let ssq_path = data.join("mdb_apx/ssq").join(format!("{code}.ssq"));
    let xwb_path = data.join("sound/win/dance").join(format!("{code}.xwb"));
    if !ssq_path.is_file() || !xwb_path.is_file() {
        return Err(Skip::Missing);
    }
    let unreadable = |e: &dyn std::fmt::Display| Skip::Unreadable(e.to_string());
    let chart = fs::read(&ssq_path).map_err(|e| unreadable(&e))?;
    let parsed = ssq::parse(&chart).map_err(|e| unreadable(&e))?;
    if parsed.song.tps != 1000 {
        return Err(Skip::NotTps1000);
    }
    let bank = fs::read(&xwb_path).map_err(|e| unreadable(&e))?;
    let audio = xwb::parse_audio(&bank).map_err(|e| unreadable(&e))?;
    let map = TimeMap::from_tempo_pairs(&parsed.raw_tempo_pairs, parsed.song.tps)
        .ok_or_else(|| Skip::Unreadable("unusable tempo chunk".to_string()))?;
    let events = sync::chart_events(&parsed.song.charts, &map);
    let est = sync::estimate(&audio, &events, &Params::default());
    Ok(est.measured_ms.map(|m| (m, est.outcome)))
}

/// `code → c` for every CSV row with a P1 value.
fn community_offsets(csv: &str) -> Vec<(String, f64)> {
    csv.lines()
        .skip(1)
        .filter_map(|line| {
            let mut cols = line.split(',').map(str::trim);
            let code = cols.next()?;
            let value: f64 = cols.next()?.parse().ok()?;
            (!code.is_empty()).then(|| (code.to_string(), value))
        })
        .collect()
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    match n {
        0 => f64::NAN,
        _ if n % 2 == 1 => values[n / 2],
        _ => 0.5 * (values[n / 2 - 1] + values[n / 2]),
    }
}
