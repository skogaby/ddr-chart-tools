//! `DDR_LEGACY → DDR` conversions with `--auto-sync`.

mod common;

use std::fs;

use common::{
    auto_sync_tail, convert, synthetic_song, tail_number, write_legacy_wavm_input,
    write_legacy_xwb_input, TestResult,
};
use ddr_chart_tools::{ssq, xwb};

#[test]
fn auto_sync_converges_on_wavm_audio() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (chart, audio) =
        write_legacy_wavm_input(&dir.path().join("in"), "sync", &synthetic_song(None)?, 20.0)?;
    let synced = dir.path().join("synced");
    let logs = convert(
        "DDR_LEGACY",
        "DDR",
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
        "DDR",
        "SM5",
        &synced.join("sync.ssq"),
        &synced.join("sync.xwb"),
        &dir.path().join("recheck"),
        &["--auto-sync", "report"],
    )?;
    let tail = auto_sync_tail(&logs)?;
    let delta = tail_number(&tail, "delta_ms")?;
    assert!(delta.abs() <= 1.0, "second pass wants {delta} ms: {tail:?}");
    Ok(())
}

#[test]
fn auto_sync_is_linear_in_audio_shift() -> TestResult {
    let dir = tempfile::tempdir()?;
    let song = synthetic_song(None)?;
    let mut corrections = Vec::new();
    for (name, shift) in [("early", 0.0), ("late", 25.0)] {
        let (chart, audio) = write_legacy_wavm_input(&dir.path().join(name), "sync", &song, shift)?;
        let logs = convert(
            "DDR_LEGACY",
            "DDR",
            &chart,
            &audio,
            &dir.path().join(format!("{name}-out")),
            &["--auto-sync", "report"],
        )?;
        corrections.push(tail_number(&auto_sync_tail(&logs)?, "correction_ms")?);
    }
    let diff = corrections[1] - corrections[0];
    assert!((diff - 25.0).abs() <= 1.0, "expected +25 ms, got {diff}");
    Ok(())
}

#[test]
fn report_mode_writes_the_same_chart() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (chart, audio) =
        write_legacy_wavm_input(&dir.path().join("in"), "sync", &synthetic_song(None)?, 20.0)?;
    let plain = dir.path().join("plain");
    let report = dir.path().join("report");
    convert("DDR_LEGACY", "DDR", &chart, &audio, &plain, &[])?;
    convert(
        "DDR_LEGACY",
        "DDR",
        &chart,
        &audio,
        &report,
        &["--auto-sync", "report"],
    )?;
    assert_eq!(
        fs::read(plain.join("sync.ssq"))?,
        fs::read(report.join("sync.ssq"))?
    );
    Ok(())
}

#[test]
fn passthrough_audio_is_untouched_while_the_chart_moves() -> TestResult {
    let dir = tempfile::tempdir()?;
    let input = dir.path().join("in");
    let (chart, bank, sounds) =
        write_legacy_xwb_input(&input, "sync", &synthetic_song(None)?, 20.0)?;
    let plain = dir.path().join("plain");
    let synced = dir.path().join("synced");
    convert("DDR_LEGACY", "DDR", &chart, &bank, &plain, &[])?;
    let logs = convert(
        "DDR_LEGACY",
        "DDR",
        &chart,
        &bank,
        &synced,
        &["--auto-sync"],
    )?;
    assert!(logs.contains("audio passthrough"), "{logs}");
    let delta = tail_number(&auto_sync_tail(&logs)?, "delta_ms")? as i32;
    assert_ne!(delta, 0, "the source is 20 ms off; it should be corrected");

    assert_eq!(
        fs::read(synced.join("sync.xwb"))?,
        fs::read(&bank)?,
        "XWB byte-copied"
    );
    assert_eq!(
        fs::read(synced.join("sync.xsb"))?,
        fs::read(&sounds)?,
        "XSB byte-copied"
    );
    let a = ssq::parse(&fs::read(plain.join("sync.ssq"))?)?.raw_tempo_pairs;
    let b = ssq::parse(&fs::read(synced.join("sync.ssq"))?)?.raw_tempo_pairs;
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x.0, y.0);
        assert_eq!(y.1 - x.1, delta, "every anchor moves by the applied delta");
    }
    Ok(())
}

