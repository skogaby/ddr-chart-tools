//! Shared helpers for the conversion integration tests.
//!
//! Builds small synthetic songs with known sync: a 150 BPM chart pair
//! (irregular sixteenth-note pattern) and audio made of short decaying
//! noise bursts at every note time, shifted by a chosen amount. Also runs
//! the real binary and reads its auto-sync log line.

// Each test crate uses a different subset of these helpers.
#![allow(dead_code)]

use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use ddr_chart_tools::model::{
    AudioBuffer, Beat, Bpm, Chart, Difficulty, Note, NoteKind, PanelSet, PreviewSlice, Rational,
    Song, Style, TempoSegment,
};
use ddr_chart_tools::sync::TimeMap;
use ddr_chart_tools::{ogg, ssc};

pub type TestResult<T = ()> = Result<T, Box<dyn Error>>;

pub const RATE: u32 = 44_100;
const BPM: i64 = 150;
const SECONDS: f64 = 11.5;

/// Deterministic xorshift64* in [-1, 1).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let x = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (x >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }
}

/// A 150 BPM song with `#OFFSET` 0: an Expert chart on an irregular
/// sixteenth-note pattern from beat 2 to beat 26, and a Basic chart with
/// the Expert notes that fall on whole beats. `max_notes` truncates the
/// Expert chart (for too-few-notes cases).
pub fn synthetic_song(max_notes: Option<usize>) -> TestResult<Song> {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut expert = Vec::new();
    for sixteenth in 8..=104i64 {
        if rng.next() < 0.1 {
            expert.push(Rational::new(sixteenth, 4)?);
        }
    }
    if let Some(n) = max_notes {
        expert.truncate(n);
    }
    let note = |beat: Rational| Note {
        beat: Beat::from_rational(beat),
        kind: NoteKind::Tap,
        panels: PanelSet::from_bits(Style::Single, 0x01),
    };
    let basic = expert
        .iter()
        .filter(|b| b.den() == 1)
        .copied()
        .map(note)
        .collect();
    Ok(Song {
        title: Some("synthetic".to_string()),
        artist: None,
        tps: 1000,
        tempo_segments: vec![TempoSegment {
            start_beat: Beat::zero(),
            bpm: Bpm::from_rational(Rational::from_integer(BPM)),
        }],
        stops: Vec::new(),
        charts: vec![
            Chart {
                style: Style::Single,
                difficulty: Difficulty::Basic,
                notes: basic,
            },
            Chart {
                style: Style::Single,
                difficulty: Difficulty::Expert,
                notes: expert.into_iter().map(note).collect(),
            },
        ],
        audio: AudioBuffer {
            samples: Vec::new(),
            sample_rate: RATE,
            channels: 2,
        },
        audio_sync_offset_seconds: Rational::zero(),
        preview: PreviewSlice {
            start_seconds: Rational::from_integer(1),
            length_seconds: Rational::from_integer(2),
        },
    })
}

/// Stereo audio with a 20 ms decaying noise burst at each of `song`'s
/// note times plus `shift_ms` (positive = audio later than the chart),
/// over quiet background noise.
pub fn burst_audio(song: &Song, shift_ms: f64) -> TestResult<AudioBuffer> {
    let map = TimeMap::from_song(song).ok_or("song timing")?;
    let mut times: Vec<f64> = song
        .charts
        .iter()
        .flat_map(|c| c.notes.iter())
        .map(|n| map.seconds_at(n.beat.as_rational().as_f64()))
        .collect();
    times.sort_by(f64::total_cmp);
    times.dedup();

    let frames = (SECONDS * f64::from(RATE)) as usize;
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    let mut mono: Vec<f64> = (0..frames).map(|_| 0.01 * rng.next()).collect();
    let burst = (0.020 * f64::from(RATE)) as usize;
    for t in times {
        let start = ((t + shift_ms / 1000.0) * f64::from(RATE)).round() as usize;
        for k in 0..burst {
            if let Some(s) = mono.get_mut(start + k) {
                let decay = (-(k as f64) / (0.005 * f64::from(RATE))).exp();
                *s += 0.6 * decay * rng.next();
            }
        }
    }
    let samples = mono
        .iter()
        .flat_map(|&s| {
            let v = (s.clamp(-1.0, 1.0) * 32_767.0) as i16;
            [v, v]
        })
        .collect();
    Ok(AudioBuffer {
        samples,
        sample_rate: RATE,
        channels: 2,
    })
}

