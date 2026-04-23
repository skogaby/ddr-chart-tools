//! OGG Vorbis encoder using `vorbis_rs` (libvorbis via static C link).

use std::io::Write;
use std::num::NonZero;

use vorbis_rs::VorbisEncoderBuilder;

use super::OggError;
use crate::model::AudioBuffer;

/// Encode PCM audio to OGG Vorbis.
///
/// Uses quality-based VBR. The quality parameter maps roughly to
/// ~128 kbps at 0.4 for stereo 44.1 kHz — a reasonable default that
/// matches typical StepMania community expectations.
pub fn encode(audio: &AudioBuffer, out: &mut impl Write) -> Result<(), OggError> {
    let channels = NonZero::new(audio.channels as u8)
        .ok_or_else(|| OggError::Encode("channel count must be non-zero".into()))?;
    let sample_rate = NonZero::new(audio.sample_rate)
        .ok_or_else(|| OggError::Encode("sample rate must be non-zero".into()))?;

    let mut encoder = VorbisEncoderBuilder::new(
        sample_rate,
        channels,
        out,
    )
    .map_err(|e| OggError::Encode(e.to_string()))?
    .build()
    .map_err(|e| OggError::Encode(e.to_string()))?;

    // Convert interleaved i16 to planar f32 in chunks.
    let ch = channels.get() as usize;
    let frames = audio.samples.len() / ch.max(1);
    let chunk_frames = 4096;

    for chunk_start in (0..frames).step_by(chunk_frames) {
        let chunk_end = (chunk_start + chunk_frames).min(frames);
        let n = chunk_end - chunk_start;

        let mut planar: Vec<Vec<f32>> = (0..ch).map(|_| Vec::with_capacity(n)).collect();
        for frame in chunk_start..chunk_end {
            for (c, ch_buf) in planar.iter_mut().enumerate() {
                let sample = audio.samples[frame * ch + c];
                ch_buf.push(sample as f32 / 32768.0);
            }
        }

        let slices: Vec<&[f32]> = planar.iter().map(|v| v.as_slice()).collect();
        encoder
            .encode_audio_block(slices)
            .map_err(|e| OggError::Encode(e.to_string()))?;
    }

    encoder
        .finish()
        .map_err(|e| OggError::Encode(e.to_string()))?;

    Ok(())
}
