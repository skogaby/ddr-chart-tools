//! Per-job conversion orchestrator.
//!
//! `run_one` reads inputs, dispatches to the right parser/writer per
//! `(from, to)` pair, and writes output files colocated with inputs.

pub mod batch;

use std::fs;
use std::path::{Path, PathBuf};

use log::{info, warn};

use crate::cli::job::{Format, Job};
use crate::error::Error;
use crate::model::{AudioBuffer, PreviewSlice};
use crate::ogg;
use crate::ssc;
use crate::ssq;
use crate::ssq::events::SsqEvent;
use crate::ssq_legacy;
use crate::wavm;
use crate::xsb;
use crate::xwb;
use crate::xwb::adpcm;
use crate::xwb::container::{WaveFormat, XwbBank, XwbEntry};

/// Execute one conversion job.
pub fn run_one(job: &Job) -> Result<(), Error> {
    fs::create_dir_all(&job.output_dir)?;

    match (job.from, job.to) {
        (Format::Ddr, Format::Sm5) => ddr_to_sm5(job),
        (Format::Sm5, Format::Ddr) => sm5_to_ddr(job),
        (Format::DdrLegacy, Format::Ddr) => legacy_to_ddr(job),
        (Format::DdrLegacy, Format::Sm5) => legacy_to_sm5(job),
        _ => unreachable!("CLI validation prevents unsupported combos"),
    }
}

// -----------------------------------------------------------------------
// Direction implementations
// -----------------------------------------------------------------------

fn ddr_to_sm5(job: &Job) -> Result<(), Error> {
    let chart_bytes = fs::read(&job.chart_in)?;
    let audio_bytes = fs::read(&job.audio_in)?;

    let mut result = ssq::parse(&chart_bytes)?;
    let audio = xwb::parse_audio(&audio_bytes)?;
    result.song.audio = audio;

    let ssc_path = output_path(&job.chart_in, "ssc", &job.output_dir);
    let ogg_path = output_path(&job.chart_in, "ogg", &job.output_dir);
    check_overwrite(&ssc_path, job.overwrite)?;
    check_overwrite(&ogg_path, job.overwrite)?;

    let mut ssc_out = Vec::new();
    ssc::write(&result.song, &mut ssc_out)?;
    fs::write(&ssc_path, &ssc_out)?;
    info!("wrote {}", ssc_path.display());

    let mut ogg_out = Vec::new();
    ogg::encode::encode(&result.song.audio, &mut ogg_out)?;
    fs::write(&ogg_path, &ogg_out)?;
    info!("wrote {}", ogg_path.display());

    Ok(())
}

fn sm5_to_ddr(job: &Job) -> Result<(), Error> {
    let chart_text = fs::read_to_string(&job.chart_in)?;
    let audio_bytes = fs::read(&job.audio_in)?;

    let ext = job
        .chart_in
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let mut song = if ext.eq_ignore_ascii_case("sm") {
        crate::sm::parse(&chart_text)?
    } else {
        ssc::parse(&chart_text)?
    };

    let audio = ogg::decode::decode(&audio_bytes)?;
    song.audio = audio;
    song.tps = 1000;

    let ssq_path = output_path(&job.chart_in, "ssq", &job.output_dir);
    let xwb_path = output_path(&job.chart_in, "xwb", &job.output_dir);
    let xsb_path = output_path(&job.chart_in, "xsb", &job.output_dir);
    check_overwrite(&ssq_path, job.overwrite)?;
    check_overwrite(&xwb_path, job.overwrite)?;
    check_overwrite(&xsb_path, job.overwrite)?;

    // SSQ — synthesize canonical events and tempo pairs from the Song.
    let (events, raw_tempo_pairs) = synthesize_events(&song, &[]);
    let mut ssq_out = Vec::new();
    ssq::writer::write(&song, &events, &raw_tempo_pairs, &mut ssq_out)?;
    fs::write(&ssq_path, &ssq_out)?;
    info!("wrote {}", ssq_path.display());

    // XWB + XSB.
    let code = song_code(&job.chart_in);
    write_ddr_audio(&song.audio, &song.preview, &code, &xwb_path, &xsb_path)?;

    Ok(())
}