/// Write `song` as `<dir>/<stem>.ssc` plus `<dir>/<stem>.ogg` whose
/// audio is offset by `shift_ms`. Returns `(chart, audio)`.
pub fn write_sm5_input(
    dir: &Path,
    stem: &str,
    song: &Song,
    shift_ms: f64,
) -> TestResult<(PathBuf, PathBuf)> {
    fs::create_dir_all(dir)?;
    let chart = dir.join(format!("{stem}.ssc"));
    let audio = dir.join(format!("{stem}.ogg"));
    let mut text = Vec::new();
    ssc::write(song, &mut text)?;
    fs::write(&chart, text)?;
    let mut ogg_bytes = Vec::new();
    ogg::encode::encode(&burst_audio(song, shift_ms)?, &mut ogg_bytes)?;
    fs::write(&audio, ogg_bytes)?;
    Ok((chart, audio))
}

/// Run one single-file conversion with the real binary, writing into
/// `out_dir`. Fails the test on a nonzero exit; returns stderr (logs).
pub fn convert(
    from: &str,
    to: &str,
    chart: &Path,
    audio: &Path,
    out_dir: &Path,
    extra: &[&str],
) -> TestResult<String> {
    let output = Command::new(env!("CARGO_BIN_EXE_ddr-chart-tools"))
        .args(["--from-format", from, "--to-format", to, "--overwrite"])
        .arg("--chartfile")
        .arg(chart)
        .arg("--audiofile")
        .arg(audio)
        .arg("--output-dir")
        .arg(out_dir)
        .args(extra)
        .output()?;
    let stderr = String::from_utf8(output.stderr)?;
    if !output.status.success() {
        return Err(format!("conversion failed ({}):\n{stderr}", output.status).into());
    }
    Ok(stderr)
}

/// The `key=value` tail of the (single) auto-sync log line in `logs`.
pub fn auto_sync_tail(logs: &str) -> TestResult<HashMap<String, String>> {
    let line = logs
        .lines()
        .find(|l| l.contains("auto_sync="))
        .ok_or_else(|| format!("no auto-sync log line in:\n{logs}"))?;
    let tail = line.rsplit(" — ").next().ok_or("no tail")?;
    Ok(tail
        .split_whitespace()
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect())
}

/// A numeric field of an auto-sync log tail.
pub fn tail_number(tail: &HashMap<String, String>, key: &str) -> TestResult<f64> {
    Ok(tail
        .get(key)
        .ok_or_else(|| format!("no {key} in {tail:?}"))?
        .parse()?)
}

// ---------------------------------------------------------------------
// Legacy inputs
// ---------------------------------------------------------------------

/// IMA ADPCM step sizes (the decoder's table in `src/wavm/xbox_ima.rs`).
const IMA_STEP: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
const IMA_INDEX: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

