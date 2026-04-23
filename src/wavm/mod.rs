//! WAVM decoder — headerless XBOX-IMA ADPCM audio.
//!
//! WAVM is a legacy DDR audio format (Ultramix era) with no header.
//! The entire file is raw interleaved stereo XBOX-IMA ADPCM data at
//! a fixed 2 channels / 44100 Hz sample rate.

pub mod xbox_ima;

use thiserror::Error;

use crate::model::AudioBuffer;

/// Fixed parameters for WAVM files (headerless format).
const CHANNELS: u16 = 2;
const SAMPLE_RATE: u32 = 44100;

#[derive(Debug, Error)]
pub enum WavmError {
    #[error("WAVM data is empty")]
    Empty,

    #[error("WAVM data length {len} is not a multiple of the stereo block size ({block_size})")]
    UnalignedData { len: usize, block_size: usize },
}

/// Decode a headerless WAVM file into PCM.
pub fn parse(bytes: &[u8]) -> Result<AudioBuffer, WavmError> {
    if bytes.is_empty() {
        return Err(WavmError::Empty);
    }
    if !bytes.len().is_multiple_of(xbox_ima::STEREO_BLOCK_SIZE) {
        return Err(WavmError::UnalignedData {
            len: bytes.len(),
            block_size: xbox_ima::STEREO_BLOCK_SIZE,
        });
    }

    let total_samples_per_ch = xbox_ima::bytes_to_samples(bytes.len());
    let mut samples = Vec::with_capacity(total_samples_per_ch * CHANNELS as usize);

    for block in bytes.chunks_exact(xbox_ima::STEREO_BLOCK_SIZE) {
        xbox_ima::decode_stereo_block(block, &mut samples);
    }

    Ok(AudioBuffer {
        samples,
        sample_rate: SAMPLE_RATE,
        channels: CHANNELS,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic WAVM file: N stereo blocks of silence (zero nibbles).
    fn make_silent_wavm(num_blocks: usize) -> Vec<u8> {
        vec![0u8; num_blocks * xbox_ima::STEREO_BLOCK_SIZE]
    }

    #[test]
    fn parse_one_block() {
        let data = make_silent_wavm(1);
        let buf = parse(&data).unwrap();
        assert_eq!(buf.channels, 2);
        assert_eq!(buf.sample_rate, 44100);
        assert_eq!(
            buf.samples.len(),
            xbox_ima::SAMPLES_PER_BLOCK * 2 // interleaved stereo
        );
    }

    #[test]
    fn parse_multiple_blocks() {
        let data = make_silent_wavm(10);
        let buf = parse(&data).unwrap();
        assert_eq!(buf.samples.len(), xbox_ima::SAMPLES_PER_BLOCK * 2 * 10);
    }

    #[test]
    fn parse_duration_matches_expected() {
        // 100 blocks at 64 samples/block/channel at 44100 Hz
        let data = make_silent_wavm(100);
        let buf = parse(&data).unwrap();
        let frames = buf.samples.len() / buf.channels as usize;
        let duration_secs = frames as f64 / buf.sample_rate as f64;
        let expected = (100 * xbox_ima::SAMPLES_PER_BLOCK) as f64 / 44100.0;
        assert!((duration_secs - expected).abs() < 0.001);
    }

    #[test]
    fn empty_input_rejected() {
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn unaligned_input_rejected() {
        let data = vec![0u8; xbox_ima::STEREO_BLOCK_SIZE + 1];
        assert!(parse(&data).is_err());
    }
}
