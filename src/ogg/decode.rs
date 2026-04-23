//! OGG Vorbis decoder using `lewton` (pure Rust).

use std::io::Cursor;

use lewton::inside_ogg::OggStreamReader;

use super::OggError;
use crate::model::AudioBuffer;

/// Decode an OGG Vorbis file from raw bytes into PCM.
pub fn decode(bytes: &[u8]) -> Result<AudioBuffer, OggError> {
    let mut reader =
        OggStreamReader::new(Cursor::new(bytes)).map_err(|e| OggError::Decode(e.to_string()))?;

    let channels = reader.ident_hdr.audio_channels as u16;
    let sample_rate = reader.ident_hdr.audio_sample_rate;

    let mut samples: Vec<i16> = Vec::new();
    while let Some(packet) = reader
        .read_dec_packet_itl()
        .map_err(|e| OggError::Decode(e.to_string()))?
    {
        samples.extend_from_slice(&packet);
    }

    Ok(AudioBuffer {
        samples,
        sample_rate,
        channels,
    })
}
