//! `DDR → SM5` conversions with `--auto-sync`.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{auto_sync_tail, convert, synthetic_song, tail_number, write_sm5_input, TestResult};
use ddr_chart_tools::model::Rational;
use ddr_chart_tools::ssc;

/// A DDR `sync.ssq` + `sync.xwb` made by the tool from a synthetic song
/// whose audio is 20 ms late.
fn ddr_input(root: &Path) -> TestResult<(PathBuf, PathBuf)> {
    let (chart, audio) = write_sm5_input(&root.join("src"), "sync", &synthetic_song(None)?, 20.0)?;
    let ddr = root.join("ddr");
    convert("SM5", "DDR", &chart, &audio, &ddr, &[])?;
    Ok((ddr.join("sync.ssq"), ddr.join("sync.xwb")))
}

#[test]
fn auto_sync_converges() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (chart, audio) = ddr_input(dir.path())?;
    let synced = dir.path().join("synced");
    let logs = convert("DDR", "SM5", &chart, &audio, &synced, &["--auto-sync"])?;
    assert_eq!(
        auto_sync_tail(&logs)?.get("auto_sync").map(String::as_str),
        Some("apply")
    );

    let recheck = dir.path().join("recheck");
    let logs = convert(
        "SM5",
        "DDR",
        &synced.join("sync.ssc"),
        &synced.join("sync.ogg"),
        &recheck,
        &["--auto-sync", "report"],
    )?;
    let tail = auto_sync_tail(&logs)?;
    let delta = tail_number(&tail, "delta_ms")?;
    assert!(delta.abs() <= 1.0, "second pass wants {delta} ms: {tail:?}");
    Ok(())
}

#[test]
fn report_mode_writes_the_same_chart() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (chart, audio) = ddr_input(dir.path())?;
    let plain = dir.path().join("plain");
    let report = dir.path().join("report");
    convert("DDR", "SM5", &chart, &audio, &plain, &[])?;
    convert(
        "DDR",
        "SM5",
        &chart,
        &audio,
        &report,
        &["--auto-sync", "report"],
    )?;
    assert_eq!(
        fs::read(plain.join("sync.ssc"))?,
        fs::read(report.join("sync.ssc"))?
    );
    Ok(())
}

#[test]
fn bias_composes_with_auto_sync() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (chart, audio) = ddr_input(dir.path())?;
    let synced = dir.path().join("synced");
    let biased = dir.path().join("biased");
    convert("DDR", "SM5", &chart, &audio, &synced, &["--auto-sync"])?;
    convert(
        "DDR",
        "SM5",
        &chart,
        &audio,
        &biased,
        &["--auto-sync", "--sync-offset-ms", "10"],
    )?;
    let a = ssc::parse(&fs::read_to_string(synced.join("sync.ssc"))?)?;
    let b = ssc::parse(&fs::read_to_string(biased.join("sync.ssc"))?)?;
    let expected = a.audio_sync_offset_seconds.add(&Rational::new(10, 1000)?)?;
    assert_eq!(
        b.audio_sync_offset_seconds, expected,
        "#OFFSET 0.010 s lower"
    );
    assert_eq!(a.tempo_segments, b.tempo_segments);
    Ok(())
}