/// Quantize one sample to an IMA nibble, advancing the decoder state
/// exactly as `src/wavm/xbox_ima.rs` will when it decodes it.
fn ima_nibble(sample: i16, hist: &mut i32, index: &mut i32) -> u8 {
    let step = IMA_STEP[*index as usize];
    let mut diff = i32::from(sample) - *hist;
    let mut nibble = 0u8;
    if diff < 0 {
        nibble = 8;
        diff = -diff;
    }
    if diff >= step {
        nibble |= 4;
        diff -= step;
    }
    if diff >= step >> 1 {
        nibble |= 2;
        diff -= step >> 1;
    }
    if diff >= step >> 2 {
        nibble |= 1;
    }
    let mut delta = step >> 3;
    if nibble & 1 != 0 {
        delta += step >> 2;
    }
    if nibble & 2 != 0 {
        delta += step >> 1;
    }
    if nibble & 4 != 0 {
        delta += step;
    }
    if nibble & 8 != 0 {
        delta = -delta;
    }
    *hist = (*hist + delta).clamp(-32_768, 32_767);
    *index = (*index + IMA_INDEX[usize::from(nibble)]).clamp(0, 88);
    nibble
}

/// Encode 44.1 kHz stereo PCM as WAVM (headerless XBOX-IMA): 0x48-byte
/// blocks of 64 frames, a 4-byte header per channel followed by nibble
/// data interleaved in 4-byte chunks per channel.
pub fn encode_wavm(audio: &AudioBuffer) -> TestResult<Vec<u8>> {
    if audio.channels != 2 || audio.sample_rate != RATE {
        return Err("WAVM is 2-channel 44.1 kHz".into());
    }
    let frames = audio.samples.len() / 2;
    let mut out = Vec::new();
    let mut index = [0i32; 2];
    for block_start in (0..frames).step_by(64) {
        let mut block = [0u8; 0x48];
        for ch in 0..2 {
            let sample = |f: usize| {
                audio
                    .samples
                    .get(2 * (block_start + f) + ch)
                    .copied()
                    .unwrap_or(0)
            };
            let first = sample(0);
            let mut hist = i32::from(first);
            block[ch * 4..ch * 4 + 2].copy_from_slice(&first.to_le_bytes());
            block[ch * 4 + 2] = index[ch] as u8;
            for f in 1..64 {
                let nibble = ima_nibble(sample(f), &mut hist, &mut index[ch]);
                let p = f - 1;
                let byte = p / 2;
                let offset = 8 + (byte / 4) * 8 + ch * 4 + byte % 4;
                block[offset] |= if p % 2 == 0 { nibble } else { nibble << 4 };
            }
        }
        out.extend_from_slice(&block);
    }
    Ok(out)
}

/// A legacy-format input in `dir`: `<stem>.ssq` authored by the tool
/// from `song`, plus audio shifted by `shift_ms` as `<stem>.wavm`.
pub fn write_legacy_wavm_input(
    dir: &Path,
    stem: &str,
    song: &Song,
    shift_ms: f64,
) -> TestResult<(PathBuf, PathBuf)> {
    let (ssq, _, _) = write_legacy_xwb_input(dir, stem, song, 0.0)?;
    let wavm = dir.join(format!("{stem}.wavm"));
    fs::write(&wavm, encode_wavm(&burst_audio(song, shift_ms)?)?)?;
    for ext in ["xwb", "xsb"] {
        fs::remove_file(dir.join(format!("{stem}.{ext}")))?;
    }
    Ok((ssq, wavm))
}

/// A legacy-format input in `dir`: `<stem>.ssq` + `<stem>.xwb` +
/// `<stem>.xsb`, as the tool writes them for `song` with audio shifted by
/// `shift_ms` (a DDR-profile bank, so conversions pass it through).
pub fn write_legacy_xwb_input(
    dir: &Path,
    stem: &str,
    song: &Song,
    shift_ms: f64,
) -> TestResult<(PathBuf, PathBuf, PathBuf)> {
    let src = dir.join("src");
    let (chart, audio) = write_sm5_input(&src, stem, song, shift_ms)?;
    convert("SM5", "DDR", &chart, &audio, dir, &[])?;
    fs::remove_dir_all(&src)?;
    Ok((
        dir.join(format!("{stem}.ssq")),
        dir.join(format!("{stem}.xwb")),
        dir.join(format!("{stem}.xsb")),
    ))
}
