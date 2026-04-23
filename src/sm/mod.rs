//! StepMania `.sm` simfile parser (read-only).
//!
//! SM is the older StepMania simfile format. It uses the same MSD
//! tokenizer as SSC, but charts are encoded as a single `#NOTES` tag
//! with 6 colon-separated fields rather than SSC's `#NOTEDATA`
//! sections with individual tags.
//!
//! SM format: `#NOTES:stepstype:description:difficulty:meter:radar:notedata;`
//!
//! This module only parses — SM is never written (SSC is the only SM5
//! output format).

use thiserror::Error;

use crate::model::Song;
use crate::ssc;

#[derive(Debug, Error)]
pub enum SmError {
    #[error("SM #NOTES block has {count} fields (expected 6: stepstype:description:difficulty:meter:radar:notedata)")]
    BadNotesFieldCount { count: usize },

    #[error("chart parse error: {0}")]
    Chart(#[from] ssc::SscError),
}

/// Parse a complete `.sm` file into a `Song`.
pub fn parse(text: &str) -> Result<Song, SmError> {
    let values = ssc::msd::tokenize(text);
    let mut song = ssc::empty_song();

    for value in &values {
        let tag = value.tag().to_ascii_uppercase();

        if tag == "NOTES" {
            // SM #NOTES has 6 colon-separated params (including the tag):
            // params[0] = "NOTES"
            // params[1] = stepstype
            // params[2] = description (ignored)
            // params[3] = difficulty
            // params[4] = meter (ignored)
            // params[5] = radar values (ignored)
            // params[6] = note data
            if value.params.len() != 7 {
                return Err(SmError::BadNotesFieldCount {
                    count: value.params.len(),
                });
            }

            let style = match ssc::notes::parse_stepstype(value.param(1)) {
                Ok(s) => s,
                Err(ssc::SscError::UnsupportedStepsType(t)) => {
                    log::warn!("skipping unsupported stepstype: {t}");
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            let difficulty = match ssc::notes::parse_difficulty(value.param(3)) {
                Ok(d) => d,
                Err(ssc::SscError::EditChartSkipped) => {
                    log::warn!("skipping Edit chart");
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            let notes = ssc::notes::parse_notes_body(value.param(6), style)?;
            song.charts.push(crate::model::Chart {
                style,
                difficulty,
                notes,
            });
            continue;
        }

        // Song-level tags are identical to SSC.
        ssc::apply_song_tag(&mut song, &tag, value)?;
    }

    Ok(song)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Bpm, Difficulty, NoteKind, Rational, Style};

    #[test]
    fn song_header_parsed() {
        let text = "\
#TITLE:Test Song;
#ARTIST:Test Artist;
#OFFSET:-0.050;
#BPMS:0.000=150.000;
#STOPS:;
";
        let song = parse(text).unwrap();
        assert_eq!(song.title.as_deref(), Some("Test Song"));
        assert_eq!(song.artist.as_deref(), Some("Test Artist"));
        assert_eq!(
            song.audio_sync_offset_seconds,
            Rational::new(-50, 1000).unwrap()
        );
        assert_eq!(song.tempo_segments.len(), 1);
        assert_eq!(
            song.tempo_segments[0].bpm,
            Bpm::from_rational(Rational::from_integer(150))
        );
    }

    #[test]
    fn single_chart_parsed() {
        let text = "\
#BPMS:0.000=120.000;
#NOTES:
     dance-single:
     :
     Easy:
     3:
     0.5,0.5,0.5,0.5,0.5:
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
    fn multiple_charts_parsed() {
        let text = "\
#BPMS:0.000=120.000;
#NOTES:
     dance-single:
     :
     Easy:
     3:
     :
1000
0000
0000
0000
;
#NOTES:
     dance-single:
     :
     Hard:
     8:
     :
1000
0100
0010
0001
;
";
        let song = parse(text).unwrap();
        assert_eq!(song.charts.len(), 2);
        assert_eq!(song.charts[0].difficulty, Difficulty::Basic);
        assert_eq!(song.charts[1].difficulty, Difficulty::Expert);
        assert_eq!(song.charts[1].notes.len(), 4);
    }

    #[test]
    fn edit_chart_skipped() {
        let text = "\
#BPMS:0.000=120.000;
#NOTES:
     dance-single:
     :
     Edit:
     5:
     :
1000
0000
0000
0000
;
#NOTES:
     dance-single:
     :
     Hard:
     8:
     :
1000
0000
0000
0000
;
";
        let song = parse(text).unwrap();
        assert_eq!(song.charts.len(), 1);
        assert_eq!(song.charts[0].difficulty, Difficulty::Expert);
    }

    #[test]
    fn unsupported_stepstype_skipped() {
        let text = "\
#BPMS:0.000=120.000;
#NOTES:
     pump-single:
     :
     Easy:
     3:
     :
10000
00000
00000
00000
;
#NOTES:
     dance-single:
     :
     Easy:
     3:
     :
1000
0000
0000
0000
;
";
        let song = parse(text).unwrap();
        assert_eq!(song.charts.len(), 1);
        assert_eq!(song.charts[0].style, Style::Single);
    }

    #[test]
    fn double_chart_parsed() {
        let text = "\
#BPMS:0.000=120.000;
#NOTES:
     dance-double:
     :
     Hard:
     8:
     :
10000001
00000000
00000000
00000000
;
";
        let song = parse(text).unwrap();
        assert_eq!(song.charts.len(), 1);
        assert_eq!(song.charts[0].style, Style::Double);
        assert_eq!(song.charts[0].notes[0].panels.bits(), 0x81);
    }

    #[test]
    fn bad_field_count_rejected() {
        // Only 4 fields instead of 6.
        let text = "#NOTES:dance-single:Easy:3:0000\n0000\n0000\n0000\n;";
        let err = parse(text).unwrap_err();
        assert!(matches!(err, SmError::BadNotesFieldCount { .. }));
    }

    #[test]
    fn hold_note_parsed() {
        let text = "\
#BPMS:0.000=120.000;
#NOTES:
     dance-single:
     :
     Easy:
     3:
     :
2000
0000
0000
3000
;
";
        let song = parse(text).unwrap();
        assert_eq!(song.charts[0].notes.len(), 1);
        assert!(matches!(
            song.charts[0].notes[0].kind,
            NoteKind::HoldHead { .. }
        ));
    }
}
