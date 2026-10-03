//! `DDR → SM5` conversions with `--auto-sync`.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{auto_sync_tail, convert, synthetic_song, tail_number, write_sm5_input, TestResult};
use ddr_chart_tools::model::{Beat, Difficulty, Note, NoteKind, PanelSet, Rational, Style};
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

/// SSQ stores note positions as whole ticks (1024 per beat), so 12th and
/// 24th notes are rounded when a chart is written to DDR and come back
/// as 341/1024-style beats. The SSC writer must put them back on their
/// 12th/24th rows instead of refusing the chart. Reported against real
/// charts whose Expert/Challenge triplet runs failed with
/// `UnrepresentableBeat`.
#[test]
fn triplets_survive_ddr_round_trip() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut song = synthetic_song(None)?;
    let expert = song
        .charts
        .iter_mut()
        .find(|c| c.difficulty == Difficulty::Expert)
        .ok_or("synthetic song has an Expert chart")?;
    let tap = |beat: Rational| Note {
        beat: Beat::from_rational(beat),
        kind: NoteKind::Tap,
        panels: PanelSet::from_bits(Style::Single, 0x04),
    };
    // A 12th-note run across beats 8..9 and a 24th-note run across 12..13.
    let mut extra: Vec<Rational> = (0..3)
        .map(|i| Rational::new(24 + i, 3))
        .collect::<Result<_, _>>()?;
    extra.extend(
        (0..6)
            .map(|i| Rational::new(72 + i, 6))
            .collect::<Result<Vec<_>, _>>()?,
    );
    expert.notes.extend(extra.iter().copied().map(tap));
    expert.notes.sort_by_key(|n| n.beat);
    // Same-beat notes merge into one SSQ step byte, so compare beats.
    let mut expected_beats: Vec<Rational> =
        expert.notes.iter().map(|n| n.beat.as_rational()).collect();
    expected_beats.dedup();

    let src = dir.path().join("src");
    let (chart, audio) = write_sm5_input(&src, "trip", &song, 0.0)?;
    let ddr = dir.path().join("ddr");
    convert("SM5", "DDR", &chart, &audio, &ddr, &[])?;
    let back = dir.path().join("back");
    let logs = convert(
        "DDR",
        "SM5",
        &ddr.join("trip.ssq"),
        &ddr.join("trip.xwb"),
        &back,
        &[],
    )?;
    assert!(
        !logs.contains("WARN"),
        "tick rounding must not be reported as off-grid:\n{logs}"
    );

    let result = ssc::parse(&fs::read_to_string(back.join("trip.ssc"))?)?;
    let got = result
        .charts
        .iter()
        .find(|c| c.difficulty == Difficulty::Expert)
        .ok_or("round-tripped Expert chart")?;
    let got_beats: Vec<Rational> = got.notes.iter().map(|n| n.beat.as_rational()).collect();
    for beat in &extra {
        assert!(
            got_beats.contains(beat),
            "{beat:?} missing from round-tripped chart; got {got_beats:?}"
        );
    }
    assert_eq!(got_beats, expected_beats);
    Ok(())
}
