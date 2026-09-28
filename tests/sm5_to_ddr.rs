//! `SM5 → DDR` conversions with `--auto-sync`.

mod common;

use std::fs;

use common::{auto_sync_tail, convert, synthetic_song, tail_number, write_sm5_input, TestResult};
use ddr_chart_tools::ssq;

#[test]
fn auto_sync_converges() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (chart, audio) =
        write_sm5_input(&dir.path().join("in"), "sync", &synthetic_song(None)?, 20.0)?;
    let synced = dir.path().join("synced");
    let logs = convert("SM5", "DDR", &chart, &audio, &synced, &["--auto-sync"])?;
    let first = auto_sync_tail(&logs)?;
    assert_eq!(
        first.get("auto_sync").map(String::as_str),
        Some("apply"),
        "{first:?}"
    );

    // Re-measuring the synced output finds nothing left to correct.
    let recheck = dir.path().join("recheck");
    let logs = convert(
        "DDR",
        "SM5",
        &synced.join("sync.ssq"),
        &synced.join("sync.xwb"),
        &recheck,
        &["--auto-sync", "report"],
    )?;
    let second = auto_sync_tail(&logs)?;
    let delta = tail_number(&second, "delta_ms")?;
    assert!(
        delta.abs() <= 1.0,
        "second pass wants {delta} ms: {second:?}"
    );
    Ok(())
}

#[test]
fn auto_sync_is_linear_in_audio_shift() -> TestResult {
    let dir = tempfile::tempdir()?;
    let song = synthetic_song(None)?;
    let mut corrections = Vec::new();
    for (name, shift) in [("early", 0.0), ("late", 25.0)] {
        let (chart, audio) = write_sm5_input(&dir.path().join(name), "sync", &song, shift)?;
        let logs = convert(
            "SM5",
            "DDR",
            &chart,
            &audio,
            &dir.path().join(format!("{name}-out")),
            &["--auto-sync", "report"],
        )?;
        corrections.push(tail_number(&auto_sync_tail(&logs)?, "correction_ms")?);
    }
    let diff = corrections[1] - corrections[0];
    assert!(
        (diff - 25.0).abs() <= 1.0,
        "25 ms later audio should ask for 25 ms later chart, got {diff}"
    );
    Ok(())
}

#[test]
fn report_mode_writes_the_same_chart() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (chart, audio) =
        write_sm5_input(&dir.path().join("in"), "sync", &synthetic_song(None)?, 20.0)?;
    let plain = dir.path().join("plain");
    let report = dir.path().join("report");
    convert("SM5", "DDR", &chart, &audio, &plain, &[])?;
    let logs = convert(
        "SM5",
        "DDR",
        &chart,
        &audio,
        &report,
        &["--auto-sync", "report"],
    )?;
    assert_eq!(
        auto_sync_tail(&logs)?.get("auto_sync").map(String::as_str),
        Some("report")
    );
    assert_eq!(
        fs::read(plain.join("sync.ssq"))?,
        fs::read(report.join("sync.ssq"))?
    );
    Ok(())
}

#[test]
fn bias_composes_with_auto_sync() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (chart, audio) =
        write_sm5_input(&dir.path().join("in"), "sync", &synthetic_song(None)?, 20.0)?;
    let synced = dir.path().join("synced");
    let biased = dir.path().join("biased");
    convert("SM5", "DDR", &chart, &audio, &synced, &["--auto-sync"])?;
    convert(
        "SM5",
        "DDR",
        &chart,
        &audio,
        &biased,
        &["--auto-sync", "--sync-offset-ms", "10"],
    )?;
    let a = ssq::parse(&fs::read(synced.join("sync.ssq"))?)?.raw_tempo_pairs;
    let b = ssq::parse(&fs::read(biased.join("sync.ssq"))?)?.raw_tempo_pairs;
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x.0, y.0);
        assert_eq!(y.1 - x.1, 10, "bias moves every anchor on top of auto-sync");
    }
    Ok(())
}

#[test]
fn refusal_keeps_sync_and_succeeds() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (chart, audio) = write_sm5_input(
        &dir.path().join("in"),
        "sync",
        &synthetic_song(Some(10))?,
        20.0,
    )?;
    let plain = dir.path().join("plain");
    let synced = dir.path().join("synced");
    convert("SM5", "DDR", &chart, &audio, &plain, &[])?;
    let logs = convert("SM5", "DDR", &chart, &audio, &synced, &["--auto-sync"])?;
    let tail = auto_sync_tail(&logs)?;
    assert_eq!(tail.get("auto_sync").map(String::as_str), Some("refused"));
    assert_eq!(
        tail.get("reason").map(String::as_str),
        Some("too_few_events")
    );
    assert!(logs.contains("WARN"), "refusals are warnings:\n{logs}");
    assert_eq!(
        fs::read(plain.join("sync.ssq"))?,
        fs::read(synced.join("sync.ssq"))?
    );
    Ok(())
}
