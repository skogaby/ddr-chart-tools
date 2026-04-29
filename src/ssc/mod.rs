//! StepMania 5 SSC simfile parser.
//!
//! Accepts MSD-syntax `.ssc` files and decodes them into the common
//! `Song` model. Song-level tags populate the song; each `#NOTEDATA`
//! section becomes one `Chart`.

pub mod msd;
pub mod notes;

use std::io::Write;

use thiserror::Error;

use crate::model::{AudioBuffer, PreviewSlice, Rational, Song};

#[derive(Debug, Error)]
pub enum SscError {
    #[error("unsupported stepstype {0:?} (only dance-single and dance-double are handled)")]
    UnsupportedStepsType(String),

    #[error("unknown difficulty {0:?}")]
    UnknownDifficulty(String),

    #[error("Edit chart skipped")]
    EditChartSkipped,

    #[error("malformed note row at measure {measure}, row {row}: {reason}")]
    BadNoteRow {
        measure: usize,
        row: usize,
        reason: String,
    },

    #[error("hold head at panel {panel} (note index {note_index}) was never closed by a matching `3` tail")]
    UnclosedHold { panel: u8, note_index: usize },

    #[error("roll arrow (`4`) at measure {measure}, row {row}, panel {panel} is not supported")]
    UnsupportedRoll {
        measure: usize,
        row: usize,
        panel: u8,
    },

    #[error("invalid value for tag {tag}: {reason}")]
    InvalidValue { tag: String, reason: String },

    #[error("#NOTEDATA section is missing required tag #{tag}")]
    MissingRequiredNotedataTag { tag: String },

    #[error("I/O error while writing SSC: {0}")]
    Write(String),

    #[error("note at beat {num}/{den} is not representable at any standard SSC quantize in measure {measure}")]
    UnrepresentableBeat { measure: usize, num: i64, den: u64 },

    #[error("shock side {side:?} cannot be written without a P2 half (style is Single)")]
    ShockSideIncompatibleWithStyle { side: String },
}

/// Parse a complete SSC file into a `Song`.
pub fn parse(text: &str) -> Result<Song, SscError> {
    let values = msd::tokenize(text);
    let mut song = empty_song();
    let mut current_chart: Option<ChartDraft> = None;

    for value in &values {
        let tag = value.tag().to_ascii_uppercase();

        // `#NOTEDATA` starts a new chart section; flush the previous one.
        if tag == "NOTEDATA" {
            if let Some(draft) = current_chart.take() {
                if let Some(chart) = draft.finalize()? {
                    song.charts.push(chart);
                }
            }
            current_chart = Some(ChartDraft::default());
            continue;
        }

        if let Some(draft) = current_chart.as_mut() {
            draft.apply(&tag, value)?;
        } else if tag == "NOTES" && value.params.len() == 7 && current_chart.is_none() {
            // SM-style `#NOTES:stepstype:desc:diff:meter:radar:notedata;`
            // found at song level. Common in real-world .ssc files.
            let style = match notes::parse_stepstype(value.param(1)) {
                Ok(s) => s,
                Err(SscError::UnsupportedStepsType(t)) => {
                    log::warn!("skipping unsupported stepstype: {t}");
                    continue;
                }
                Err(e) => return Err(e),
            };
            let difficulty = match notes::parse_difficulty(value.param(3)) {
                Ok(d) => d,
                Err(SscError::EditChartSkipped) => {
                    log::warn!("skipping Edit chart");
                    continue;
                }
                Err(e) => return Err(e),
            };
            let parsed_notes = notes::parse_notes_body(value.param(6), style)?;
            song.charts.push(crate::model::Chart {
                style,
                difficulty,
                notes: parsed_notes,
            });
        } else {
            apply_song_tag(&mut song, &tag, value)?;
        }
    }

    if let Some(draft) = current_chart.take() {
        if let Some(chart) = draft.finalize()? {
            song.charts.push(chart);
        }
    }

    Ok(song)
}

/// Serialize a `Song` into an SSC file. Song-level tags are emitted
/// first, followed by one `#NOTEDATA` section per `Chart`.
///
/// Decimal fields (`#OFFSET`, `#BPMS`, `#STOPS`, `#SAMPLESTART`,
/// `#SAMPLELENGTH`) are written with 6 fractional digits — enough
/// precision that parse → write → parse preserves any value whose
/// rational denominator divides `10^6` (which all human-authored SSC
/// values do).
pub fn write(song: &Song, out: &mut impl Write) -> Result<(), SscError> {
    let io = |e: std::io::Error| SscError::Write(e.to_string());

    writeln!(out, "#VERSION:0.83;").map_err(io)?;
    write_str_tag(out, "TITLE", song.title.as_deref().unwrap_or(""))?;
    write_str_tag(out, "ARTIST", song.artist.as_deref().unwrap_or(""))?;
    write_decimal_tag(out, "OFFSET", song.audio_sync_offset_seconds)?;
    write_bpms(out, &song.tempo_segments)?;
    write_stops(out, &song.stops)?;
    write_decimal_tag(out, "SAMPLESTART", song.preview.start_seconds)?;
    write_decimal_tag(out, "SAMPLELENGTH", song.preview.length_seconds)?;

    for chart in &song.charts {
        notes::write_notedata(chart, out)?;
    }

    Ok(())
}