fn legacy_to_ddr(job: &Job) -> Result<(), Error> {
    let chart_bytes = fs::read(&job.chart_in)?;
    let audio_bytes = fs::read(&job.audio_in)?;

    let mut result = ssq::parse(&chart_bytes)?;

    // Log dropped aux chunks.
    for aux in &result.aux_chunks_dropped {
        warn!(
            "{}: dropped legacy chunk type {} ({} bytes)",
            job.chart_in.display(),
            aux.ty,
            aux.size,
        );
    }

    ssq_legacy::modernize::modernize(&mut result);
    apply_sync_offset(&mut result, job.sync_offset_ms);

    let ssq_path = output_path(&job.chart_in, "ssq", &job.output_dir);
    let xwb_path = output_path(&job.chart_in, "xwb", &job.output_dir);
    let xsb_path = output_path(&job.chart_in, "xsb", &job.output_dir);
    check_overwrite(&ssq_path, job.overwrite)?;
    check_overwrite(&xwb_path, job.overwrite)?;
    check_overwrite(&xsb_path, job.overwrite)?;

    // Write modernized SSQ. Discard the source's events and synthesize
    // the canonical 6-event sequence: Ultramix-era SSQs place event[3]
    // (alt-start cue 0xF8) at a different tick than event[2] (chart
    // start 0xFA), which DDR World rejects. Synthesizing from scratch
    // matches the SM5→DDR path and produces the spec's canonical shape.
    // `synthesize_events` also extends `raw_tempo_pairs` as needed to
    // keep FINISH bracketed by TIMING notes (see its doc comment).
    let (events, tempo_pairs) = synthesize_events(&result.song, &result.raw_tempo_pairs);
    let mut ssq_out = Vec::new();
    ssq::writer::write(
        &result.song,
        &events,
        &tempo_pairs,
        &mut ssq_out,
    )?;
    fs::write(&ssq_path, &ssq_out)?;
    info!("wrote {}", ssq_path.display());

    // Audio: try passthrough, else decode + re-encode.
    if try_audio_passthrough(&audio_bytes, &job.audio_in, &xwb_path, &xsb_path)? {
        info!("audio passthrough (XWB+XSB byte-copied)");
    } else {
        let audio = decode_legacy_audio(&audio_bytes)?;
        result.song.audio = audio;
        let code = song_code(&job.chart_in);
        write_ddr_audio(&result.song.audio, &result.song.preview, &code, &xwb_path, &xsb_path)?;
    }

    Ok(())
}

fn legacy_to_sm5(job: &Job) -> Result<(), Error> {
    let chart_bytes = fs::read(&job.chart_in)?;
    let audio_bytes = fs::read(&job.audio_in)?;

    let mut result = ssq::parse(&chart_bytes)?;

    for aux in &result.aux_chunks_dropped {
        warn!(
            "{}: dropped legacy chunk type {} ({} bytes)",
            job.chart_in.display(),
            aux.ty,
            aux.size,
        );
    }

    ssq_legacy::modernize::modernize(&mut result);
    apply_sync_offset(&mut result, job.sync_offset_ms);

    apply_ultramix_sif_if_present(&job.chart_in, &mut result.song);

    let audio = decode_legacy_audio(&audio_bytes)?;
    result.song.audio = audio;

    let ssc_path = output_path(&job.chart_in, "ssc", &job.output_dir);
    let ogg_path = output_path(&job.chart_in, "ogg", &job.output_dir);
    check_overwrite(&ssc_path, job.overwrite)?;
    check_overwrite(&ogg_path, job.overwrite)?;

    let mut ssc_out = Vec::new();
    ssc::write(&result.song, &mut ssc_out)?;
    fs::write(&ssc_path, &ssc_out)?;
    info!("wrote {}", ssc_path.display());

    let mut ogg_out = Vec::new();
    ogg::encode::encode(&result.song.audio, &mut ogg_out)?;
    fs::write(&ogg_path, &ogg_out)?;
    info!("wrote {}", ogg_path.display());

    Ok(())
}

