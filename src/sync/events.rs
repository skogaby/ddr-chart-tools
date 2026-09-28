//! Weighted chart events for correlation against the audio.
//!
//! The estimator correlates the audio against every note position of
//! every difficulty rather than a regular beat grid: a chart's rests,
//! breaks and syncopation make the pattern aperiodic, which keeps
//! aliased alignments from scoring as well as the true one.

use std::collections::{BTreeMap, BTreeSet};

use crate::model::{Beat, Chart, NoteKind};

use super::TimeMap;

/// One chart position the audio is correlated against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SyncEvent {
    /// When the position is heard, in audio seconds.
    pub time_s: f64,
    /// How many charts place a note there. Positions every difficulty
    /// agrees on are usually the strongest musical hits.
    pub weight: f64,
}

/// Union of the note positions of `charts`, placed on `map`.
///
/// Taps, hold heads and shocks count; mines do not, because they are not
/// musical hits. Each chart contributes at most once per beat, and the
/// weight of a position is the number of charts that place a note there.
/// Events are returned in beat order.
#[must_use]
pub fn chart_events(charts: &[Chart], map: &TimeMap) -> Vec<SyncEvent> {
    let mut weights: BTreeMap<Beat, u32> = BTreeMap::new();
    for chart in charts {
        let beats: BTreeSet<Beat> = chart
            .notes
            .iter()
            .filter(|n| !matches!(n.kind, NoteKind::Mine))
            .map(|n| n.beat)
            .collect();
        for beat in beats {
            *weights.entry(beat).or_insert(0) += 1;
        }
    }
    weights
        .into_iter()
        .map(|(beat, count)| SyncEvent {
            time_s: map.seconds_at(beat.as_rational().as_f64()),
            weight: f64::from(count),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Beat, Chart, Difficulty, Note, NoteKind, PanelSet, ShockSide, Style};
    use crate::sync::TimeMap;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn note(beat: i64, kind: NoteKind, panels: u8) -> Result<Note, Box<dyn std::error::Error>> {
        Ok(Note {
            beat: Beat::from_measure_ticks(beat * 1024)?,
            kind,
            panels: PanelSet::from_bits(Style::Single, panels),
        })
    }

    fn chart(difficulty: Difficulty, notes: Vec<Note>) -> Chart {
        Chart {
            style: Style::Single,
            difficulty,
            notes,
        }
    }

    /// 120 BPM from beat 0: half a second per beat.
    fn half_second_beats() -> Result<TimeMap, Box<dyn std::error::Error>> {
        TimeMap::from_tempo_pairs(&[(0, 0), (4096, 2000)], 1000).ok_or_else(|| "map".into())
    }

    #[test]
    fn weights_count_charts_not_notes() -> TestResult {
        let hold = NoteKind::HoldHead {
            length: Beat::from_measure_ticks(1024)?,
        };
        let a = chart(
            Difficulty::Basic,
            vec![
                note(4, NoteKind::Tap, 0x01)?,
                note(4, hold, 0x08)?,
                note(6, NoteKind::Tap, 0x02)?,
            ],
        );
        let b = chart(
            Difficulty::Expert,
            vec![
                note(4, NoteKind::Tap, 0x04)?,
                note(5, NoteKind::Mine, 0x01)?,
            ],
        );
        let events = chart_events(&[a, b], &half_second_beats()?);
        assert_eq!(
            events,
            vec![
                SyncEvent {
                    time_s: 2.0,
                    weight: 2.0
                },
                SyncEvent {
                    time_s: 3.0,
                    weight: 1.0
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn mines_only_chart_contributes_nothing() -> TestResult {
        let c = chart(Difficulty::Basic, vec![note(1, NoteKind::Mine, 0x01)?]);
        assert!(chart_events(&[c], &half_second_beats()?).is_empty());
        Ok(())
    }

    #[test]
    fn shocks_count() -> TestResult {
        let shock = NoteKind::Shock {
            side: ShockSide::P1Only,
        };
        let c = chart(Difficulty::Basic, vec![note(2, shock, 0x0F)?]);
        assert_eq!(
            chart_events(&[c], &half_second_beats()?),
            vec![SyncEvent {
                time_s: 1.0,
                weight: 1.0
            }]
        );
        Ok(())
    }
}
