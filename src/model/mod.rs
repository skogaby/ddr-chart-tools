//! Format-independent song representation.
//!
//! Every conversion goes `source format → model → target format`.
//! This module owns the types the conversions translate through.
//!
//! Design principles:
//!
//! - All musical positions (`Beat`) and tempo values (`Bpm`) are exact
//!   rationals. Floats drift; rationals round-trip losslessly.
//! - All musical positions in the model are measured in *beats* (one
//!   whole-note == 4 beats). SSQ stores them as measure ticks (4096 per
//!   whole note == 1024 per beat); conversion is lossless via `Rational`.
//! - Audio lives on the `Song` alongside charts — one pipeline, not two.

pub mod audio;
pub mod preview;
pub mod rational;
pub mod tick;

pub use audio::{AudioBuffer, AudioFormatKind};
pub use preview::PreviewSlice;
pub use rational::{Rational, RationalError};
pub use tick::{TickScale, TickScaleError};
/// A musical position in beats. 1 beat == 1024 SSQ measure ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Beat(Rational);

impl Beat {
    /// SSQ measure-tick convention: 4096 ticks per whole note == 1024 per beat.
    pub const TICKS_PER_BEAT: i64 = 1024;

    #[must_use]
    pub fn zero() -> Self {
        Self(Rational::zero())
    }

    pub fn from_rational(r: Rational) -> Self {
        Self(r)
    }

    /// Construct from an SSQ measure-tick offset.
    pub fn from_measure_ticks(ticks: i64) -> Result<Self, RationalError> {
        Rational::new(ticks, Self::TICKS_PER_BEAT).map(Self)
    }

    #[must_use]
    pub fn as_rational(&self) -> Rational {
        self.0
    }
}

/// Tempo in beats per minute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Bpm(Rational);

impl Bpm {
    pub fn from_rational(r: Rational) -> Self {
        Self(r)
    }

    #[must_use]
    pub fn as_rational(&self) -> Rational {
        self.0
    }
}

/// Play style — determines how many panels are active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Style {
    /// Four panels (L, D, U, R).
    Single,
    /// Eight panels (P1 L/D/U/R, P2 L/D/U/R).
    Double,
}

impl Style {
    #[must_use]
    pub fn panel_count(self) -> u8 {
        match self {
            Self::Single => 4,
            Self::Double => 8,
        }
    }
}

/// Chart difficulty slot. Maps to SSQ difficulty codes in `ssq/`
/// and to SSC `#DIFFICULTY` names in `ssc/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Difficulty {
    Beginner,
    Basic,
    Difficult,
    Expert,
    Challenge,
}

/// Set of active panels for a single note, stored as a bitmask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PanelSet(u8);

impl PanelSet {
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Construct from a raw bitmask. Bits above `style.panel_count()` are masked off.
    #[must_use]
    pub fn from_bits(style: Style, bits: u8) -> Self {
        let mask = (1u16 << style.panel_count()) - 1;
        Self(bits & (mask as u8))
    }

    #[must_use]
    pub fn bits(self) -> u8 {
        self.0
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub fn contains(self, panel: u8) -> bool {
        panel < 8 && (self.0 & (1u8 << panel)) != 0
    }

    #[must_use]
    pub fn with(self, panel: u8) -> Self {
        if panel < 8 {
            Self(self.0 | (1u8 << panel))
        } else {
            self
        }
    }

    #[must_use]
    pub fn count(self) -> u32 {
        self.0.count_ones()
    }
}

/// Which side(s) a shock arrow affects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShockSide {
    /// All panels of both sides active.
    BothSides,
    /// Only the P1 side (4 panels).
    P1Only,
    /// Only the P2 side (4 panels).
    P2Only,
}

/// One note (or sustain start, shock, or mine) at a specific beat.
#[derive(Debug, Clone)]
pub struct Note {
    pub beat: Beat,
    pub kind: NoteKind,
    pub panels: PanelSet,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NoteKind {
    Tap,
    /// Freeze/hold start. `length` is the hold duration in beats.
    HoldHead {
        length: Beat,
    },
    Shock {
        side: ShockSide,
    },
    /// Per-panel hazard note (ITG-style mine). Panel info rides on
    /// `Note.panels`, matching `Tap`. Unlike `Shock`, a mine affects
    /// only the panels it sits on.
    Mine,
}

/// One difficulty of one song.
#[derive(Debug, Clone)]
pub struct Chart {
    pub style: Style,
    pub difficulty: Difficulty,
    /// Notes sorted by beat. Parsers are responsible for producing sorted output.
    pub notes: Vec<Note>,
}

/// A tempo change at a specific beat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TempoSegment {
    pub start_beat: Beat,
    pub bpm: Bpm,
}