/// Add a user-specified sync offset (in milliseconds) to the post-modernize
/// audio-sync state. Applied to both `audio_sync_offset_seconds` (consumed
/// by the SSC writer as `#OFFSET`) and `raw_tempo_pairs[0].1` (consumed by
/// the SSQ writer as `tempo_data[0]`). Modernize runs first, so `song.tps`
/// is already 1000 and seconds-ticks are directly in milliseconds.
fn apply_sync_offset(result: &mut crate::ssq::SsqParseResult, offset_ms: i32) {
    if offset_ms == 0 {
        return;
    }
    let offset_seconds = crate::model::Rational::new(offset_ms as i64, 1000)
        .unwrap_or(crate::model::Rational::zero());
    if let Ok(new) = result.song.audio_sync_offset_seconds.add(&offset_seconds) {
        result.song.audio_sync_offset_seconds = new;
    }
    if let Some(pair) = result.raw_tempo_pairs.first_mut() {
        pair.1 = pair.1.saturating_add(offset_ms);
    }
}

// -----------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------

/// Derive output path: input's basename with new extension, placed in
/// the job's output directory. For Ultramix inputs, strips the `_all`
/// suffix (e.g. `abs2_all.ssq` → `abs2.ssc`) so output filenames match
/// the canonical per-song ID the game uses to find assets.
fn output_path(input: &Path, ext: &str, output_dir: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.strip_suffix("_all").unwrap_or(s))
        .unwrap_or("");
    output_dir.join(Path::new(stem).with_extension(ext))
}

/// Fail if `path` exists and overwrite is not enabled.
fn check_overwrite(path: &Path, overwrite: bool) -> Result<(), Error> {
    if !overwrite && path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("output file already exists: {}", path.display()),
        )
        .into());
    }
    Ok(())
}

/// Look for an Ultramix `.sif` file alongside the chart and populate
/// `song.title` / `song.artist` from it.
///
/// Chart filenames are `{id}_all.ssq`; the sibling SIF is `{id}.sif`.
/// The SIF is a sequence of null-terminated ASCII strings at fixed
/// indices (see docs/ultramix_archive_formats.md):
///   [0] empty leader, [1] short_id, [2] title, [3] subtitle, [4] artist.
/// When a subtitle is present it's appended to the title with a space.
fn apply_ultramix_sif_if_present(chart_path: &Path, song: &mut crate::model::Song) {
    let Some(stem) = chart_path.file_stem().and_then(|s| s.to_str()) else {
        return;
    };
    let id = stem.strip_suffix("_all").unwrap_or(stem);
    let sif_path = chart_path.with_file_name(format!("{id}.sif"));
    let Ok(bytes) = fs::read(&sif_path) else {
        return;
    };
    let fields: Vec<&str> = bytes
        .split(|&b| b == 0)
        .filter_map(|s| std::str::from_utf8(s).ok())
        .collect();
    // Field layout: [0]=empty leader, [1]=id, [2]=title, [3]=subtitle, [4]=artist.
    let title = fields.get(2).copied().unwrap_or("");
    let subtitle = fields.get(3).copied().unwrap_or("");
    let artist = fields.get(4).copied().unwrap_or("");
    if !title.is_empty() {
        song.title = Some(if subtitle.is_empty() {
            title.to_string()
        } else {
            format!("{title} {subtitle}")
        });
    }
    if !artist.is_empty() {
        song.artist = Some(artist.to_string());
    }
    info!("applied metadata from {}", sif_path.display());
}

/// Derive a 4-char song code from the input chart's basename.
/// For Ultramix inputs, strips the `_all` suffix first so the code is
/// the canonical 4-char per-song ID (e.g. `abs2_all` → `abs2`).
fn song_code(chart_path: &Path) -> String {
    let stem = chart_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.strip_suffix("_all").unwrap_or(s))
        .unwrap_or("song");
    let alnum: String = stem
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(4)
        .collect();
    if alnum.is_empty() {
        "song".to_string()
    } else {
        alnum
    }
}

/// Detect and decode legacy audio (XWB or WAVM) by header inspection.
fn decode_legacy_audio(bytes: &[u8]) -> Result<AudioBuffer, Error> {
    // Try XWB first (has "WBND" magic).
    if bytes.len() >= 4 && &bytes[..4] == b"WBND" {
        return Ok(xwb::parse_audio(bytes)?);
    }
    // Fall back to WAVM (headerless XBOX-IMA).
    Ok(wavm::parse(bytes)?)
}

