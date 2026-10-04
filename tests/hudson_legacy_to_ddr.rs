//! `DDR_LEGACY → DDR` for Hudson-format (DDR Hottest Party) inputs:
//! type 9 step chunks with gimmick items, plus RIFF WAV audio.
//!
//! Fixtures are synthetic, built from `docs/hudson_ssq_format.md` and the
//! RIFF spec; no game assets are read.

mod common;

use std::fs;
use std::path::Path;
use std::process::Command;

use common::{convert, TestResult};
use ddr_chart_tools::model::{Beat, NoteKind, Rational, Style};
use ddr_chart_tools::{ssq, xwb};

/// Source TPS, as on Hottest Party discs.
const TPS: u16 = 150;
/// WAV sample rate, as on Hottest Party rips.
const RATE: u32 = 32_000;

/// Append one SSQ chunk, padding its length to a dword.
fn push_chunk(out: &mut Vec<u8>, ty: u16, param2: u16, param3: u16, body: &[u8]) {
    let pad = (4 - (12 + body.len()) % 4) % 4;
    out.extend_from_slice(&((12 + body.len() + pad) as u32).to_le_bytes());
    out.extend_from_slice(&ty.to_le_bytes());
    out.extend_from_slice(&param2.to_le_bytes());
    out.extend_from_slice(&param3.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(body);
    out.extend(std::iter::repeat_n(0u8, pad));
}

/// Body of a Hudson type 9 chunk. `rows` are `(tick, mask, item bytes)`;
/// `chara` lists item types; `frez` lists freeze-end masks in row order.
fn hudson_body(rows: &[(i32, u8, &[u8])], chara: &[u16], frez: &[u8]) -> Vec<u8> {
    let mut body = vec![0u8; 4];
    for (tick, _, _) in rows {
        body.extend_from_slice(&tick.to_le_bytes());
    }
    for (_, mask, items) in rows {
        body.push(*mask);
        body.extend_from_slice(items);
    }
    while !body.len().is_multiple_of(4) {
        body.push(0);
    }
    let extra_start = body.len();
    let mut subs = Vec::new();
    subs.extend_from_slice(b"CHAR");
    subs.extend_from_slice(&((8 + 12 * chara.len()) as u32).to_le_bytes());
    for item in chara {
        subs.extend_from_slice(&u32::from(*item).to_le_bytes());
        subs.extend_from_slice(&[0u8; 8]);
    }
    let mut payload: Vec<u8> = frez.iter().flat_map(|m| [*m, 1, 0]).collect();
    while !payload.len().is_multiple_of(4) {
        payload.push(0);
    }
    subs.extend_from_slice(b"FREZ");
    subs.extend_from_slice(&((8 + payload.len()) as u32).to_le_bytes());
    subs.extend_from_slice(&payload);
    body.extend_from_slice(b"EXDT");
    body.extend_from_slice(&((8 + subs.len()) as u32).to_le_bytes());
    body.extend_from_slice(&subs);
    body[..4].copy_from_slice(&((extra_start - 4) as u32).to_le_bytes());
    body
}

/// A Hottest Party–shaped SSQ: TPS=150 tempo at 120 BPM over 8 measures,
/// an events chunk, one Single Basic Hudson chart exercising every
/// gimmicks-off rule, and an arcade-style type 9 metadata chunk.
fn hudson_ssq() -> Vec<u8> {
    let mut out = Vec::new();
    let mut tempo = Vec::new();
    for v in [0i32, 8 * 4096, 0, 8 * 300] {
        tempo.extend_from_slice(&v.to_le_bytes());
    }
    push_chunk(&mut out, 1, TPS, 2, &tempo);
    let mut events = 0i32.to_le_bytes().to_vec();
    events.extend_from_slice(&[1, 4]);
    push_chunk(&mut out, 2, 1, 1, &events);

    // Items: 1 = hand marker, 2 = echo, 3 = hazard.
    let rows: [(i32, u8, &[u8]); 6] = [
        (4096, 0x01, &[0]), // Left, plain
        (5120, 0x08, &[1]), // Right, hand marker -> arrow
        (6144, 0x02, &[2]), // Down, echo -> + Down one beat later
        (8192, 0x04, &[3]), // Up, hazard -> removed
        (9216, 0x01, &[0]), // Left, freeze head
        (11264, 0x00, &[]), // Left freeze end
    ];
    let chart = hudson_body(&rows, &[1, 2, 5], &[0x01]);
    push_chunk(&mut out, 9, 0x0114, rows.len() as u16, &chart);
    push_chunk(&mut out, 9, 0, 0, b"Some Artist\0");
    out.extend_from_slice(&0u32.to_le_bytes());
    out
}

/// Stereo 16-bit PCM WAV of `seconds` of a quiet square wave.
fn wav(seconds: u32) -> Vec<u8> {
    let frames = RATE * seconds;
    let data: Vec<u8> = (0..frames)
        .flat_map(|i| {
            let s: i16 = if (i / 40) % 2 == 0 { 1000 } else { -1000 };
            let b = s.to_le_bytes();
            [b[0], b[1], b[0], b[1]]
        })
        .collect();
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1u16.to_le_bytes());
    fmt.extend_from_slice(&2u16.to_le_bytes());
    fmt.extend_from_slice(&RATE.to_le_bytes());
    fmt.extend_from_slice(&(RATE * 4).to_le_bytes());
    fmt.extend_from_slice(&4u16.to_le_bytes());
    fmt.extend_from_slice(&16u16.to_le_bytes());
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&((4 + 8 + fmt.len() + 8 + data.len()) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    out.extend_from_slice(&fmt);
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    out
}

fn write_inputs(dir: &Path, stem: &str) -> TestResult<(std::path::PathBuf, std::path::PathBuf)> {
    fs::create_dir_all(dir)?;
    let chart = dir.join(format!("{stem}.ssq"));
    let audio = dir.join(format!("{stem}.wav"));
    fs::write(&chart, hudson_ssq())?;
    fs::write(&audio, wav(20))?;
    Ok((chart, audio))
}

fn ticks(beat: Beat) -> i64 {
    let r: Rational = beat.as_rational();
    r.num() * Beat::TICKS_PER_BEAT / r.den() as i64
}

fn assert_gimmicks_off_chart(ssq_bytes: &[u8]) -> TestResult {
    let parsed = ssq::parse(ssq_bytes)?;
    assert_eq!(parsed.song.tps, 1000);
    assert_eq!(
        parsed.song.charts.len(),
        1,
        "metadata type 9 must not become a chart"
    );
    let chart = &parsed.song.charts[0];
    assert_eq!(chart.style, Style::Single);
    let notes: Vec<(i64, u8, Option<i64>)> = chart
        .notes
        .iter()
        .map(|n| {
            let len = match n.kind {
                NoteKind::HoldHead { length } => Some(ticks(length)),
                _ => None,
            };
            (ticks(n.beat), n.panels.bits(), len)
        })
        .collect();
    assert_eq!(
        notes,
        [
            (4096, 0x01, None),
            (5120, 0x08, None),
            (6144, 0x02, None),
            (7168, 0x02, None),
            (9216, 0x01, Some(2048)),
        ]
    );
    Ok(())
}

#[test]
fn hudson_chart_and_wav_convert_to_ddr() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (chart, audio) = write_inputs(&dir.path().join("in"), "hpsong")?;
    let out = dir.path().join("out");
    let logs = convert("DDR_LEGACY", "DDR", &chart, &audio, &out, &[])?;
    assert!(logs.contains("dropped 1 hazard arrows"), "{logs}");

    let ssq_bytes = fs::read(out.join("hpsong.ssq"))?;
    assert_gimmicks_off_chart(&ssq_bytes)?;

    let bank = xwb::parse(&fs::read(out.join("hpsong.xwb"))?)?;
    assert!(!bank.entries.is_empty());
    for entry in &bank.entries {
        assert_eq!(
            entry.format.sample_rate(),
            RATE,
            "audio kept at source rate"
        );
    }
    let audio = xwb::parse_audio(&fs::read(out.join("hpsong.xwb"))?)?;
    assert_eq!(audio.channels, 2);
    assert!((audio.duration_seconds() - 20.0).abs() < 0.1);
    Ok(())
}

#[test]
fn batch_pairs_ssq_with_wav() -> TestResult {
    let dir = tempfile::tempdir()?;
    let input = dir.path().join("in");
    write_inputs(&input, "hpsong")?;
    let out = dir.path().join("out");
    let status = Command::new(env!("CARGO_BIN_EXE_ddr-chart-tools"))
        .args(["--from-format", "DDR_LEGACY", "--to-format", "DDR", "-q"])
        .arg("--input-folder")
        .arg(&input)
        .arg("--output-dir")
        .arg(&out)
        .status()?;
    assert!(status.success());
    assert_gimmicks_off_chart(&fs::read(out.join("hpsong.ssq"))?)?;
    assert!(out.join("hpsong.xwb").exists());
    Ok(())
}

#[test]
fn hudson_chart_converts_to_sm5() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (chart, audio) = write_inputs(&dir.path().join("in"), "hpsong")?;
    let out = dir.path().join("out");
    convert("DDR_LEGACY", "SM5", &chart, &audio, &out, &[])?;
    let ssc = fs::read_to_string(out.join("hpsong.ssc"))?;
    assert!(ssc.contains("#NOTES:"), "{ssc}");
    assert!(out.join("hpsong.ogg").exists());
    Ok(())
}
