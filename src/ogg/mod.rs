//! OGG Vorbis decode and encode.
//!
//! Decode uses `lewton` (pure Rust). Encode uses `vorbis_rs` (static
//! libvorbis via `cc`). No external tools at runtime.

pub mod decode;
pub mod encode;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum OggError {
    #[error("OGG decode error: {0}")]
    Decode(String),

    #[error("OGG encode error: {0}")]
    Encode(String),
}

#[cfg(test)]
mod tests {
    use crate::model::AudioBuffer;

    #[test]
    fn encode_decode_round_trip() {
        // Generate a short 440 Hz stereo sine wave.
        let sample_rate = 44100u32;
        let channels = 2u16;
        let duration_frames = 4410; // 0.1 seconds
        let mut samples = Vec::with_capacity(duration_frames * channels as usize);
        for frame in 0..duration_frames {
            let t = frame as f64 / sample_rate as f64;
            let val = (t * 440.0 * 2.0 * std::f64::consts::PI).sin();
            let s = (val * 16000.0) as i16;
            samples.push(s); // L
            samples.push(s); // R
        }

        let audio = AudioBuffer {
            samples: samples.clone(),
            sample_rate,
            channels,
        };

        // Encode to OGG.
        let mut ogg_bytes = Vec::new();
        super::encode::encode(&audio, &mut ogg_bytes).unwrap();
        assert!(!ogg_bytes.is_empty());

        // Decode back.
        let decoded = super::decode::decode(&ogg_bytes).unwrap();
        assert_eq!(decoded.sample_rate, sample_rate);
        assert_eq!(decoded.channels, channels);

        // Vorbis is lossy and adds encoder padding — check frame count
        // is within a reasonable range (up to ~512 frames of padding).
        let orig_frames = duration_frames;
        let dec_frames = decoded.frame_count();
        assert!(
            dec_frames >= orig_frames && dec_frames <= orig_frames + 1024,
            "frame count {dec_frames} too far from {orig_frames}"
        );

        // SNR on the overlapping region.
        let check_samples = (orig_frames * channels as usize).min(decoded.samples.len());
        let mut signal_power: f64 = 0.0;
        let mut noise_power: f64 = 0.0;
        for (orig, dec) in samples[..check_samples]
            .iter()
            .zip(decoded.samples[..check_samples].iter())
        {
            let s = *orig as f64;
            let n = *orig as f64 - *dec as f64;
            signal_power += s * s;
            noise_power += n * n;
        }
        let snr_db = if noise_power > 0.0 {
            10.0 * (signal_power / noise_power).log10()
        } else {
            f64::INFINITY
        };
        assert!(snr_db >= 10.0, "SNR {snr_db:.1} dB too low");
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(super::decode::decode(b"not an ogg file").is_err());
    }
}