/// DDR-format WaveFormat: ADPCM, 2ch, 44100Hz, raw_align=48.
fn ddr_wave_format() -> WaveFormat {
    WaveFormat::from_packed(2 | (2 << 2) | (44100 << 5) | (48 << 23))
}

/// Encode audio + preview and write XWB + XSB.
fn write_ddr_audio(
    audio: &AudioBuffer,
    preview: &PreviewSlice,
    code: &str,
    xwb_path: &Path,
    xsb_path: &Path,
) -> Result<(), Error> {
    let fmt = ddr_wave_format();

    // Encode main audio.
    let main_adpcm = adpcm::encode::encode(&audio.samples, &fmt)?;

    // Slice and encode preview.
    let preview_pcm = slice_preview(audio, preview);
    let preview_adpcm = adpcm::encode::encode(&preview_pcm, &fmt)?;

    // Build XWB bank.
    let bank = build_xwb_bank(code, &fmt, &main_adpcm, &preview_adpcm);
    let mut xwb_out = Vec::new();
    xwb::write(&bank, &mut xwb_out)?;
    fs::write(xwb_path, &xwb_out)?;
    info!("wrote {}", xwb_path.display());

    // Write XSB.
    let mut xsb_out = Vec::new();
    xsb::write(code, &mut xsb_out)?;
    fs::write(xsb_path, &xsb_out)?;
    info!("wrote {}", xsb_path.display());

    Ok(())
}

/// Extract the preview slice from the audio buffer as interleaved PCM.
fn slice_preview(audio: &AudioBuffer, preview: &PreviewSlice) -> Vec<i16> {
    let ch = audio.channels as usize;
    if ch == 0 || audio.sample_rate == 0 {
        return Vec::new();
    }
    let start_frame = (preview.start_seconds.as_f64() * audio.sample_rate as f64) as usize;
    let length_frames = (preview.length_seconds.as_f64() * audio.sample_rate as f64) as usize;
    let total_frames = audio.samples.len() / ch;

    let start = start_frame.min(total_frames);
    let end = (start + length_frames).min(total_frames);

    audio.samples[start * ch..end * ch].to_vec()
}

/// Build a DDR-compatible XwbBank with main + preview entries.
fn build_xwb_bank(
    code: &str,
    fmt: &WaveFormat,
    main_data: &[u8],
    preview_data: &[u8],
) -> XwbBank {
    let spb = fmt.samples_per_block() as usize;
    let ba = fmt.block_align() as usize;

    let main_duration = if ba > 0 { (main_data.len() / ba) * spb } else { 0 };
    let preview_duration = if ba > 0 { (preview_data.len() / ba) * spb } else { 0 };

    let mut bank_name = [0u8; 64];
    for (i, &b) in code.as_bytes().iter().enumerate().take(64) {
        bank_name[i] = b;
    }

    let make_entry = |name: &str, data: &[u8], duration: usize| -> XwbEntry {
        let mut name_bytes = vec![0u8; 64];
        for (i, &b) in name.as_bytes().iter().enumerate().take(64) {
            name_bytes[i] = b;
        }
        XwbEntry {
            flags_and_duration: (duration as u32) << 4,
            format: *fmt,
            data: data.to_vec(),
            loop_start: 0,
            loop_length: duration.saturating_sub(1) as u32,
            name_bytes,
        }
    };

    let preview_name = format!("{code}_s");
    XwbBank {
        header_version: 42,
        flags: 0x0009_0001,
        name: bank_name,
        entry_name_element_size: 64,
        alignment: 2048,
        compact_format: 0,
        build_time: 0,
        entries: vec![
            make_entry(&preview_name, preview_data, preview_duration),
            make_entry(code, main_data, main_duration),
        ],
    }
}

