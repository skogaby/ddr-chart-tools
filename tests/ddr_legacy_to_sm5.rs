//! `DDR_LEGACY → SM5` conversions with `--auto-sync`.

mod common;

use std::fs;

use common::{
    auto_sync_tail, convert, synthetic_song, tail_number, write_legacy_wavm_input, TestResult,
};
use ddr_chart_tools::model::Rational;
use ddr_chart_tools::ssc;

#[test]
fn auto_sync_converges_on_wavm_audio() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (chart, audio) =
        write_legacy_wavm_input(&dir.path().join("in"), "sync", &synthetic_song(None)?, 20.0)?;
    let synced = dir.path().join("synced");
    let logs = convert(
        "DDR_LEGACY",
        "SM5",
        &chart,
        &audio,
        &synced,
        &["--auto-sync"],
    )?;
    assert_eq!(
        auto_sync_tail(&logs)?.get("auto_sync").map(String::as_str),
        Some("apply")
    );

    let logs = convert(
        "SM5",
        "DDR",
        &synced.join("sync.ssc"),
        &synced.join("sync.ogg"),
        &dir.path().join("recheck"),
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
    let (chart, audio) =
        write_legacy_wavm_input(&dir.path().join("in"), "sync", &synthetic_song(None)?, 20.0)?;
    let plain = dir.path().join("plain");
    let report = dir.path().join("report");
    convert("DDR_LEGACY", "SM5", &chart, &audio, &plain, &[])?;
    convert(
        "DDR_LEGACY",
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
    let (chart, audio) =
        write_legacy_wavm_input(&dir.path().join("in"), "sync", &synthetic_song(None)?, 20.0)?;
    let synced = dir.path().join("synced");
    let biased = dir.path().join("biased");
    convert(
        "DDR_LEGACY",
        "SM5",
        &chart,
        &audio,
        &synced,
        &["--auto-sync"],
    )?;
    convert(
        "DDR_LEGACY",
        "SM5",
        &chart,
        &audio,
        &biased,
        &["--auto-sync", "--sync-offset-ms", "10"],
    )?;
    let a = ssc::parse(&fs::read_to_string(synced.join("sync.ssc"))?)?;
    let b = ssc::parse(&fs::read_to_string(biased.join("sync.ssc"))?)?;
    let expected = a.audio_sync_offset_seconds.add(&Rational::new(10, 1000)?)?;
    assert_eq!(b.audio_sync_offset_seconds, expected);
    Ok(())
}
