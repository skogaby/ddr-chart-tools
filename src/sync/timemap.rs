//! Beat → audio-time mapping for analysis.
//!
//! A [`TimeMap`] is the piecewise-linear timeline an output chart will
//! actually play with: built from the SSQ tempo pairs an SSQ output
//! writes, or from the model timing an SSC output writes. It is `f64`
//! and used only to place chart events for correlation; nothing here is
//! written to disk.

use crate::model::{Beat, Bpm, Song};

/// Measure ticks per beat in an SSQ tempo chunk (`docs/ssq_format.md` §3).
const TICKS_PER_BEAT: f64 = Beat::TICKS_PER_BEAT as f64;

/// Beats per measure, used to place the trailing anchor of a song map.
const BEATS_PER_MEASURE: f64 = 4.0;

/// Piecewise-linear beat → audio-seconds mapping.
///
/// Anchors are `(beat, seconds)` sorted by beat. A stop is two anchors
/// at the same beat; a note exactly on a stop plays at the stop's start.
/// Outside the anchors the map extrapolates with the nearest tempo.
#[derive(Debug, Clone, PartialEq)]
pub struct TimeMap {
    anchors: Vec<(f64, f64)>,
    /// Seconds per beat of the first segment with a nonzero beat span.
    first_slope: f64,
    /// Seconds per beat of the last segment with a nonzero beat span.
    last_slope: f64,
}

impl TimeMap {
    /// Build from SSQ `(measure_tick, tempo_data)` pairs at `tps` ticks
    /// per second (`docs/ssq_format.md` §3): beat = tick / 1024,
    /// seconds = tempo_data / tps.
    ///
    /// Returns `None` for `tps == 0`, fewer than two pairs, ticks that
    /// decrease, or pairs that never advance in beat.
    #[must_use]
    pub fn from_tempo_pairs(pairs: &[(i32, i32)], tps: u32) -> Option<TimeMap> {
        if tps == 0 || pairs.windows(2).any(|w| w[1].0 < w[0].0) {
            return None;
        }
        let tps = f64::from(tps);
        let anchors = pairs
            .iter()
            .map(|&(tick, td)| (f64::from(tick) / TICKS_PER_BEAT, f64::from(td) / tps))
            .collect();
        Self::from_anchors(anchors)
    }

    /// Build from the model's semantic timing: `tempo_segments`, `stops`
    /// and `audio_sync_offset_seconds` (the audio time of beat 0).
    ///
    /// Walks the timeline the way the SSQ writer synthesizes tempo pairs
    /// (`ssq::writer::synthesize_tempo_entries_until`): the earliest
    /// segment's tempo applies from beat 0, and segment boundaries and
    /// stops merge in beat order, segments first on a tie. A trailing
    /// anchor one measure past the last is added at the tempo in force,
    /// so extrapolation beyond the final BPM change uses that tempo.
    ///
    /// Returns `None` when there are no tempo segments or any BPM is not
    /// positive.
    #[must_use]
    pub fn from_song(song: &Song) -> Option<TimeMap> {
        let bpm_of = |bpm: Bpm| {
            let b = bpm.as_rational().as_f64();
            (b > 0.0).then_some(b)
        };
        let first = song.tempo_segments.iter().min_by_key(|s| s.start_beat)?;
        let mut cur_bpm = bpm_of(first.bpm)?;

        enum Ev {
            Segment { beat: f64, bpm: f64 },
            Stop { beat: f64, seconds: f64 },
        }
        let mut timeline: Vec<(Beat, Ev)> = Vec::new();
        for seg in &song.tempo_segments {
            timeline.push((
                seg.start_beat,
                Ev::Segment {
                    beat: seg.start_beat.as_rational().as_f64(),
                    bpm: bpm_of(seg.bpm)?,
                },
            ));
        }
        for stop in &song.stops {
            timeline.push((
                stop.at_beat,
                Ev::Stop {
                    beat: stop.at_beat.as_rational().as_f64(),
                    seconds: stop.duration_seconds.as_f64(),
                },
            ));
        }
        // Stable: segments were pushed first, so they win beat ties.
        timeline.sort_by_key(|(beat, _)| *beat);

        let mut acc = song.audio_sync_offset_seconds.as_f64();
        let mut prev_beat = 0.0;
        let mut anchors = vec![(0.0, acc)];
        let push = |anchors: &mut Vec<(f64, f64)>, beat: f64, secs: f64| {
            if anchors.last() != Some(&(beat, secs)) {
                anchors.push((beat, secs));
            }
        };
        for (_, ev) in timeline {
            match ev {
                Ev::Segment { beat, bpm } => {
                    if beat > prev_beat {
                        acc += (beat - prev_beat) * 60.0 / cur_bpm;
                        prev_beat = beat;
                    }
                    if beat > 0.0 {
                        push(&mut anchors, beat, acc);
                    }
                    cur_bpm = bpm;
                }
                Ev::Stop { beat, seconds } => {
                    if beat > prev_beat {
                        acc += (beat - prev_beat) * 60.0 / cur_bpm;
                        prev_beat = beat;
                    }
                    push(&mut anchors, beat, acc);
                    acc += seconds;
                    push(&mut anchors, beat, acc);
                }
            }
        }
        let trailing = prev_beat + BEATS_PER_MEASURE;
        push(
            &mut anchors,
            trailing,
            acc + BEATS_PER_MEASURE * 60.0 / cur_bpm,
        );
        Self::from_anchors(anchors)
    }

