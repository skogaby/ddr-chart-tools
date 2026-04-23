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

    #[error("unsupported mine pattern at measure {measure}, row {row}: {reason}. Only DDR-shock-equivalent full-row mine patterns (all panels, or all-P1 / all-P2 on Double) are accepted.")]
    UnsupportedMine {
        measure: usize,
        row: usize,
        reason: String,
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

pub(crate) fn apply_song_tag(song: &mut Song, tag: &str, value: &msd::MsdValue) -> Result<(), SscError> {
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
}
