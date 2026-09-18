//! Per-job conversion orchestrator.
//!
//! `run_one` reads inputs, dispatches to the right parser/writer per
//! `(from, to)` pair, and writes output files colocated with inputs.

pub mod batch;
pub mod se_bank;

use std::fs;
use std::path::{Path, PathBuf};

use log::{info, warn};
use thiserror::Error;

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

/// Sample rates accepted for DDR World song wave banks. The XWB entry
/// header carries the rate (18-bit field), and the game's audio engine
/// (`xactengine2_10`) reads it per wave and sample-rate-converts into its
/// fixed 44.1 kHz mix buffer. The chart clock in `gamemdx` is a wall-clock
/// delta and never consults the rate, so any rate the engine can decode
/// stays in sync. Stock content is 44.1 kHz; 48 kHz is the other rate
/// StepMania packs commonly ship at. Anything else is almost certainly a
/// mistake (a downsampled preview, a voice clip) and is refused.
const DDR_SAMPLE_RATES: [u32; 2] = [44_100, 48_000];
/// Channel count DDR World's song wave banks are authored at. The ADPCM
/// encoder de-interleaves by this count, so any other layout would be
/// scrambled.
const DDR_CHANNELS: u16 = 2;

/// Orchestration-level failures that are not attributable to one format
/// module.
#[derive(Debug, Error)]
pub enum JobError {
    #[error(
        "DDR audio must be one of {DDR_SAMPLE_RATES:?} Hz, got {sample_rate} Hz \
         (resample the source before converting — this tool will not do it silently)"
    )]
    UnsupportedSampleRate { sample_rate: u32 },

    #[error(
        "DDR audio must have {DDR_CHANNELS} channels, got {channels} \
         (remix the source before converting — this tool will not do it silently)"
    )]
    WrongChannelCount { channels: u16 },
}

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

    let ssc_path = output_path(job, "ssc");
    let ogg_path = output_path(job, "ogg");
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

    let code = resolve_song_code(job);
    let ssq_path = output_path(job, "ssq");
    let xwb_path = output_path(job, "xwb");
    let xsb_path = output_path(job, "xsb");
    check_overwrite(&ssq_path, job.overwrite)?;
    check_overwrite(&xwb_path, job.overwrite)?;
    check_overwrite(&xsb_path, job.overwrite)?;

    // SSQ — synthesize tempo pairs from the Song's semantic view, with
    // the trailing pair placed exactly where `synthesize_events` will
    // put END. Two things depend on that trailing pair: it is what makes
    // the final `#BPMS` entry's tempo derivable at all (SSQ encodes BPM
    // as the slope between consecutive pairs), and it brackets FINISH
    // between two TIMING entries — without which the game locks at
    // READY (see `synthesize_events`).
    let end_tick = chart_end_tick(&song);
    let end_beat = crate::model::Beat::from_measure_ticks(i64::from(end_tick))
        .map_err(|e| ssq::SsqError::Write(format!("end beat: {e}")))?;
    let initial_tempo_pairs = ssq::writer::synthesize_tempo_entries_until(&song, Some(end_beat))?;
    let (events, tempo_pairs) = synthesize_events(&song, &initial_tempo_pairs);
    let mut ssq_out = Vec::new();
    ssq::writer::write(&song, &events, &tempo_pairs, &mut ssq_out)?;
    fs::write(&ssq_path, &ssq_out)?;
    info!("wrote {}", ssq_path.display());

    // XWB + XSB.
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

    let ssq_path = output_path(job, "ssq");
    let xwb_path = output_path(job, "xwb");
    let xsb_path = output_path(job, "xsb");
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
    ssq::writer::write(&result.song, &events, &tempo_pairs, &mut ssq_out)?;
    fs::write(&ssq_path, &ssq_out)?;
    info!("wrote {}", ssq_path.display());

    // Audio: try passthrough, else decode + re-encode.
    if try_audio_passthrough(&audio_bytes, &job.audio_in, &xwb_path, &xsb_path)? {
        info!("audio passthrough (XWB+XSB byte-copied)");
    } else {
        let audio = decode_legacy_audio(&audio_bytes)?;
        result.song.audio = audio;
        let code = resolve_song_code(job);
        write_ddr_audio(
            &result.song.audio,
            &result.song.preview,
            &code,
            &xwb_path,
            &xsb_path,
        )?;
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

    let ssc_path = output_path(job, "ssc");
    let ogg_path = output_path(job, "ogg");
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
/// audio-sync state. Applied to both `audio_sync_offset_seconds` (which the
/// SSC writer negates into `#OFFSET`) and `raw_tempo_pairs[0].1` (emitted
/// verbatim by the SSQ writer as `tempo_data[0]`). Both carry the DDR sign
/// convention — positive = beat 0 later in the audio — so the same signed
/// value is added to each. Modernize runs first, so `song.tps` is already
/// 1000 and seconds-ticks are directly in milliseconds.
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

/// Derive output path: the job's output basename with the given
/// extension, placed in the job's output directory. See [`output_stem`].
fn output_path(job: &Job, ext: &str) -> PathBuf {
    // Not `Path::with_extension`: a stem like `$1.78` would lose its
    // `.78` as though it were an extension.
    job.output_dir.join(format!("{}.{ext}", output_stem(job)))
}

/// Basename shared by every file a job writes: the explicit `--song-code`
/// when given, else the input chart's stem. For Ultramix inputs the
/// `_all` suffix is stripped (`abs2_all.ssq` → `abs2.ssc`) so output
/// filenames match the canonical per-song ID the game uses to find
/// assets.
fn output_stem(job: &Job) -> String {
    match &job.song_code {
        Some(code) => code.clone(),
        None => input_stem(&job.chart_in).to_string(),
    }
}

/// The input chart's file stem with any Ultramix `_all` suffix removed.
fn input_stem(chart_path: &Path) -> &str {
    let stem = chart_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    stem.strip_suffix("_all").unwrap_or(stem)
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

/// Resolve the DDR song code for a job: the name of the XACT wave bank
/// and of its two cues (`{code}` main, `{code}_s` preview).
///
/// DDR World plays a song by asking XACT for the cue named after the
/// song's ID, compared byte-for-byte (case included). The bank therefore
/// only produces sound when this code equals the ID the files are
/// installed under — which is also their basename. So the code *is* the
/// output basename: `--song-code` when given, else the input stem.
///
/// When the stem cannot be a code (spaces, punctuation, more than 16
/// characters) the bank is still written, with a best-effort code, so
/// the chart can be inspected — but the audio will be silent in-game
/// and the user is told what to rename.
fn resolve_song_code(job: &Job) -> String {
    let stem = output_stem(job);
    if xsb::is_valid_code(&stem) {
        return stem;
    }
    let fallback: String = stem
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(SONG_CODE_FALLBACK_LEN)
        .collect();
    let code = if fallback.is_empty() {
        "song".to_string()
    } else {
        fallback
    };
    warn!(
        "{}: basename {stem:?} cannot name a DDR wave bank (need 1-16 ASCII letters/digits); \
         audio was written under code {code:?} and the game will NOT play it unless the song \
         is installed as {code}.ssq/.xwb/.xsb. Name the input pair after the song's ID, or \
         pass --song-code <ID> in single-file mode.",
        job.chart_in.display(),
    );
    code
}

/// Length of the best-effort code derived from a basename that is not a
/// valid code in its own right. Stock DDR IDs are 4–5 characters.
const SONG_CODE_FALLBACK_LEN: usize = 4;

/// Detect and decode legacy audio (XWB or WAVM) by header inspection.
fn decode_legacy_audio(bytes: &[u8]) -> Result<AudioBuffer, Error> {
    // Try XWB first (has "WBND" magic).
    if bytes.len() >= 4 && &bytes[..4] == b"WBND" {
        return Ok(xwb::parse_audio(bytes)?);
    }
    // Fall back to WAVM (headerless XBOX-IMA).
    Ok(wavm::parse(bytes)?)
}

/// DDR-format WaveFormat: ADPCM, 2ch, raw_align=48, at the given sample
/// rate. Only the rate varies; codec, layout, and block alignment are the
/// fixed profile DDR's authoring tool emits.
fn ddr_wave_format(sample_rate: u32) -> WaveFormat {
    WaveFormat::from_packed(2 | ((DDR_CHANNELS as u32) << 2) | (sample_rate << 5) | (48 << 23))
}

/// Whether a wave-format sample rate is one this tool will put in a DDR
/// song bank. See [`DDR_SAMPLE_RATES`] for why the set is closed.
fn is_accepted_ddr_rate(sample_rate: u32) -> bool {
    DDR_SAMPLE_RATES.contains(&sample_rate)
}

/// Reject PCM whose rate or layout can't be encoded into a DDR song bank.
/// The header is derived from the buffer's rate, so a rate mismatch is
/// not a speed bug any more — the check exists to refuse inputs that are
/// almost certainly wrong (see [`DDR_SAMPLE_RATES`]) and to keep the
/// channel layout the ADPCM encoder assumes.
fn validate_ddr_audio(audio: &AudioBuffer) -> Result<(), JobError> {
    if !is_accepted_ddr_rate(audio.sample_rate) {
        return Err(JobError::UnsupportedSampleRate {
            sample_rate: audio.sample_rate,
        });
    }
    if audio.channels != DDR_CHANNELS {
        return Err(JobError::WrongChannelCount {
            channels: audio.channels,
        });
    }
    Ok(())
}

/// Encode audio + preview and write XWB + XSB.
fn write_ddr_audio(
    audio: &AudioBuffer,
    preview: &PreviewSlice,
    code: &str,
    xwb_path: &Path,
    xsb_path: &Path,
) -> Result<(), Error> {
    validate_ddr_audio(audio)?;
    let fmt = ddr_wave_format(audio.sample_rate);
    info!(
        "encoding {} Hz {}ch PCM to MS-ADPCM (bank declares {} Hz); wave bank and cues named {code:?}",
        audio.sample_rate,
        audio.channels,
        fmt.sample_rate()
    );

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
fn build_xwb_bank(code: &str, fmt: &WaveFormat, main_data: &[u8], preview_data: &[u8]) -> XwbBank {
    let spb = fmt.samples_per_block() as usize;
    let ba = fmt.block_align() as usize;

    // `checked_div` guards a zero block-align (a degenerate WaveFormat), which
    // yields a zero duration rather than dividing by zero.
    let main_duration = main_data.len().checked_div(ba).unwrap_or(0) * spb;
    let preview_duration = preview_data.len().checked_div(ba).unwrap_or(0) * spb;

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

    // Both entries must be DDR-profile ADPCM at an accepted rate: the
    // codec/layout/alignment must equal what we'd write ourselves for
    // that rate, so anything the encoder wouldn't produce is re-encoded.
    for entry in &bank.entries {
        let rate = entry.format.sample_rate();
        if !is_accepted_ddr_rate(rate) || entry.format != ddr_wave_format(rate) {
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

/// Measure-tick where END belongs: two whole measures past the measure
/// containing the last note (a hold counts to its tail). Songs with no
/// notes at all are treated as ending one measure in.
fn chart_end_tick(song: &crate::model::Song) -> i32 {
    use crate::model::NoteKind;

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
    last_measure + 8192 // last note + 2 measures
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
/// source's trailing tempo pair is already at or past
/// [`chart_end_tick`], we adopt its tick as END; otherwise we
/// extrapolate a new trailing pair there using the last segment's
/// slope. The SM5→DDR path pre-places its trailing pair at
/// `chart_end_tick` with exact tempo math, so extrapolation is only
/// ever exercised for legacy sources.
fn synthesize_events(
    song: &crate::model::Song,
    raw_tempo_pairs: &[(i32, i32)],
) -> (Vec<SsqEvent>, Vec<(i32, i32)>) {
    let desired_end = chart_end_tick(song);

    // Build the tempo-pair list that goes into the SSQ. Invariant:
    // the final pair's time_offset == end_tick, so END coincides with
    // the trailing TIMING note and FINISH (one measure earlier) falls
    // strictly inside the last real tempo bracket.
    let source_last_tick = raw_tempo_pairs.last().map(|p| p.0).unwrap_or(0);
    let end_tick = std::cmp::max(desired_end, source_last_tick);
    let finish_tick = end_tick - 4096;

    let tempo_pairs = if raw_tempo_pairs.is_empty() {
        // Defensive fallback: no callers currently pass empty pairs
        // (legacy_to_ddr has source pairs; sm5_to_ddr pre-synthesizes
        // pairs before calling us). Kept for test-robustness.
        Vec::new()
    } else if source_last_tick >= end_tick {
        raw_tempo_pairs.to_vec()
    } else {
        extend_tempo_pairs_to(raw_tempo_pairs, end_tick)
    };

    let events = vec![
        SsqEvent {
            tick: 0,
            code: 1,
            arg: 4,
        },
        SsqEvent {
            tick: 0,
            code: 2,
            arg: 1,
        },
        SsqEvent {
            tick: 4096,
            code: 2,
            arg: 2,
        },
        SsqEvent {
            tick: 4096,
            code: 2,
            arg: 5,
        },
        SsqEvent {
            tick: finish_tick,
            code: 2,
            arg: 3,
        },
        SsqEvent {
            tick: end_tick,
            code: 2,
            arg: 4,
        },
    ];

    (events, tempo_pairs)
}

/// Append a synthesized trailing tempo pair at `end_tick`, extrapolating
/// seconds-ticks linearly from the last non-stop tempo segment.
///
/// "Non-stop" means a pair-pair interval where consecutive pairs have
/// distinct measure-ticks (`dt > 0`). Stops are encoded as a pair-pair
/// with `dt == 0` and a non-zero seconds-tick delta — they represent a
/// pause in the audio timeline rather than a rate, so they can't be
/// used to extrapolate continuing tempo. When the input's final pair
/// is the closing pair of a stop, we walk backward to the last segment
/// with `dt > 0` and use its rate.
///
/// Extrapolation is always applied on top of the *last pair's*
/// seconds-ticks (not the referenced segment's s0), so any stop delays
/// that preceded the last pair are correctly preserved.
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
    // Walk backward to find the last pair-pair interval with dt > 0.
    // This skips over stop pair-pairs (dt == 0) at the tail.
    let rate_segment = (1..n).rev().find(|&i| out[i].0 != out[i - 1].0);
    let (t0, s0, t1, s1) = match rate_segment {
        Some(i) => (out[i - 1].0, out[i - 1].1, out[i].0, out[i].1),
        None => {
            // All pairs share the same tick — pathological input; keep
            // seconds-tick constant so callers don't choke.
            if let Some(last) = out.last().copied() {
                out.push((end_tick, last.1));
            }
            return out;
        }
    };
    let dt = (t1 - t0) as i64;
    let ds = (s1 - s0) as i64;
    let last_tick = out[n - 1].0 as i64;
    let last_seconds = out[n - 1].1 as i64;
    let extra_ticks = end_tick as i64 - last_tick;
    // dt > 0 is guaranteed by the find() predicate above.
    let extra_seconds = (ds * extra_ticks + dt / 2) / dt;
    let new_s = (last_seconds + extra_seconds).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
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

    // ---------- song code / output naming ----------

    fn job_for(chart: &str, song_code: Option<&str>) -> Job {
        Job {
            from: Format::Sm5,
            to: Format::Ddr,
            chart_in: PathBuf::from(chart),
            audio_in: PathBuf::from("x.ogg"),
            overwrite: false,
            output_dir: PathBuf::from("out"),
            sync_offset_ms: 0,
            song_code: song_code.map(str::to_string),
        }
    }

    #[test]
    fn song_code_is_the_whole_basename_when_it_can_be() {
        // The game plays the cue named after the installed basename, so
        // the two must be identical — including case and length. The old
        // 4-character truncation silently broke every 5-character ID.
        assert_eq!(resolve_song_code(&job_for("muka.ssc", None)), "muka");
        assert_eq!(resolve_song_code(&job_for("bknh2.ssq", None)), "bknh2");
        assert_eq!(resolve_song_code(&job_for("Muka.ssc", None)), "Muka");
        assert_eq!(resolve_song_code(&job_for("abs2_all.ssq", None)), "abs2");
    }

    #[test]
    fn explicit_song_code_names_bank_and_output_files() {
        let job = job_for("Mukade.ssc", Some("muka"));
        assert_eq!(resolve_song_code(&job), "muka");
        assert_eq!(output_path(&job, "xwb"), PathBuf::from("out/muka.xwb"));
        assert_eq!(output_path(&job, "ssq"), PathBuf::from("out/muka.ssq"));
    }

    #[test]
    fn unusable_basename_falls_back_to_a_short_code() {
        // Still produces *something* (so the SSQ can be inspected) but
        // the caller logs that it will not play under this filename.
        assert_eq!(
            resolve_song_code(&job_for("A Is For Action.ssc", None)),
            "AIsF"
        );
        assert_eq!(resolve_song_code(&job_for("$1.78.ssc", None)), "178");
        assert_eq!(resolve_song_code(&job_for("!!!.ssc", None)), "song");
        assert_eq!(
            output_path(&job_for("A Is For Action.ssc", None), "ssq"),
            PathBuf::from("out/A Is For Action.ssq"),
            "output filename still follows the input when no code is given"
        );
    }

    #[test]
    fn output_stem_keeps_dots_that_are_not_the_extension() {
        assert_eq!(
            output_path(&job_for("$1.78.ssc", None), "ssq"),
            PathBuf::from("out/$1.78.ssq")
        );
    }

    // ---------- validate_ddr_audio ----------

    fn audio(sample_rate: u32, channels: u16) -> AudioBuffer {
        AudioBuffer {
            samples: vec![0; 256 * channels as usize],
            sample_rate,
            channels,
        }
    }

    #[test]
    fn ddr_audio_accepts_44100_stereo() {
        assert!(validate_ddr_audio(&audio(44_100, 2)).is_ok());
    }

    #[test]
    fn ddr_audio_accepts_48000_stereo() {
        // 48 kHz is carried natively: the header declares it and the
        // engine resamples at playback. Previously this was mislabelled
        // as 44.1 kHz and played ~9% slow.
        assert!(validate_ddr_audio(&audio(48_000, 2)).is_ok());
    }

    #[test]
    fn ddr_audio_rejects_unlisted_rate() {
        let err = validate_ddr_audio(&audio(22_050, 2)).unwrap_err();
        assert!(
            matches!(
                err,
                JobError::UnsupportedSampleRate {
                    sample_rate: 22_050
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn ddr_audio_rejects_mono() {
        let err = validate_ddr_audio(&audio(44_100, 1)).unwrap_err();
        assert!(
            matches!(err, JobError::WrongChannelCount { channels: 1 }),
            "got {err:?}"
        );
    }

    #[test]
    fn ddr_audio_reports_sample_rate_before_channels() {
        // Both wrong: the rate is the more likely root cause, so it is
        // what the user sees first.
        let err = validate_ddr_audio(&audio(22_050, 1)).unwrap_err();
        assert!(matches!(err, JobError::UnsupportedSampleRate { .. }));
    }

    #[test]
    fn ddr_wave_format_carries_buffer_rate() {
        // The header must declare the buffer's real rate — this is the
        // whole fix for the "plays slow" bug — and the fixed parts of
        // the profile must not change with it.
        for &rate in &DDR_SAMPLE_RATES {
            let fmt = ddr_wave_format(rate);
            assert_eq!(fmt.sample_rate(), rate);
            assert_eq!(u16::from(fmt.channels()), DDR_CHANNELS);
            assert_eq!(fmt.codec(), WaveFormat::CODEC_ADPCM);
            assert_eq!(fmt.block_align_raw(), 48);
            assert_eq!(fmt.samples_per_block(), 128);
        }
    }

    #[test]
    fn ddr_wave_format_48k_survives_container_round_trip() {
        // 48000 must fit the 18-bit rate field without clobbering
        // neighbouring fields once packed and unpacked.
        let fmt = ddr_wave_format(48_000);
        let again = WaveFormat::from_packed(fmt.packed());
        assert_eq!(again, fmt);
        assert_eq!(again.sample_rate(), 48_000);
        assert_eq!(again.block_align(), 140);
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
        assert_eq!(
            events[0],
            SsqEvent {
                tick: 0,
                code: 1,
                arg: 4
            }
        );
        assert_eq!(
            events[1],
            SsqEvent {
                tick: 0,
                code: 2,
                arg: 1
            }
        );
        assert_eq!(
            events[2],
            SsqEvent {
                tick: 4096,
                code: 2,
                arg: 2
            }
        );
        assert_eq!(
            events[3],
            SsqEvent {
                tick: 4096,
                code: 2,
                arg: 5
            }
        );
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
        assert!(
            finish > 232448,
            "FINISH must be past last note (got {finish})"
        );
        assert!(
            end > finish,
            "END must be past FINISH (got end={end}, finish={finish})"
        );
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
        assert_eq!(
            end, 254976,
            "END should adopt the source trailing tempo tick"
        );
        assert_eq!(
            finish,
            end - 4096,
            "FINISH should be one measure before END"
        );
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
    fn extend_tempo_pairs_extrapolates_past_trailing_stop() {
        // Regression: source pairs end with a stop (same-tick pair with
        // non-zero seconds-tick delta). The last *segment* with a real
        // rate is pairs[n-3] → pairs[n-2]; we must use that to
        // extrapolate past the stop's closing pair.
        //
        // Shape modelled on the Xuxa (SM5→DDR) chart that uncovered this
        // bug: a BPM=160 segment followed by a 0.375s stop, then the
        // caller asks for extension past the stop to the END tick.
        //
        // pairs[-3]: (317440, 117369)  — start of last BPM=160 segment
        // pairs[-2]: (318464, 117744)  — end of that segment
        // pairs[-1]: (318464, 118119)  — stop-end (same tick, +375 ticks)
        //
        // Extending to tick 331776:
        //   last segment rate: ds/dt = 375/1024 per measure-tick
        //   extra_ticks = 331776 - 318464 = 13312
        //   extra_seconds = 13312 * 375 / 1024 = 4875
        //   new pair's seconds-ticks = 118119 + 4875 = 122994
        let pairs = vec![(317440, 117369), (318464, 117744), (318464, 118119)];
        let out = extend_tempo_pairs_to(&pairs, 331776);
        assert_eq!(out.len(), 4);
        assert_eq!(out[3], (331776, 122994));
    }

    #[test]
    fn extend_tempo_pairs_all_same_tick_falls_back_gracefully() {
        // Pathological: every pair has the same measure-tick (no
        // non-stop segment anywhere). We can't extrapolate a rate;
        // fall back to keeping seconds-tick constant.
        let pairs = vec![(500, 100), (500, 200), (500, 300)];
        let out = extend_tempo_pairs_to(&pairs, 1000);
        assert_eq!(out.len(), 4);
        assert_eq!(out[3], (1000, 300));
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

    #[test]
    fn sm5_to_ddr_flow_places_trailing_pair_at_end_with_final_bpm() {
        // The SM5→DDR job pre-computes END's tick and asks the writer to
        // place the trailing tempo pair there with exact tempo math, so
        // `synthesize_events` adopts the pairs unchanged and the slope
        // into END is the *final* `#BPMS` entry — not an extrapolation
        // of whichever segment happened to precede the last pair (the
        // Mukade / "media offline" bug: 1280 or 348 BPM to the end).
        use crate::model::{Bpm, Stop, TempoSegment};
        let seg = |beat: i64, bpm: i64| TempoSegment {
            start_beat: Beat::from_rational(Rational::from_integer(beat)),
            bpm: Bpm::from_rational(Rational::from_integer(bpm)),
        };
        let mut song = song_with_last_note_at(324 * 1024);
        song.tempo_segments = vec![seg(0, 160), seg(180, 1280), seg(196, 160)];
        song.stops = vec![Stop {
            at_beat: Beat::from_rational(Rational::from_integer(180)),
            duration_seconds: Rational::new(9, 4).unwrap(),
        }];

        let end_tick = chart_end_tick(&song);
        assert_eq!(end_tick, 324 * 1024 + 8192);
        let end_beat = Beat::from_measure_ticks(i64::from(end_tick)).unwrap();
        let initial = crate::ssq::writer::synthesize_tempo_entries_until(&song, Some(end_beat))
            .expect("tempo synthesis");
        let (events, pairs) = synthesize_events(&song, &initial);
        let (_, end) = finish_and_end_ticks(&events);

        assert_eq!(pairs, initial, "no extrapolated pair should be appended");
        assert_eq!(pairs.last().unwrap().0, end);
        let n = pairs.len();
        let (a, b) = (pairs[n - 2], pairs[n - 1]);
        let bpm = 240.0 * 1000.0 * f64::from(b.0 - a.0) / (4096.0 * f64::from(b.1 - a.1));
        assert!(
            (bpm - 160.0).abs() < 1e-9,
            "final slope must be 160 BPM, got {bpm}"
        );
    }

    #[test]
    fn sm5_to_ddr_flow_brackets_finish_end_to_end() {
        // SM5→DDR pre-synthesizes tempo pairs via
        // `ssq::writer::synthesize_tempo_entries`, then feeds them into
        // `synthesize_events`. The writer's synthesis places its
        // trailing entry at the last-note beat, which without the
        // job-layer extension would leave FINISH unbracketed. This test
        // locks in the full end-to-end flow.
        use crate::model::{Bpm, TempoSegment};
        let mut song = song_with_last_note_at(2937856); // beat 2869
        song.tempo_segments = vec![TempoSegment {
            start_beat: Beat::zero(),
            bpm: Bpm::from_rational(Rational::from_integer(200)),
        }];

        // Simulate the sm5_to_ddr job flow.
        let initial_pairs = crate::ssq::writer::synthesize_tempo_entries(&song)
            .expect("tempo synthesis should succeed with one segment");
        // The writer puts the trailing entry at the last-note beat —
        // same as the source of the original bug.
        assert_eq!(initial_pairs.last().unwrap().0, 2937856);

        let (events, tempo_pairs) = synthesize_events(&song, &initial_pairs);
        let (finish, _) = finish_and_end_ticks(&events);

        // Invariant: FINISH is strictly between two tempo entries.
        let before = tempo_pairs
            .iter()
            .rev()
            .find(|p| p.0 <= finish)
            .map(|p| p.0);
        let after = tempo_pairs.iter().find(|p| p.0 > finish).map(|p| p.0);
        assert!(
            before.is_some() && after.is_some(),
            "FINISH at {finish} not bracketed: before={before:?}, after={after:?}, pairs={tempo_pairs:?}"
        );
    }

    #[test]
    fn synthesize_events_with_mine_as_last_note_brackets_finish_correctly() {
        // Learning 11 invariant applied to mines: a chart whose last
        // note is a `NoteKind::Mine` (no trailing tap/hold) must
        // still produce FINISH bracketed by two TIMING notes.
        //
        // Before Task 3 wired mines into the parser, `last_tick`'s
        // filter clause only saw Tap/HoldHead/Shock. Now that mines
        // participate in `Chart.notes`, the `_ => n.beat` fallthrough
        // arm in `synthesize_events::last_tick` picks them up. This
        // test locks that down.
        use crate::model::{Bpm, TempoSegment};

        // Build a Song where the chart's ONLY note is a mine at a
        // late beat. No tap/hold at all.
        let mine_tick = 10240i32; // beat 10
        let mut song = song_with_last_note_at(i64::from(mine_tick));
        song.charts[0].notes = vec![Note {
            beat: Beat::from_measure_ticks(i64::from(mine_tick)).unwrap(),
            kind: NoteKind::Mine,
            panels: PanelSet::from_bits(Style::Single, 0x01),
        }];
        song.tempo_segments = vec![TempoSegment {
            start_beat: Beat::zero(),
            bpm: Bpm::from_rational(Rational::from_integer(120)),
        }];

        let initial_pairs = crate::ssq::writer::synthesize_tempo_entries(&song)
            .expect("tempo synthesis should succeed");
        // The writer's trailing entry should be at the mine's tick —
        // same as any other last-note scenario.
        assert_eq!(
            initial_pairs.last().unwrap().0,
            mine_tick,
            "tempo synthesis must include the mine's beat as max_chart_beat"
        );

        let (events, tempo_pairs) = synthesize_events(&song, &initial_pairs);
        let (finish, end) = finish_and_end_ticks(&events);

        // last_measure = ((mine_tick + 4095) / 4096) * 4096
        //              = ((10240 + 4095) / 4096) * 4096 = 3 * 4096 = 12288.
        // desired_end = last_measure + 8192 = 20480. finish = end - 4096.
        let last_measure = ((mine_tick + 4095) / 4096) * 4096;
        let desired_end = last_measure + 8192;
        assert_eq!(end, desired_end, "END at last_measure + 2 measures");
        assert_eq!(finish, desired_end - 4096, "FINISH one measure before END");

        // FINISH must be strictly bracketed by two tempo entries (Learning 11).
        let before = tempo_pairs
            .iter()
            .rev()
            .find(|p| p.0 <= finish)
            .map(|p| p.0);
        let after = tempo_pairs.iter().find(|p| p.0 > finish).map(|p| p.0);
        assert!(
            before.is_some(),
            "FINISH at {finish}: no tempo pair at or before it. pairs={tempo_pairs:?}"
        );
        assert!(
            after.is_some(),
            "FINISH at {finish}: no tempo pair after it. pairs={tempo_pairs:?}"
        );

        // Final tempo pair should sit at END's tick.
        assert_eq!(
            tempo_pairs.last().unwrap().0,
            end,
            "last tempo pair must coincide with END"
        );
    }
}