#[test]
fn undecodable_passthrough_audio_skips_auto_sync() -> TestResult {
    let dir = tempfile::tempdir()?;
    let input = dir.path().join("in");
    let (chart, bank, _) = write_legacy_xwb_input(&input, "sync", &synthetic_song(None)?, 20.0)?;
    // Keep the bank DDR-compliant (so it is passed through) but make its
    // audio undecodable: MS-ADPCM predictor index 7 does not exist.
    let mut parsed = xwb::parse(&fs::read(&bank)?)?;
    let main = parsed
        .entries
        .iter_mut()
        .max_by_key(|e| e.data.len())
        .ok_or("no entries")?;
    main.data[0] = 7;
    let mut corrupted = Vec::new();
    xwb::write(&parsed, &mut corrupted)?;
    fs::write(&bank, &corrupted)?;

    let plain = dir.path().join("plain");
    let synced = dir.path().join("synced");
    convert("DDR_LEGACY", "DDR", &chart, &bank, &plain, &[])?;
    let logs = convert(
        "DDR_LEGACY",
        "DDR",
        &chart,
        &bank,
        &synced,
        &["--auto-sync"],
    )?;
    assert!(logs.contains("auto-sync skipped"), "{logs}");
    assert_eq!(
        fs::read(plain.join("sync.ssq"))?,
        fs::read(synced.join("sync.ssq"))?
    );
    assert_eq!(fs::read(synced.join("sync.xwb"))?, corrupted);
    Ok(())
}

#[test]
fn suffix_names_files_and_bank_of_a_derived_code() -> TestResult {
    let dir = tempfile::tempdir()?;
    let input = dir.path().join("in");
    let (chart, audio) = write_legacy_wavm_input(&input, "sign", &synthetic_song(None)?, 0.0)?;
    // A basename that is not a valid song code (capital, space).
    let chart_named = input.join("Sign Here.ssq");
    let audio_named = input.join("Sign Here.wavm");
    fs::rename(&chart, &chart_named)?;
    fs::rename(&audio, &audio_named)?;

    let out = dir.path().join("out");
    let logs = convert(
        "DDR_LEGACY",
        "DDR",
        &chart_named,
        &audio_named,
        &out,
        &["--suffix", "_h"],
    )?;
    assert!(logs.contains("writing it as sign_h"), "{logs}");
    for ext in ["ssq", "xwb", "xsb"] {
        assert!(out.join(format!("sign_h.{ext}")).is_file(), "sign_h.{ext}");
    }
    let bank = xwb::parse(&fs::read(out.join("sign_h.xwb"))?)?;
    assert_eq!(bank.name_str(), "sign_h", "bank named after the files");
    Ok(())
}

#[test]
fn passthrough_is_skipped_when_the_song_code_changes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let input = dir.path().join("in");
    let (chart, bank, _) = write_legacy_xwb_input(&input, "sync", &synthetic_song(None)?, 0.0)?;
    let out = dir.path().join("out");
    let logs = convert(
        "DDR_LEGACY",
        "DDR",
        &chart,
        &bank,
        &out,
        &["--song-code", "sync_h"],
    )?;
    // A byte-copied bank would still be named `sync`, so the game would
    // never find the `sync_h` cue.
    assert!(!logs.contains("audio passthrough (XWB+XSB"), "{logs}");
    let written = xwb::parse(&fs::read(out.join("sync_h.xwb"))?)?;
    assert_eq!(written.name_str(), "sync_h");
    Ok(())
}