/// Try to byte-copy XWB+XSB if the source audio is already DDR-compliant.
///
/// Returns `true` if passthrough succeeded, `false` if re-encode is needed.
fn try_audio_passthrough(
    audio_bytes: &[u8],
    audio_path: &Path,
    xwb_out: &Path,
    xsb_out: &Path,
) -> Result<bool, Error> {
    // Must be XWB (not WAVM).
    if audio_bytes.len() < 4 || &audio_bytes[..4] != b"WBND" {
        return Ok(false);
    }

    // Parse container to check compliance.
    let bank = match xwb::parse(audio_bytes) {
        Ok(b) => b,
        Err(_) => return Ok(false),
    };

    // Must have exactly 2 entries.
    if bank.entries.len() != 2 {
        return Ok(false);
    }

    // Both entries must be DDR-format ADPCM.
    let expected_fmt = ddr_wave_format();
    for entry in &bank.entries {
        if entry.format != expected_fmt {
            return Ok(false);
        }
    }

    // Check for sibling XSB.
    let xsb_src = audio_path.with_extension("xsb");
    if !xsb_src.is_file() {
        return Ok(false);
    }

    // All checks pass — byte-copy both files.
    fs::copy(audio_path, xwb_out)?;
    fs::copy(&xsb_src, xsb_out)?;
    Ok(true)
}

/// Synthesize the canonical 6-event sequence (spec §4.4) and return it
/// alongside a tempo-pair list guaranteed to bracket FINISH.
///
/// The game (1) assigns a `musicCount` to each non-TIMING note by
/// linearly interpolating between the surrounding TIMING notes and
/// (2) uses `musicCount` to drive a per-frame beatCount lookup via
/// `lower_bound` over the notes list. If FINISH sits past the last
/// TIMING note in walk-order, it never gets a `musicCount` assigned
/// (stays at `INT32_MIN`), which both (a) causes the results-screen
/// guard to fire instantly on STEP_FINISHED and (b) leaves an
/// out-of-order `musicCount` in the notes list — which breaks
/// `lower_bound` and makes per-frame beatCount computation return
/// garbage. That garbage manifests as the game locking at READY
/// without ever transitioning to GO / gameplay.
///
/// To guarantee FINISH is bracketed by TIMING notes: the last tempo
/// pair must sit at or past END's tick. Hand-authored reference
/// charts place the trailing tempo pair at END's tick exactly. If the
/// source's trailing tempo pair is already past FINISH+1 measure, we
/// adopt its tick as END; otherwise we extrapolate a new trailing
/// pair at `last_note + 2 measures` using the last segment's BPM.
fn synthesize_events(
    song: &crate::model::Song,
    raw_tempo_pairs: &[(i32, i32)],
) -> (Vec<SsqEvent>, Vec<(i32, i32)>) {
    use crate::model::NoteKind;

    // Find the last note tick across all charts.
    let last_tick: i32 = song
        .charts
        .iter()
        .flat_map(|c| c.notes.iter())
        .filter_map(|n| {
            let beat_r = match n.kind {
                NoteKind::HoldHead { length } => {
                    n.beat.as_rational().add(&length.as_rational()).ok()?
                }
                _ => n.beat.as_rational(),
            };
            let num = (beat_r.num() as i128).checked_mul(1024)?;
            let den = beat_r.den() as i128;
            let half = if num >= 0 { den / 2 } else { -(den / 2) };
            i32::try_from((num + half) / den).ok()
        })
        .max()
        .unwrap_or(4096);

    let last_measure = ((last_tick + 4095) / 4096) * 4096;
    let desired_end = last_measure + 8192; // last note + 2 measures

    // Build the tempo-pair list that goes into the SSQ. Invariant:
    // the final pair's time_offset == end_tick, so END coincides with
    // the trailing TIMING note and FINISH (one measure earlier) falls
    // strictly inside the last real tempo bracket.
    let source_last_tick = raw_tempo_pairs.last().map(|p| p.0).unwrap_or(0);
    let end_tick = std::cmp::max(desired_end, source_last_tick);
    let finish_tick = end_tick - 4096;

    let tempo_pairs = if raw_tempo_pairs.is_empty() {
        // SM5→DDR path: leave empty; the SSQ writer will synthesize
        // from the Song's semantic view. Bracketing there is addressed
        // separately if it becomes an issue.
        Vec::new()
    } else if source_last_tick >= end_tick {
        raw_tempo_pairs.to_vec()
    } else {
        extend_tempo_pairs_to(raw_tempo_pairs, end_tick)
    };

    let events = vec![
        SsqEvent { tick: 0,           code: 1, arg: 4 },
        SsqEvent { tick: 0,           code: 2, arg: 1 },
        SsqEvent { tick: 4096,        code: 2, arg: 2 },
        SsqEvent { tick: 4096,        code: 2, arg: 5 },
        SsqEvent { tick: finish_tick, code: 2, arg: 3 },
        SsqEvent { tick: end_tick,    code: 2, arg: 4 },
    ];

    (events, tempo_pairs)
}

