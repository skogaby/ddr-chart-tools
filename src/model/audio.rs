//! Format-independent audio representation.
//!
//! All audio formats decode into `AudioBuffer` and encode from it.
//! `AudioFormatKind` enumerates the supported container/codec formats
//! — the list is expected to grow as more legacy rhythm-game audio
//! formats are added as decode-only inputs.

/// A decoded audio buffer. Samples are interleaved when `channels > 1`.
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    pub samples: Vec<i16>,
    pub sample_rate: u32,
    pub channels: u16,
}

impl AudioBuffer {
    /// Number of frames (samples per channel) in the buffer.
    #[must_use]
    pub fn frame_count(&self) -> usize {
        if self.channels == 0 {
            return 0;
        }
        self.samples.len() / (self.channels as usize)
    }

    /// Duration of the buffer in seconds as `f64`. Use only for logging
    /// or display; prefer integer frame math for anything positional.
    #[must_use]
    pub fn duration_seconds(&self) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.frame_count() as f64 / self.sample_rate as f64
    }
}

/// Known audio formats. Extend this enum when adding a new decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormatKind {
    /// Microsoft XACT Wave Bank — DDR's audio container.
    Xwb,
    /// Headerless XBOX-IMA ADPCM — pre-World DDR audio.
    Wavm,
    /// Ogg Vorbis — StepMania 5's standard audio format.
    Ogg,
}

impl AudioFormatKind {
    /// Whether the format can be written by this tool. Decode-only
    /// formats (WAVM, and future legacy-authoring formats) return false.
    #[must_use]
    pub fn is_writable(self) -> bool {
        match self {
            Self::Xwb | Self::Ogg => true,
            Self::Wavm => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_count_handles_stereo() {
        let buf = AudioBuffer {
            samples: vec![0; 88_200], // 1 second of stereo 44.1 kHz
            sample_rate: 44_100,
            channels: 2,
        };
        assert_eq!(buf.frame_count(), 44_100);
        assert!((buf.duration_seconds() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn frame_count_on_empty_buffer() {
        let buf = AudioBuffer {
            samples: vec![],
            sample_rate: 44_100,
            channels: 2,
        };
        assert_eq!(buf.frame_count(), 0);
        assert_eq!(buf.duration_seconds(), 0.0);
    }

    #[test]
    fn wavm_is_decode_only() {
        assert!(!AudioFormatKind::Wavm.is_writable());
        assert!(AudioFormatKind::Xwb.is_writable());
        assert!(AudioFormatKind::Ogg.is_writable());
    }
}