fn write_str_tag(out: &mut impl Write, tag: &str, value: &str) -> Result<(), SscError> {
    writeln!(out, "#{tag}:{value};").map_err(|e| SscError::Write(e.to_string()))
}

fn write_decimal_tag(out: &mut impl Write, tag: &str, value: Rational) -> Result<(), SscError> {
    writeln!(out, "#{tag}:{};", format_decimal(value)).map_err(|e| SscError::Write(e.to_string()))
}

fn write_bpms(
    out: &mut impl Write,
    segments: &[crate::model::TempoSegment],
) -> Result<(), SscError> {
    let io = |e: std::io::Error| SscError::Write(e.to_string());
    write!(out, "#BPMS:").map_err(io)?;
    for (i, seg) in segments.iter().enumerate() {
        if i > 0 {
            write!(out, ",").map_err(io)?;
        }
        write!(
            out,
            "{}={}",
            format_decimal(seg.start_beat.as_rational()),
            format_decimal(seg.bpm.as_rational())
        )
        .map_err(io)?;
    }
    writeln!(out, ";").map_err(io)
}

fn write_stops(out: &mut impl Write, stops: &[crate::model::Stop]) -> Result<(), SscError> {
    let io = |e: std::io::Error| SscError::Write(e.to_string());
    write!(out, "#STOPS:").map_err(io)?;
    for (i, stop) in stops.iter().enumerate() {
        if i > 0 {
            write!(out, ",").map_err(io)?;
        }
        write!(
            out,
            "{}={}",
            format_decimal(stop.at_beat.as_rational()),
            format_decimal(stop.duration_seconds)
        )
        .map_err(io)?;
    }
    writeln!(out, ";").map_err(io)
}

/// Format a `Rational` as a fixed-point decimal with 6 fractional digits.
/// Rounds half-to-even. Used for every decimal tag the writer emits.
pub(crate) fn format_decimal(r: Rational) -> String {
    const PRECISION: i128 = 1_000_000;
    let num = r.num() as i128;
    let den = r.den() as i128;
    let scaled = num * PRECISION;
    let q = scaled / den;
    let rem = scaled % den;
    let rounded = {
        let twice = rem.abs() * 2;
        if twice < den {
            q
        } else if twice > den {
            if scaled >= 0 {
                q + 1
            } else {
                q - 1
            }
        } else if q % 2 == 0 {
            q
        } else if scaled >= 0 {
            q + 1
        } else {
            q - 1
        }
    };
    let sign = if rounded < 0 { "-" } else { "" };
    let abs = rounded.unsigned_abs();
    let int_part = abs / (PRECISION as u128);
    let frac_part = abs % (PRECISION as u128);
    format!("{sign}{int_part}.{frac_part:06}")
}

pub(crate) fn empty_song() -> Song {
    Song {
        title: None,
        artist: None,
        tps: 1000,
        tempo_segments: Vec::new(),
        stops: Vec::new(),
        charts: Vec::new(),
        audio: AudioBuffer {
            samples: Vec::new(),
            sample_rate: 0,
            channels: 0,
        },
        audio_sync_offset_seconds: Rational::zero(),
        preview: PreviewSlice::default_window(),
    }
}

pub(crate) fn apply_song_tag(
    song: &mut Song,
    tag: &str,
    value: &msd::MsdValue,
) -> Result<(), SscError> {
    let v = value.param(1).trim();
    match tag {
        "TITLE" => {
            if !v.is_empty() {
                song.title = Some(v.to_string());
            }
        }
        "ARTIST" => {
            if !v.is_empty() {
                song.artist = Some(v.to_string());
            }
        }
        "OFFSET" => {
            song.audio_sync_offset_seconds = parse_decimal_seconds(tag, v)?;
        }
        "BPMS" => {
            song.tempo_segments = parse_bpms(v)?;
        }
        "STOPS" => {
            song.stops = parse_stops(v)?;
        }
        "SAMPLESTART" => {
            song.preview.start_seconds = parse_decimal_seconds(tag, v)?;
        }
        "SAMPLELENGTH" => {
            song.preview.length_seconds = parse_decimal_seconds(tag, v)?;
        }
        _ => {
            // Unknown song-level tags are silently ignored. SSC files
            // carry many optional tags (CDTITLE, BACKGROUND, etc.) that
            // aren't represented in our common model.
        }
    }
    Ok(())
}

/// Parse a decimal string like `"0.123"` or `"-4.5"` into an exact
/// `Rational` by treating the fractional part as `num / 10^digits`.
fn parse_decimal_seconds(tag: &str, s: &str) -> Result<Rational, SscError> {
    decimal_to_rational(s).ok_or_else(|| SscError::InvalidValue {
        tag: tag.to_string(),
        reason: format!("not a decimal number: {s:?}"),
    })
}