    fn from_anchors(anchors: Vec<(f64, f64)>) -> Option<TimeMap> {
        let slope =
            |w: &[(f64, f64)]| (w[1].0 > w[0].0).then(|| (w[1].1 - w[0].1) / (w[1].0 - w[0].0));
        let first_slope = anchors.windows(2).find_map(slope)?;
        let last_slope = anchors.windows(2).rev().find_map(slope)?;
        Some(TimeMap {
            anchors,
            first_slope,
            last_slope,
        })
    }

    /// Audio time of `beat`, in seconds.
    #[must_use]
    pub fn seconds_at(&self, beat: f64) -> f64 {
        let n = self.anchors.len();
        let k = self.anchors.partition_point(|&(b, _)| b < beat);
        if k < n && self.anchors[k].0 == beat {
            return self.anchors[k].1;
        }
        if k == 0 {
            let (b0, s0) = self.anchors[0];
            return s0 + (beat - b0) * self.first_slope;
        }
        if k == n {
            let (bn, sn) = self.anchors[n - 1];
            return sn + (beat - bn) * self.last_slope;
        }
        // anchors[k-1].0 < beat < anchors[k].0, so the span is nonzero.
        let (b0, s0) = self.anchors[k - 1];
        let (b1, s1) = self.anchors[k];
        s0 + (s1 - s0) * (beat - b0) / (b1 - b0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AudioBuffer, Beat, Bpm, PreviewSlice, Rational, Song, Stop, TempoSegment};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    fn map(pairs: &[(i32, i32)], tps: u32) -> Result<TimeMap, Box<dyn std::error::Error>> {
        TimeMap::from_tempo_pairs(pairs, tps).ok_or_else(|| "map should build".into())
    }

    fn beat(b: i64) -> Beat {
        Beat::from_rational(Rational::from_integer(b))
    }

    fn segment(at: i64, bpm: i64) -> TempoSegment {
        TempoSegment {
            start_beat: beat(at),
            bpm: Bpm::from_rational(Rational::from_integer(bpm)),
        }
    }

    fn song(segments: Vec<TempoSegment>, stops: Vec<Stop>, offset_ms: i64) -> Song {
        Song {
            title: None,
            artist: None,
            tps: 1000,
            tempo_segments: segments,
            stops,
            charts: Vec::new(),
            audio: AudioBuffer {
                samples: Vec::new(),
                sample_rate: 0,
                channels: 0,
            },
            audio_sync_offset_seconds: Rational::new(offset_ms, 1000).unwrap_or(Rational::zero()),
            preview: PreviewSlice::default_window(),
        }
    }

    #[test]
    fn pairs_single_bpm_interpolates() -> TestResult {
        let m = map(&[(0, 0), (4096, 2000)], 1000)?;
        assert!(close(m.seconds_at(0.0), 0.0, 1e-12));
        assert!(close(m.seconds_at(2.0), 1.0, 1e-12));
        assert!(close(m.seconds_at(4.0), 2.0, 1e-12));
        Ok(())
    }

    #[test]
    fn pairs_stop_note_on_stop_takes_stop_start() -> TestResult {
        let m = map(&[(0, 0), (8192, 4000), (8192, 4500), (12288, 6500)], 1000)?;
        assert!(
            close(m.seconds_at(8.0), 4.0, 1e-12),
            "a note on a stop plays at its start"
        );
        assert!(
            close(m.seconds_at(9.0), 5.0, 1e-12),
            "notes after a stop include it"
        );
        Ok(())
    }

    #[test]
    fn pairs_extrapolate_before_and_after() -> TestResult {
        let m = map(&[(0, 0), (8192, 4000), (8192, 4500), (12288, 6500)], 1000)?;
        assert!(close(m.seconds_at(-1.0), -0.5, 1e-12));
        assert!(close(m.seconds_at(13.0), 7.0, 1e-12));
        Ok(())
    }

    #[test]
    fn pairs_extrapolate_past_trailing_stop() -> TestResult {
        // The last anchor closes a stop; extrapolation continues from it
        // with the last real tempo.
        let m = map(&[(0, 0), (4096, 2000), (4096, 2500)], 1000)?;
        assert!(close(m.seconds_at(5.0), 3.0, 1e-12));
        Ok(())
    }

    #[test]
    fn pairs_tps_scales_seconds() -> TestResult {
        let m = map(&[(0, 0), (4096, 300)], 150)?;
        assert!(close(m.seconds_at(4.0), 2.0, 1e-12));
        Ok(())
    }

    #[test]
    fn pairs_reject_malformed() {
        assert!(
            TimeMap::from_tempo_pairs(&[(0, 0)], 1000).is_none(),
            "one pair"
        );
        assert!(
            TimeMap::from_tempo_pairs(&[(0, 0), (4096, 2000)], 0).is_none(),
            "TPS 0"
        );
        assert!(
            TimeMap::from_tempo_pairs(&[(0, 0), (4096, 2000), (2048, 3000)], 1000).is_none(),
            "decreasing ticks"
        );
        assert!(
            TimeMap::from_tempo_pairs(&[(0, 0), (0, 500), (0, 900)], 1000).is_none(),
            "no beat span"
        );
    }

    #[test]
    fn song_extrapolates_with_final_bpm() -> TestResult {
        let s = song(vec![segment(0, 160), segment(16, 120)], Vec::new(), 0);
        let m = TimeMap::from_song(&s).ok_or("map should build")?;
        // 16 beats at 160 BPM (6 s) + 24 beats at 120 BPM (12 s).
        assert!(
            close(m.seconds_at(40.0), 18.0, 1e-9),
            "got {}",
            m.seconds_at(40.0)
        );
        Ok(())
    }

    #[test]
    fn song_matches_synthesized_pairs() -> TestResult {
        let s = song(
            vec![segment(0, 160), segment(16, 120)],
            vec![Stop {
                at_beat: beat(24),
                duration_seconds: Rational::new(1, 2)?,
            }],
            20,
        );
        let pairs = crate::ssq::writer::synthesize_tempo_entries_until(&s, Some(beat(44)))?;
        let from_pairs = map(&pairs, 1000)?;
        let from_song = TimeMap::from_song(&s).ok_or("map should build")?;
        for b in 0..=44 {
            let (a, c) = (
                from_song.seconds_at(b as f64),
                from_pairs.seconds_at(b as f64),
            );
            assert!(close(a, c, 0.001), "beat {b}: song {a} vs pairs {c}");
        }
        Ok(())
    }

    #[test]
    fn song_first_bpm_applies_from_beat_zero() -> TestResult {
        let s = song(vec![segment(2, 120)], Vec::new(), 20);
        let m = TimeMap::from_song(&s).ok_or("map should build")?;
        assert!(close(m.seconds_at(0.0), 0.02, 1e-12));
        assert!(close(m.seconds_at(2.0), 1.02, 1e-12));
        Ok(())
    }

    #[test]
    fn song_rejects_no_segments_and_nonpositive_bpm() {
        assert!(TimeMap::from_song(&song(Vec::new(), Vec::new(), 0)).is_none());
        assert!(TimeMap::from_song(&song(vec![segment(0, 0)], Vec::new(), 0)).is_none());
        assert!(TimeMap::from_song(&song(vec![segment(0, -120)], Vec::new(), 0)).is_none());
    }
}