/// A pause in the song timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stop {
    pub at_beat: Beat,
    pub duration_seconds: Rational,
}

/// Format-independent representation of one song plus its audio.
#[derive(Debug, Clone)]
pub struct Song {
    pub title: Option<String>,
    pub artist: Option<String>,
    /// SSQ tick rate from the source file (or 1000 for SM5 sources).
    /// Dominant value for newly-authored charts is 1000; older charts use
    /// lower rates (150 and 75 observed). Set to 1000 after modernization.
    /// The writer emits this value verbatim.
    pub tps: u32,
    pub tempo_segments: Vec<TempoSegment>,
    pub stops: Vec<Stop>,
    pub charts: Vec<Chart>,
    pub audio: AudioBuffer,
    /// Offset between chart time-zero and audio time-zero. Positive
    /// means audio has elapsed `audio_sync_offset_seconds` of pre-roll
    /// by the time the chart reaches beat zero.
    pub audio_sync_offset_seconds: Rational,
    pub preview: PreviewSlice,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beat_from_measure_ticks_round_trips_4096() {
        // 4096 ticks = 4 beats = whole note
        let b = Beat::from_measure_ticks(4096).unwrap();
        assert_eq!(b.as_rational(), Rational::from_integer(4));
    }

    #[test]
    fn beat_zero_is_ordered_first() {
        assert!(Beat::zero() < Beat::from_measure_ticks(1).unwrap());
    }

    #[test]
    fn beat_ordering_matches_rational() {
        let a = Beat::from_measure_ticks(1024).unwrap();
        let b = Beat::from_measure_ticks(2048).unwrap();
        assert!(a < b);
    }

    #[test]
    fn style_panel_counts() {
        assert_eq!(Style::Single.panel_count(), 4);
        assert_eq!(Style::Double.panel_count(), 8);
    }

    #[test]
    fn panelset_masks_out_bits_above_style_width() {
        // Single has 4 panels — upper nibble should be masked.
        let ps = PanelSet::from_bits(Style::Single, 0xFF);
        assert_eq!(ps.bits(), 0x0F);
    }

    #[test]
    fn panelset_double_keeps_all_8_bits() {
        let ps = PanelSet::from_bits(Style::Double, 0xFF);
        assert_eq!(ps.bits(), 0xFF);
        assert_eq!(ps.count(), 8);
    }

    #[test]
    fn panelset_contains_and_with() {
        let ps = PanelSet::empty().with(0).with(3);
        assert!(ps.contains(0));
        assert!(ps.contains(3));
        assert!(!ps.contains(1));
        assert_eq!(ps.count(), 2);
    }

    #[test]
    fn panelset_with_out_of_range_is_noop() {
        let ps = PanelSet::empty().with(9);
        assert!(ps.is_empty());
    }

    #[test]
    fn bpm_ordering() {
        let slow = Bpm::from_rational(Rational::from_integer(120));
        let fast = Bpm::from_rational(Rational::from_integer(240));
        assert!(slow < fast);
    }

    #[test]
    fn mine_variant_equals_itself_and_differs_from_tap() {
        assert_eq!(NoteKind::Mine, NoteKind::Mine);
        assert_ne!(NoteKind::Mine, NoteKind::Tap);
    }

    #[test]
    fn mine_variant_differs_from_shock() {
        assert_ne!(
            NoteKind::Mine,
            NoteKind::Shock {
                side: ShockSide::BothSides
            }
        );
    }

    #[test]
    fn mine_variant_differs_from_holdhead() {
        assert_ne!(
            NoteKind::Mine,
            NoteKind::HoldHead {
                length: Beat::from_measure_ticks(1024).unwrap()
            }
        );
    }

    #[test]
    fn mine_variant_hashes_consistently() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut h1 = DefaultHasher::new();
        let mut h2 = DefaultHasher::new();
        NoteKind::Mine.hash(&mut h1);
        NoteKind::Mine.hash(&mut h2);
        assert_eq!(h1.finish(), h2.finish());
    }

    #[test]
    fn mine_note_clones_preserve_panels_and_beat() {
        let note = Note {
            beat: Beat::from_measure_ticks(2048).unwrap(),
            kind: NoteKind::Mine,
            panels: PanelSet::from_bits(Style::Single, 0x05),
        };
        let cloned = note.clone();
        assert_eq!(cloned.kind, NoteKind::Mine);
        assert_eq!(cloned.panels.bits(), 0x05);
        assert_eq!(cloned.beat, note.beat);
    }
}