fn decimal_to_rational(s: &str) -> Option<Rational> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Some(Rational::zero());
    }
    let (sign, rest) = match trimmed.strip_prefix('-') {
        Some(r) => (-1i64, r),
        None => (1i64, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    let (int_part, frac_part) = match rest.split_once('.') {
        Some((a, b)) => (a, b),
        None => (rest, ""),
    };
    let int_val: i64 = if int_part.is_empty() {
        0
    } else {
        int_part.parse().ok()?
    };
    let frac_digits = frac_part.len();
    let frac_val: i64 = if frac_part.is_empty() {
        0
    } else {
        frac_part.parse().ok()?
    };
    let pow = 10i64.checked_pow(frac_digits as u32)?;
    let num = sign
        .checked_mul(int_val.checked_mul(pow)?.checked_add(frac_val)?)
        .unwrap_or(0);
    Rational::new(num, pow).ok()
}

/// Parse `"beat=bpm,beat=bpm,..."` into `TempoSegment`s.
fn parse_bpms(s: &str) -> Result<Vec<crate::model::TempoSegment>, SscError> {
    use crate::model::{Beat, Bpm, TempoSegment};
    if s.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for (i, entry) in s.split(',').enumerate() {
        let (beat_s, bpm_s) = entry
            .split_once('=')
            .ok_or_else(|| SscError::InvalidValue {
                tag: "BPMS".to_string(),
                reason: format!("entry {i} missing `=`: {entry:?}"),
            })?;
        let beat_r = decimal_to_rational(beat_s).ok_or_else(|| SscError::InvalidValue {
            tag: "BPMS".to_string(),
            reason: format!("entry {i} beat is not a decimal: {beat_s:?}"),
        })?;
        let bpm_r = decimal_to_rational(bpm_s).ok_or_else(|| SscError::InvalidValue {
            tag: "BPMS".to_string(),
            reason: format!("entry {i} bpm is not a decimal: {bpm_s:?}"),
        })?;
        out.push(TempoSegment {
            start_beat: Beat::from_rational(beat_r),
            bpm: Bpm::from_rational(bpm_r),
        });
    }
    Ok(out)
}

/// Parse `"beat=duration,beat=duration,..."` into `Stop`s.
fn parse_stops(s: &str) -> Result<Vec<crate::model::Stop>, SscError> {
    use crate::model::{Beat, Stop};
    if s.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for (i, entry) in s.split(',').enumerate() {
        let (beat_s, dur_s) = entry
            .split_once('=')
            .ok_or_else(|| SscError::InvalidValue {
                tag: "STOPS".to_string(),
                reason: format!("entry {i} missing `=`: {entry:?}"),
            })?;
        let beat_r = decimal_to_rational(beat_s).ok_or_else(|| SscError::InvalidValue {
            tag: "STOPS".to_string(),
            reason: format!("entry {i} beat is not a decimal: {beat_s:?}"),
        })?;
        let dur_r = decimal_to_rational(dur_s).ok_or_else(|| SscError::InvalidValue {
            tag: "STOPS".to_string(),
            reason: format!("entry {i} duration is not a decimal: {dur_s:?}"),
        })?;
        out.push(Stop {
            at_beat: Beat::from_rational(beat_r),
            duration_seconds: dur_r,
        });
    }
    Ok(out)
}

/// Accumulates the fields of one `#NOTEDATA` section until the next
/// section begins or parsing ends.
#[derive(Debug, Default)]
struct ChartDraft {
    stepstype: Option<crate::model::Style>,
    difficulty: Option<crate::model::Difficulty>,
    notes_body: Option<String>,
    /// If this chart was an Edit difficulty or otherwise skipped, this
    /// flag suppresses the usual missing-required-tag errors on
    /// finalization.
    skip: bool,
}

impl ChartDraft {
    fn apply(&mut self, tag: &str, value: &msd::MsdValue) -> Result<(), SscError> {
        let v = value.param(1);
        match tag {
            "STEPSTYPE" => {
                self.stepstype = Some(notes::parse_stepstype(v)?);
            }
            "DIFFICULTY" => match notes::parse_difficulty(v) {
                Ok(d) => self.difficulty = Some(d),
                Err(SscError::EditChartSkipped) => {
                    log::warn!("skipping Edit chart");
                    self.skip = true;
                }
                Err(e) => return Err(e),
            },
            "NOTES" | "NOTES2" => {
                self.notes_body = Some(v.to_string());
            }
            _ => {
                // Other per-chart tags (METER, RADARVALUES, CHARTNAME,
                // CREDIT, chart-local timing overrides, etc.) are
                // ignored for now.
            }
        }
        Ok(())
    }

    fn finalize(self) -> Result<Option<crate::model::Chart>, SscError> {
        if self.skip {
            return Ok(None);
        }
        let style = self
            .stepstype
            .ok_or_else(|| SscError::MissingRequiredNotedataTag {
                tag: "STEPSTYPE".to_string(),
            })?;
        let difficulty = self
            .difficulty
            .ok_or_else(|| SscError::MissingRequiredNotedataTag {
                tag: "DIFFICULTY".to_string(),
            })?;
        let notes_body = self
            .notes_body
            .ok_or_else(|| SscError::MissingRequiredNotedataTag {
                tag: "NOTES".to_string(),
            })?;
        let notes = notes::parse_notes_body(&notes_body, style)?;
        Ok(Some(crate::model::Chart {
            style,
            difficulty,
            notes,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Bpm, Difficulty, NoteKind, Style};

    #[test]
    fn song_header_only() {
        let text = "\
#TITLE:Song Title;
#ARTIST:Someone;
#OFFSET:-0.123;
#SAMPLESTART:30.0;
#SAMPLELENGTH:15.0;
#BPMS:0.000=120.000;
";
        let song = parse(text).unwrap();
        assert_eq!(song.title.as_deref(), Some("Song Title"));
        assert_eq!(song.artist.as_deref(), Some("Someone"));
        assert_eq!(
            song.audio_sync_offset_seconds,
            Rational::new(-123, 1000).unwrap()
        );
        assert_eq!(song.preview.start_seconds, Rational::from_integer(30));
        assert_eq!(song.preview.length_seconds, Rational::from_integer(15));
        assert_eq!(song.tempo_segments.len(), 1);
        assert_eq!(
            song.tempo_segments[0].bpm,
            Bpm::from_rational(Rational::from_integer(120))
        );
        assert!(song.charts.is_empty());
    }

    #[test]
    fn multi_segment_bpms() {
        let text = "#BPMS:0.000=120.000,16.000=180.000;";
        let song = parse(text).unwrap();
        assert_eq!(song.tempo_segments.len(), 2);
        assert_eq!(
            song.tempo_segments[1].bpm,
            Bpm::from_rational(Rational::from_integer(180))
        );
    }

    #[test]
    fn stops_parsed_into_model() {
        let text = "#STOPS:8.000=0.500;";
        let song = parse(text).unwrap();
        assert_eq!(song.stops.len(), 1);
        assert_eq!(
            song.stops[0].duration_seconds,
            Rational::new(500, 1000).unwrap()
        );
    }

    #[test]
    fn notedata_section_becomes_chart() {
        let text = "\
#TITLE:X;
#BPMS:0.000=120.000;
#NOTEDATA:;
#STEPSTYPE:dance-single;
#DIFFICULTY:Easy;
#NOTES:
1000
0000
0000
0000
;
";
        let song = parse(text).unwrap();
        assert_eq!(song.charts.len(), 1);
        assert_eq!(song.charts[0].style, Style::Single);
        assert_eq!(song.charts[0].difficulty, Difficulty::Basic);
        assert_eq!(song.charts[0].notes.len(), 1);
        assert_eq!(song.charts[0].notes[0].kind, NoteKind::Tap);
    }

    #[test]
    fn two_notedata_sections_become_two_charts() {
        let text = "\
#BPMS:0.000=120.000;
#NOTEDATA:;
#STEPSTYPE:dance-single;
#DIFFICULTY:Easy;
#NOTES:
1000
0000
0000
0000
;
#NOTEDATA:;
#STEPSTYPE:dance-double;
#DIFFICULTY:Hard;
#NOTES:
10000000
00000000
00000000
00000000
;
";
        let song = parse(text).unwrap();
        assert_eq!(song.charts.len(), 2);
        assert_eq!(song.charts[0].style, Style::Single);
        assert_eq!(song.charts[1].style, Style::Double);
    }

    #[test]
    fn edit_chart_is_skipped_with_warning() {
        let text = "\
#BPMS:0.000=120.000;
#NOTEDATA:;
#STEPSTYPE:dance-single;
#DIFFICULTY:Edit;
#NOTES:
1000
0000
0000
0000
;
#NOTEDATA:;
#STEPSTYPE:dance-single;
#DIFFICULTY:Hard;
#NOTES:
1000
0000
0000
0000
;
";
        let song = parse(text).unwrap();
        // Only the Hard chart survives.
        assert_eq!(song.charts.len(), 1);
        assert_eq!(song.charts[0].difficulty, Difficulty::Expert);
    }

    #[test]
    fn missing_stepstype_in_notedata_is_rejected() {
        let text = "\
#BPMS:0.000=120.000;
#NOTEDATA:;
#DIFFICULTY:Easy;
#NOTES:
1000
0000
0000
0000
;
";
        let err = parse(text).unwrap_err();
        assert!(matches!(err, SscError::MissingRequiredNotedataTag { .. }));
    }

    #[test]
    fn unknown_tags_are_silently_ignored() {
        let text = "\
#TITLE:X;
#CDTITLE:whatever.png;
#BACKGROUND:bg.jpg;
#LASTSECONDHINT:180.0;
";
        let song = parse(text).unwrap();
        assert_eq!(song.title.as_deref(), Some("X"));
    }

    // ---------- format_decimal ----------

    #[test]
    fn format_decimal_integer_value() {
        assert_eq!(format_decimal(Rational::from_integer(120)), "120.000000");
    }

    #[test]
    fn format_decimal_zero() {
        assert_eq!(format_decimal(Rational::zero()), "0.000000");
    }

    #[test]
    fn format_decimal_negative() {
        // -123/1000 = -0.123
        assert_eq!(
            format_decimal(Rational::new(-123, 1000).unwrap()),
            "-0.123000"
        );
    }

    #[test]
    fn format_decimal_round_trips_common_values() {
        // Every decimal whose denominator divides 10^6 round-trips.
        for (num, den) in [(3, 4), (1, 2), (7, 8), (123, 1000), (-5, 2)] {
            let r = Rational::new(num, den).unwrap();
            let s = format_decimal(r);
            let back = decimal_to_rational(&s).unwrap();
            assert_eq!(back, r, "{num}/{den} -> {s}");
        }
    }

    // ---------- ssc::write + round-trip ----------

    fn write_string(song: &Song) -> String {
        let mut buf: Vec<u8> = Vec::new();
        write(song, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn roundtrip_song_header_only() {
        let text = "\
#TITLE:Song;
#ARTIST:Someone;
#OFFSET:-0.123;
#SAMPLESTART:30.0;
#SAMPLELENGTH:10.0;
#BPMS:0.000=120.000;
#STOPS:;
";
        let song = parse(text).unwrap();
        let written = write_string(&song);
        let re_parsed = parse(&written).unwrap();
        assert_eq!(re_parsed.title, song.title);
        assert_eq!(re_parsed.artist, song.artist);
        assert_eq!(
            re_parsed.audio_sync_offset_seconds,
            song.audio_sync_offset_seconds
        );
        assert_eq!(re_parsed.preview.start_seconds, song.preview.start_seconds);
        assert_eq!(
            re_parsed.preview.length_seconds,
            song.preview.length_seconds
        );
        assert_eq!(re_parsed.tempo_segments, song.tempo_segments);
        assert_eq!(re_parsed.stops, song.stops);
    }

    #[test]
    fn roundtrip_multi_segment_bpms_and_stops() {
        let text = "\
#BPMS:0.000=120.000,16.000=180.000,32.000=240.000;
#STOPS:8.000=0.500,24.000=1.000;
";
        let song = parse(text).unwrap();
        let written = write_string(&song);
        let re_parsed = parse(&written).unwrap();
        assert_eq!(re_parsed.tempo_segments, song.tempo_segments);
        assert_eq!(re_parsed.stops, song.stops);
    }

    #[test]
    fn roundtrip_with_one_chart() {
        let text = "\
#TITLE:X;
#BPMS:0.000=120.000;
#STOPS:;
#NOTEDATA:;
#STEPSTYPE:dance-single;
#DIFFICULTY:Easy;
#NOTES:
1000
0000
0020
0000
,
0000
0030
0000
1000
;
";
        let song = parse(text).unwrap();
        let written = write_string(&song);
        let re_parsed = parse(&written).unwrap();
        assert_eq!(re_parsed.charts.len(), 1);
        let a = &re_parsed.charts[0];
        let b = &song.charts[0];
        assert_eq!(a.style, b.style);
        assert_eq!(a.difficulty, b.difficulty);
        assert_eq!(a.notes.len(), b.notes.len());
        for (x, y) in a.notes.iter().zip(b.notes.iter()) {
            assert_eq!(x.beat, y.beat);
            assert_eq!(x.kind, y.kind);
            assert_eq!(x.panels.bits(), y.panels.bits());
        }
    }

    #[test]
    fn roundtrip_with_two_charts_of_different_styles() {
        let text = "\
#BPMS:0.000=120.000;
#STOPS:;
#NOTEDATA:;
#STEPSTYPE:dance-single;
#DIFFICULTY:Easy;
#NOTES:
1000
0000
0000
0000
;
#NOTEDATA:;
#STEPSTYPE:dance-double;
#DIFFICULTY:Hard;
#NOTES:
10000001
00000000
00000000
00000000
;
";
        let song = parse(text).unwrap();
        let written = write_string(&song);
        let re_parsed = parse(&written).unwrap();
        assert_eq!(re_parsed.charts.len(), 2);
        assert_eq!(re_parsed.charts[0].style, crate::model::Style::Single);
        assert_eq!(re_parsed.charts[1].style, crate::model::Style::Double);
        assert_eq!(re_parsed.charts[1].notes[0].panels.bits(), 0x81);
    }

    #[test]
    fn roundtrip_shock_bothsides() {
        let text = "\
#BPMS:0.000=120.000;
#STOPS:;
#NOTEDATA:;
#STEPSTYPE:dance-single;
#DIFFICULTY:Easy;
#NOTES:
MMMM
0000
0000
0000
;
";
        let song = parse(text).unwrap();
        let written = write_string(&song);
        let re_parsed = parse(&written).unwrap();
        assert_eq!(
            re_parsed.charts[0].notes[0].kind,
            NoteKind::Shock {
                side: crate::model::ShockSide::BothSides
            }
        );
    }

    // ---------- SM5 → DDR integration (Task 4) ----------

    /// Parse an SSC string, then re-serialize as SSQ and re-parse.
    /// Models the full SM5 → DDR pipeline for these tests.
    fn ssc_to_ssq_to_ssq_parse(ssc_text: &str) -> crate::ssq::SsqParseResult {
        let song = parse(ssc_text).unwrap();
        // Empty events list + empty raw pairs → SSQ writer synthesizes
        // its own tempo entries from the semantic view. That's the
        // same path the real SM5 → DDR job layer takes for a
        // synthetic song with no pre-parsed SSQ sidecar data.
        let events: Vec<crate::ssq::events::SsqEvent> = Vec::new();
        let raw_pairs: Vec<(i32, i32)> = Vec::new();
        let mut ssq_bytes = Vec::new();
        crate::ssq::writer::write(&song, &events, &raw_pairs, &mut ssq_bytes).unwrap();
        crate::ssq::parse(&ssq_bytes).unwrap()
    }

    #[test]
    fn sm5_to_ddr_per_difficulty_mines_produces_two_mine_chunks() {
        // Two charts with different per-panel mines. After SM5 → DDR
        // both should have their mines preserved, each in a distinct
        // MINE_DATA chunk keyed by the chart's difficulty code.
        // Single Basic gets a mine at beat 0 on Left, beat 1 on
        // Down + Right (multi-bit 0x0A). Double Expert gets a mine
        // at beat 2 on P1 Left + P2 Right (multi-bit 0x81).
        use crate::model::{NoteKind, Style};

        let text = "\
#TITLE:MinesTwoDifficulties;
#BPMS:0.000=120.000;
#STOPS:;
#NOTEDATA:;
#STEPSTYPE:dance-single;
#DIFFICULTY:Easy;
#NOTES:
M000
0M0M
0000
0000
;
#NOTEDATA:;
#STEPSTYPE:dance-double;
#DIFFICULTY:Hard;
#NOTES:
00000000
00000000
M000000M
00000000
;
";
        let re_parsed = ssc_to_ssq_to_ssq_parse(text);

        assert_eq!(re_parsed.song.charts.len(), 2);

        // Chart 0: Single Basic.
        let basic = &re_parsed.song.charts[0];
        assert_eq!(basic.style, Style::Single);
        assert_eq!(basic.difficulty, Difficulty::Basic);
        let basic_mines: Vec<&crate::model::Note> = basic
            .notes
            .iter()
            .filter(|n| matches!(n.kind, NoteKind::Mine))
            .collect();
        assert_eq!(
            basic_mines.len(),
            2,
            "Single Basic should have 2 mine entries (one per unique beat)"
        );
        // Beats: 0 (row 0) and 1 (row 1 in a 4-row measure = beat 4*1/4 = 1).
        // Beat 0 → panels 0x01 (Left only).
        // Beat 1 → panels 0x0A (Down + Right).
        assert_eq!(basic_mines[0].panels.bits(), 0x01);
        assert_eq!(basic_mines[1].panels.bits(), 0x0A);

        // Chart 1: Double Expert.
        let expert = &re_parsed.song.charts[1];
        assert_eq!(expert.style, Style::Double);
        assert_eq!(expert.difficulty, Difficulty::Expert);
        let expert_mines: Vec<&crate::model::Note> = expert
            .notes
            .iter()
            .filter(|n| matches!(n.kind, NoteKind::Mine))
            .collect();
        assert_eq!(
            expert_mines.len(),
            1,
            "Double Expert should have 1 mine entry"
        );
        // Beat 2 (row 2 in a 4-row measure = beat 2), panels 0x81.
        assert_eq!(expert_mines[0].panels.bits(), 0x81);
    }

    #[test]
    fn ddr_to_sm5_to_ddr_shock_round_trip_preserves_step_byte_shock() {
        // Build a Song with a Shock note (as if parsed from a step
        // chunk with byte 0xFF). Write → parse as DDR → SSC, parse
        // SSC → parse back as DDR. After the full round-trip the
        // chart must still have the Shock note (not a Mine note),
        // and the final SSQ bytes must contain NO type-20 chunk.
        use crate::model::{Beat, Chart, Note, NoteKind, Rational, ShockSide, Song, Style};

        let chart = Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: vec![Note {
                beat: Beat::from_measure_ticks(1024).unwrap(),
                kind: NoteKind::Shock {
                    side: ShockSide::BothSides,
                },
                panels: crate::model::PanelSet::empty(),
            }],
        };
        let song_in = Song {
            title: None,
            artist: None,
            tps: 1000,
            tempo_segments: vec![crate::model::TempoSegment {
                start_beat: Beat::zero(),
                bpm: crate::model::Bpm::from_rational(Rational::from_integer(120)),
            }],
            stops: Vec::new(),
            charts: vec![chart],
            audio: crate::model::AudioBuffer {
                samples: Vec::new(),
                sample_rate: 0,
                channels: 0,
            },
            audio_sync_offset_seconds: Rational::zero(),
            preview: crate::model::PreviewSlice::default_window(),
        };

        // DDR → SSC: write the song as SSC text.
        let ssc_text = write_string(&song_in);
        // Full-row `M` pattern must be present in the SSC output.
        assert!(
            ssc_text.contains("MMMM"),
            "SSC output must contain full-row M pattern for a Single BothSides shock"
        );

        // SSC → DDR: parse the SSC back, then serialize as SSQ and
        // re-parse.
        let re_parsed = ssc_to_ssq_to_ssq_parse(&ssc_text);
        let chart = &re_parsed.song.charts[0];
        let shock_count = chart
            .notes
            .iter()
            .filter(|n| matches!(n.kind, NoteKind::Shock { .. }))
            .count();
        let mine_count = chart
            .notes
            .iter()
            .filter(|n| matches!(n.kind, NoteKind::Mine))
            .count();
        assert_eq!(
            shock_count, 1,
            "shock must survive full round-trip as a Shock note"
        );
        assert_eq!(
            mine_count, 0,
            "shock must NOT degenerate into per-panel mines"
        );

        // Finally: re-serialize once more and scan the bytes for
        // any type-20 chunk header. A chart with only shocks and no
        // per-panel mines must produce zero MINE_DATA chunks.
        let events: Vec<crate::ssq::events::SsqEvent> = Vec::new();
        let raw_pairs: Vec<(i32, i32)> = Vec::new();
        let mut final_bytes = Vec::new();
        crate::ssq::writer::write(&re_parsed.song, &events, &raw_pairs, &mut final_bytes).unwrap();
        assert!(
            !bytes_contain_mine_chunk(&final_bytes),
            "round-tripped shock must not produce a MINE_DATA chunk"
        );
    }

    /// Walk an SSQ byte sequence and return true if any chunk has
    /// `type == 20` (MINE_DATA). Used by the shock-regression test
    /// to assert that a shock-only song produces no mine chunks.
    fn bytes_contain_mine_chunk(bytes: &[u8]) -> bool {
        let mut off = 0;
        while off + 6 <= bytes.len() {
            let length = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
            if length == 0 {
                return false; // terminator
            }
            let ty = u16::from_le_bytes(bytes[off + 4..off + 6].try_into().unwrap());
            if ty == 20 {
                return true;
            }
            off += length as usize;
        }
        false
    }

    #[test]
    fn ssc_mixed_row_parse_produces_mine_and_tap_at_same_beat() {
        // Row `M1M1` on Single: mines on Left + Up (bits 0+2, mask
        // 0x05), taps on Down + Right (bits 1+3, mask 0x0A). Both
        // notes at beat 0.
        use crate::model::NoteKind;

        let text = "\
#BPMS:0.000=120.000;
#STOPS:;
#NOTEDATA:;
#STEPSTYPE:dance-single;
#DIFFICULTY:Easy;
#NOTES:
M1M1
0000
0000
0000
;
";
        let song = parse(text).unwrap();
        let chart = &song.charts[0];
        assert_eq!(chart.notes.len(), 2);

        // Parser order: mine first (from classify_mine_row path), tap second.
        assert_eq!(chart.notes[0].kind, NoteKind::Mine);
        assert_eq!(chart.notes[0].panels.bits(), 0x05);

        assert_eq!(chart.notes[1].kind, NoteKind::Tap);
        assert_eq!(chart.notes[1].panels.bits(), 0x0A);

        // Both at beat 0.
        assert_eq!(chart.notes[0].beat, chart.notes[1].beat);
    }

    // ---------- DDR → SM5 integration (Task 5) ----------

    /// Build a complete Song from charts and write it as SSQ, round
    /// through the SSQ parser, and return the re-parsed result. Same
    /// helper pattern as `ssc_to_ssq_to_ssq_parse` but for building
    /// from scratch (not from an SSC string).
    fn song_to_ssq_to_ssq_parse(song: &crate::model::Song) -> crate::ssq::SsqParseResult {
        let events: Vec<crate::ssq::events::SsqEvent> = Vec::new();
        let raw_pairs: Vec<(i32, i32)> = Vec::new();
        let mut bytes = Vec::new();
        crate::ssq::writer::write(song, &events, &raw_pairs, &mut bytes).unwrap();
        crate::ssq::parse(&bytes).unwrap()
    }

    /// Minimal Song shell used by the round-trip tests — one tempo
    /// segment at 120 BPM, TPS=1000, no stops, no audio.
    fn mine_bearing_song(charts: Vec<crate::model::Chart>) -> crate::model::Song {
        use crate::model::{AudioBuffer, Bpm, PreviewSlice, Song, TempoSegment};
        Song {
            title: None,
            artist: None,
            tps: 1000,
            tempo_segments: vec![TempoSegment {
                start_beat: crate::model::Beat::zero(),
                bpm: Bpm::from_rational(Rational::from_integer(120)),
            }],
            stops: Vec::new(),
            charts,
            audio: AudioBuffer {
                samples: Vec::new(),
                sample_rate: 0,
                channels: 0,
            },
            audio_sync_offset_seconds: Rational::zero(),
            preview: PreviewSlice::default_window(),
        }
    }

    #[test]
    fn ddr_to_sm5_per_difficulty_mines_emits_m_chars_in_each_notedata() {
        // Build a Song with two charts with distinct mine patterns,
        // write as SSQ (Task 3 path), parse back (Task 3 attach),
        // then write as SSC (Task 5 Mine arm) and inspect the text.
        use crate::model::{Beat, Chart, Difficulty, Note, NoteKind, PanelSet, Style};

        let basic = Chart {
            style: Style::Single,
            difficulty: Difficulty::Basic,
            notes: vec![Note {
                beat: Beat::from_measure_ticks(0).unwrap(),
                kind: NoteKind::Mine,
                panels: PanelSet::from_bits(Style::Single, 0x01),
            }],
        };
        let expert = Chart {
            style: Style::Double,
            difficulty: Difficulty::Expert,
            notes: vec![Note {
                beat: Beat::from_measure_ticks(2048).unwrap(),
                kind: NoteKind::Mine,
                panels: PanelSet::from_bits(Style::Double, 0x81),
            }],
        };
        let song_in = mine_bearing_song(vec![basic, expert]);

        // DDR write → parse → SSC write.
        let re_parsed = song_to_ssq_to_ssq_parse(&song_in);
        let mut ssc_buf: Vec<u8> = Vec::new();
        write(&re_parsed.song, &mut ssc_buf).unwrap();
        let ssc_text = String::from_utf8(ssc_buf).unwrap();

        // Split by `#NOTEDATA:;` to inspect each chart's body in isolation.
        let sections: Vec<&str> = ssc_text.split("#NOTEDATA:;").collect();
        assert_eq!(sections.len(), 3, "song-level header + 2 notedata sections");
        let basic_section = sections[1];
        let expert_section = sections[2];

        // Basic (Single): mine on P1 Left at beat 0 → first body row is `M000`.
        assert!(
            basic_section.contains("dance-single"),
            "basic section should declare dance-single"
        );
        assert!(
            basic_section.contains("M000"),
            "Single Basic body must have M000 for Left-only mine at beat 0. body:\n{basic_section}"
        );

        // Expert (Double): mine at beat 2 on P1 Left + P2 Right →
        // row `M000000M` at row 2 of measure 0 (a 4-row measure).
        assert!(
            expert_section.contains("dance-double"),
            "expert section should declare dance-double"
        );
        assert!(
            expert_section.contains("M000000M"),
            "Double Expert body must have M000000M for L+R mine at beat 2. body:\n{expert_section}"
        );
    }

    #[test]
    fn ddr_to_sm5_to_ddr_mines_full_round_trip_is_byte_identical() {
        // A Song with multiple mines across multiple beats, written as
        // SSQ → parsed → written as SSC → parsed as SSC → written as
        // SSQ. After normalization (writer's group-by-beat + sort),
        // the second SSQ bytes should equal the first SSQ bytes.
        use crate::model::{Beat, Chart, Difficulty, Note, NoteKind, PanelSet, Style};

        let chart = Chart {
            style: Style::Single,
            difficulty: Difficulty::Challenge,
            notes: vec![
                Note {
                    beat: Beat::from_measure_ticks(0).unwrap(),
                    kind: NoteKind::Mine,
                    panels: PanelSet::from_bits(Style::Single, 0x01),
                },
                Note {
                    beat: Beat::from_measure_ticks(1024).unwrap(),
                    kind: NoteKind::Mine,
                    panels: PanelSet::from_bits(Style::Single, 0x09),
                },
                Note {
                    beat: Beat::from_measure_ticks(2048).unwrap(),
                    kind: NoteKind::Mine,
                    panels: PanelSet::from_bits(Style::Single, 0x04),
                },
            ],
        };
        let song_in = mine_bearing_song(vec![chart]);

        // First SSQ write.
        let events: Vec<crate::ssq::events::SsqEvent> = Vec::new();
        let raw_pairs: Vec<(i32, i32)> = Vec::new();
        let mut bytes1 = Vec::new();
        crate::ssq::writer::write(&song_in, &events, &raw_pairs, &mut bytes1).unwrap();

        // SSQ → SSC → SSC parse.
        let parsed_ssq = crate::ssq::parse(&bytes1).unwrap();
        let mut ssc_buf: Vec<u8> = Vec::new();
        write(&parsed_ssq.song, &mut ssc_buf).unwrap();
        let ssc_text = String::from_utf8(ssc_buf).unwrap();
        let song_from_ssc = parse(&ssc_text).unwrap();

        // Second SSQ write.
        let mut bytes2 = Vec::new();
        crate::ssq::writer::write(&song_from_ssc, &events, &raw_pairs, &mut bytes2).unwrap();

        assert_eq!(
            bytes1, bytes2,
            "DDR → SM5 → DDR round-trip must produce byte-identical SSQ output for mines"
        );
    }

    #[test]
    fn sm5_to_ddr_to_sm5_mines_full_round_trip_preserves_mine_positions() {
        // SSC with per-panel mines → SSQ write → SSQ parse → SSC write.
        // The final SSC's Mine notes must have the same
        // (beat_tick, panels.bits()) set as the original parse.
        use crate::model::NoteKind;

        let original_text = "\
#TITLE:RoundTrip;
#BPMS:0.000=120.000;
#STOPS:;
#NOTEDATA:;
#STEPSTYPE:dance-single;
#DIFFICULTY:Easy;
#NOTES:
M000
0M0M
0000
M00M
;
";
        let first_song = parse(original_text).unwrap();
        let original_mines: Vec<(i32, u8)> = first_song.charts[0]
            .notes
            .iter()
            .filter(|n| matches!(n.kind, NoteKind::Mine))
            .map(|n| {
                let r = n.beat.as_rational();
                let tick = ((r.num() * 1024) / r.den() as i64) as i32;
                (tick, n.panels.bits())
            })
            .collect();
        assert!(
            !original_mines.is_empty(),
            "test fixture must produce some mines; got none"
        );

        // SSC → SSQ → SSC.
        let events: Vec<crate::ssq::events::SsqEvent> = Vec::new();
        let raw_pairs: Vec<(i32, i32)> = Vec::new();
        let mut ssq_bytes = Vec::new();
        crate::ssq::writer::write(&first_song, &events, &raw_pairs, &mut ssq_bytes).unwrap();

        let parsed_ssq = crate::ssq::parse(&ssq_bytes).unwrap();

        let mut ssc_buf2: Vec<u8> = Vec::new();
        write(&parsed_ssq.song, &mut ssc_buf2).unwrap();
        let final_text = String::from_utf8(ssc_buf2).unwrap();
        let final_song = parse(&final_text).unwrap();

        let final_mines: Vec<(i32, u8)> = final_song.charts[0]
            .notes
            .iter()
            .filter(|n| matches!(n.kind, NoteKind::Mine))
            .map(|n| {
                let r = n.beat.as_rational();
                let tick = ((r.num() * 1024) / r.den() as i64) as i32;
                (tick, n.panels.bits())
            })
            .collect();

        assert_eq!(
            original_mines, final_mines,
            "SM5 → DDR → SM5 round-trip must preserve mine positions and masks.\n\
             original SSC:\n{original_text}\nfinal SSC:\n{final_text}"
        );
    }
}