/// Append a synthesized trailing tempo pair at `end_tick`, extrapolating
/// seconds-ticks linearly from the last real tempo segment.
fn extend_tempo_pairs_to(pairs: &[(i32, i32)], end_tick: i32) -> Vec<(i32, i32)> {
    let mut out = pairs.to_vec();
    let n = out.len();
    if n < 2 {
        // No segment to extrapolate from — fall back to "same seconds-tick"
        // (degenerate, but non-crashing).
        if let Some(last) = out.last().copied() {
            out.push((end_tick, last.1));
        }
        return out;
    }
    let (t0, s0) = out[n - 2];
    let (t1, s1) = out[n - 1];
    let dt = (t1 - t0) as i64;
    let ds = (s1 - s0) as i64;
    // seconds-ticks per measure-tick in the last segment
    let extra_ticks = (end_tick - t1) as i64;
    let extra_seconds = if dt != 0 {
        (ds * extra_ticks + dt / 2) / dt
    } else {
        0
    };
    let new_s = (s1 as i64 + extra_seconds).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
    out.push((end_tick, new_s));
    out
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AudioBuffer, Beat, Chart, Difficulty, Note, NoteKind, PanelSet, PreviewSlice, Rational,
        Song, Style,
    };

    /// Build a minimal Song with a single Single-Basic chart holding
    /// one tap note at the given measure-tick position.
    fn song_with_last_note_at(last_tick: i64) -> Song {
        Song {
            title: None,
            artist: None,
            tps: 1000,
            tempo_segments: Vec::new(),
            stops: Vec::new(),
            charts: vec![Chart {
                style: Style::Single,
                difficulty: Difficulty::Basic,
                notes: vec![Note {
                    beat: Beat::from_measure_ticks(last_tick).unwrap(),
                    kind: NoteKind::Tap,
                    panels: PanelSet::from_bits(Style::Single, 0x01),
                }],
            }],
            audio: AudioBuffer {
                samples: Vec::new(),
                sample_rate: 0,
                channels: 0,
            },
            audio_sync_offset_seconds: Rational::zero(),
            preview: PreviewSlice::default_window(),
        }
    }

    /// Given synthesized events, extract (FINISH tick, END tick).
    fn finish_and_end_ticks(events: &[SsqEvent]) -> (i32, i32) {
        let finish = events
            .iter()
            .find(|e| e.code == 2 && e.arg == 3)
            .expect("FINISH event missing");
        let end = events
            .iter()
            .find(|e| e.code == 2 && e.arg == 4)
            .expect("END event missing");
        (finish.tick, end.tick)
    }

    #[test]
    fn events_have_canonical_6_entry_shape() {
        let song = song_with_last_note_at(232448); // beat 227
        let (events, _) = synthesize_events(&song, &[]);
        assert_eq!(events.len(), 6);
        // MEASURE(4/4) at 0, READY at 0, GO at 4096, EDIT at 4096
        assert_eq!(events[0], SsqEvent { tick: 0,    code: 1, arg: 4 });
        assert_eq!(events[1], SsqEvent { tick: 0,    code: 2, arg: 1 });
        assert_eq!(events[2], SsqEvent { tick: 4096, code: 2, arg: 2 });
        assert_eq!(events[3], SsqEvent { tick: 4096, code: 2, arg: 5 });
        // FINISH and END are tested separately.
    }

    #[test]
    fn finish_sits_after_last_note_when_no_tempo_hint() {
        // No source tempo pairs — the SM5 path. FINISH must still be
        // strictly after the last note's measure boundary so the game
        // doesn't cut to results before the last note is played.
        let song = song_with_last_note_at(232448); // beat 227, last measure boundary = 233472
        let (events, _) = synthesize_events(&song, &[]);
        let (finish, end) = finish_and_end_ticks(&events);
        assert!(finish > 232448, "FINISH must be past last note (got {finish})");
        assert!(end > finish, "END must be past FINISH (got end={end}, finish={finish})");
    }

    #[test]
    fn end_adopts_source_trailing_tempo_tick_when_far_enough() {
        // Source has a trailing tempo entry past last note + 2 measures.
        // After modernize, raw_tempo_pairs[-1].0 = 254976 (beat 249).
        // last note = 232448 (beat 227). desired_end = 232448 + 8192 = 240640.
        // Because source trailing tick 254976 > 240640, END adopts 254976.
        let song = song_with_last_note_at(232448);
        let raw_pairs = vec![
            (0, 53),
            (8192, 3387),
            (135168, 55053),
            (221184, 90053),
            (225280, 91840),
            (228352, 93053),
            (232448, 94773),
            (254976, 103747),
        ];
        let (events, tempo_pairs) = synthesize_events(&song, &raw_pairs);
        let (finish, end) = finish_and_end_ticks(&events);
        assert_eq!(end, 254976, "END should adopt the source trailing tempo tick");
        assert_eq!(finish, end - 4096, "FINISH should be one measure before END");
        // No new pair should have been appended — the source already had a sufficient guard.
        assert_eq!(tempo_pairs, raw_pairs);
    }

    #[test]
    fn appends_trailing_tempo_pair_when_source_ends_at_last_note() {
        // Source's last tempo pair coincides with the last note.
        // We must synthesize a new trailing entry past FINISH so FINISH gets
        // bracketed by two consumed TIMING entries in the game's walk.
        // Using last_note = 110592 (beat 108, measure-aligned) keeps the
        // arithmetic simple: last_measure = 110592.
        let song = song_with_last_note_at(110592); // beat 108, on measure boundary
        // Source tempo: 120 BPM throughout, ending at the last note.
        // 120 BPM, TPS=1000 → 500 seconds-ticks per beat.
        // At beat 108 = 110592 measure-ticks, seconds-ticks = 108 * 500 = 54000.
        let raw_pairs = vec![(0, 0), (110592, 54000)];
        let (events, tempo_pairs) = synthesize_events(&song, &raw_pairs);
        let (finish, end) = finish_and_end_ticks(&events);
        // last_measure = 110592 (already measure-aligned)
        // desired_end = 110592 + 8192 = 118784
        // source trailing 110592 < 118784, so end_tick = 118784.
        assert_eq!(end, 118784);
        assert_eq!(finish, end - 4096);
        // A new trailing tempo pair should have been appended at end_tick.
        assert_eq!(tempo_pairs.len(), raw_pairs.len() + 1);
        assert_eq!(tempo_pairs[tempo_pairs.len() - 1].0, 118784);
        // Extrapolated seconds-ticks at 120 BPM: prior pair had 54000 at tick 110592.
        // Over 8192 measure-ticks at 120 BPM, that's 4000 seconds-ticks.
        // So new seconds-ticks = 54000 + 4000 = 58000.
        assert_eq!(tempo_pairs[tempo_pairs.len() - 1].1, 58000);
    }

    #[test]
    fn appends_trailing_tempo_pair_when_source_has_small_gap() {
        // Source's last tempo pair is only 1 measure past the
        // last note — not enough to bracket FINISH at last_note + 1 measure.
        // Using measure-aligned last_note = 155648 (beat 152, 152/4 = 38 measures).
        let song = song_with_last_note_at(155648); // beat 152
        // Source tempo ending at beat 156 (last_note + 1 measure). 120 BPM.
        // At tick 159744, seconds-ticks = 156 * 500 = 78000.
        let raw_pairs = vec![(0, 0), (159744, 78000)];
        let (events, tempo_pairs) = synthesize_events(&song, &raw_pairs);
        let (finish, end) = finish_and_end_ticks(&events);
        // last_measure = 155648 (already measure-aligned)
        // desired_end = 155648 + 8192 = 163840
        // source trailing 159744 < 163840, so end_tick = 163840.
        assert_eq!(end, 163840);
        assert_eq!(finish, end - 4096);
        // A new trailing pair should have been appended.
        assert_eq!(tempo_pairs.last().copied(), Some((163840, 80000)));
    }

    #[test]
    fn finish_is_bracketed_by_two_tempo_entries_in_all_cases() {
        // This is the critical invariant: regardless of where the source
        // tempo ends, FINISH must sit strictly between two tempo ticks
        // in the emitted tempo pairs so the game's musicCount
        // interpolation assigns it a valid value.
        let cases = [
            // (last_note, raw_pairs)
            (232448, vec![(0, 0), (254976, 100000)]), // source past desired end
            (110592, vec![(0, 0), (110592, 54000)]),  // source at last note
            (155648, vec![(0, 0), (159744, 78000)]),  // source with small gap
            (4096, vec![(0, 0), (4096, 2000)]),       // very short song
        ];
        for (last_note, raw_pairs) in cases {
            let song = song_with_last_note_at(last_note);
            let (events, tempo_pairs) = synthesize_events(&song, &raw_pairs);
            let (finish, _) = finish_and_end_ticks(&events);
            // Find the two tempo ticks that straddle FINISH.
            let before = tempo_pairs
                .iter()
                .rev()
                .find(|p| p.0 <= finish)
                .map(|p| p.0);
            let after = tempo_pairs.iter().find(|p| p.0 > finish).map(|p| p.0);
            assert!(
                before.is_some() && after.is_some(),
                "FINISH at {finish} not bracketed for last_note={last_note}: before={before:?}, after={after:?}, pairs={tempo_pairs:?}"
            );
        }
    }

    #[test]
    fn end_coincides_with_last_tempo_pair_tick() {
        // The hand-authored reference pattern: END's tick matches the
        // last tempo pair exactly. This produces the notes-vector shape
        // where only END (and not FINISH) ends up with INT32_MIN
        // musicCount after the game's postprocessing — matching known-
        // working files.
        let cases = [
            (232448, vec![(0, 0), (254976, 100000)]),
            (110592, vec![(0, 0), (110592, 54000)]),
            (155648, vec![(0, 0), (159744, 78000)]),
        ];
        for (last_note, raw_pairs) in cases {
            let song = song_with_last_note_at(last_note);
            let (events, tempo_pairs) = synthesize_events(&song, &raw_pairs);
            let (_, end) = finish_and_end_ticks(&events);
            assert_eq!(
                end,
                tempo_pairs.last().unwrap().0,
                "END must equal last tempo tick for last_note={last_note}"
            );
        }
    }

    #[test]
    fn extend_tempo_pairs_extrapolates_linearly_from_last_segment() {
        // Last segment: (0, 0) → (1000, 500). Slope = 0.5 seconds-tick per tick.
        // Extending to tick 1500 should produce (1500, 750).
        let pairs = vec![(0, 0), (1000, 500)];
        let out = extend_tempo_pairs_to(&pairs, 1500);
        assert_eq!(out.len(), 3);
        assert_eq!(out[2], (1500, 750));
    }

    #[test]
    fn extend_tempo_pairs_with_single_pair_falls_back_gracefully() {
        // Only one pair — no segment to extrapolate from. Falls back to
        // "same seconds-tick" (non-crashing degenerate case).
        let pairs = vec![(0, 100)];
        let out = extend_tempo_pairs_to(&pairs, 5000);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1], (5000, 100));
    }

    #[test]
    fn synthesize_events_with_empty_song_handles_gracefully() {
        // No notes — fallback last_tick is 4096 (per the .unwrap_or in
        // synthesize_events). FINISH and END should still produce a valid
        // event sequence and not panic.
        let mut song = song_with_last_note_at(1024); // beat 1
        song.charts[0].notes.clear();
        let (events, _) = synthesize_events(&song, &[]);
        assert_eq!(events.len(), 6);
        let (finish, end) = finish_and_end_ticks(&events);
        assert!(end > finish);
    }
}
