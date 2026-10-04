//! RIFF WAVE decoder — 16-bit PCM only.
//!
//! Used for `DDR_LEGACY` inputs whose audio was ripped to plain WAV
//! (DDR Hottest Party on Wii, among others). Accepts format tag 1 (PCM),
//! or `WAVE_FORMAT_EXTENSIBLE` with the PCM sub-format GUID, at 16 bits
//! per sample, any channel count and sample rate. Channel-layout and rate
//! policy belong to the job layer, not here.
//!
//! Does not own other WAV encodings (ADPCM, float, 24-bit), WAV writing,
//! or resampling.

use thiserror::Error;

use crate::model::AudioBuffer;
use crate::util::io::{IoError, LeReader};

/// `WAVE_FORMAT_PCM`.
const FORMAT_PCM: u16 = 1;
/// `WAVE_FORMAT_EXTENSIBLE`; the real format is in the sub-format GUID.
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;
/// `KSDATAFORMAT_SUBTYPE_PCM` as stored on disk.
const SUBFORMAT_PCM: [u8; 16] = [
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
];
/// Only bit depth this decoder accepts.
const BITS_PER_SAMPLE: u16 = 16;
/// Size of the base `fmt ` payload (through `bits_per_sample`).
const FMT_BASE_SIZE: usize = 16;
/// Size of an extensible `fmt ` payload (base + cbSize + 22 bytes).
const FMT_EXTENSIBLE_SIZE: usize = 40;

/// Errors from decoding a WAV file. Offsets are bytes from file start.
#[derive(Debug, Error)]
pub enum WavError {
    #[error("I/O error while parsing WAV: {0}")]
    Io(#[from] IoError),

    #[error("not a RIFF/WAVE file (header bytes {found:?})")]
    NotRiffWave { found: Vec<u8> },

    #[error(
        "WAV chunk {id:?} at byte {offset} declares {declared} bytes but only {remaining} remain"
    )]
    TruncatedChunk {
        id: [u8; 4],
        offset: usize,
        declared: usize,
        remaining: usize,
    },

    #[error("WAV file has no {0:?} chunk")]
    MissingChunk(&'static str),

    #[error("WAV fmt chunk at byte {offset} is {len} bytes, too short for {needed}")]
    ShortFmt {
        offset: usize,
        len: usize,
        needed: usize,
    },

    #[error("WAV fmt chunk at byte {offset}: unsupported encoding (format tag 0x{format_tag:04X}, {bits} bits); only 16-bit PCM is supported")]
    UnsupportedEncoding {
        offset: usize,
        format_tag: u16,
        bits: u16,
    },

    #[error("WAV fmt chunk at byte {offset}: invalid layout ({channels} channels, block align {block_align})")]
    InvalidLayout {
        offset: usize,
        channels: u16,
        block_align: u16,
    },

    #[error("WAV data chunk at byte {offset}: {len} bytes is not a whole number of {block_align}-byte frames")]
    UnalignedData {
        offset: usize,
        len: usize,
        block_align: usize,
    },
}

/// Recognize a RIFF/WAVE header without decoding.
#[must_use]
pub fn is_wav(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE"
}

/// The parts of a `fmt ` chunk this decoder uses.
#[derive(Debug, Clone, Copy)]
struct Fmt {
    channels: u16,
    sample_rate: u32,
    block_align: u16,
}

/// Decode a 16-bit PCM RIFF/WAVE file into interleaved samples.
pub fn parse(bytes: &[u8]) -> Result<AudioBuffer, WavError> {
    if !is_wav(bytes) {
        return Err(WavError::NotRiffWave {
            found: bytes.iter().take(12).copied().collect(),
        });
    }

    let mut reader = LeReader::new(bytes);
    reader.read_bytes(12)?; // "RIFF", size (not trusted), "WAVE"

    let mut fmt: Option<Fmt> = None;
    let mut data: Option<(usize, &[u8])> = None;
    // Walk sub-chunks to the end of the buffer. Trailing bytes too short
    // for a chunk header are ignored.
    while reader.remaining() >= 8 {
        let offset = reader.position();
        let id_bytes = reader.read_bytes(4)?;
        let id = [id_bytes[0], id_bytes[1], id_bytes[2], id_bytes[3]];
        let declared = reader.read_u32()? as usize;
        if declared > reader.remaining() {
            return Err(WavError::TruncatedChunk {
                id,
                offset,
                declared,
                remaining: reader.remaining(),
            });
        }
        let payload = reader.read_bytes(declared)?;
        // RIFF pads odd-sized chunks to an even boundary.
        if declared % 2 == 1 && reader.remaining() > 0 {
            reader.read_u8()?;
        }
        match &id {
            b"fmt " if fmt.is_none() => fmt = Some(parse_fmt(payload, offset)?),
            b"data" if data.is_none() => data = Some((offset, payload)),
            _ => log::debug!("WAV: skipping chunk {id:?} at byte {offset}"),
        }
    }

    let fmt = fmt.ok_or(WavError::MissingChunk("fmt "))?;
    let (data_offset, data) = data.ok_or(WavError::MissingChunk("data"))?;
    let block_align = usize::from(fmt.block_align);
    if !data.len().is_multiple_of(block_align) {
        return Err(WavError::UnalignedData {
            offset: data_offset,
            len: data.len(),
            block_align,
        });
    }

    let samples = data
        .chunks_exact(2)
        .map(|s| i16::from_le_bytes([s[0], s[1]]))
        .collect();
    Ok(AudioBuffer {
        samples,
        sample_rate: fmt.sample_rate,
        channels: fmt.channels,
    })
}

/// Decode and validate a `fmt ` payload. `offset` is the chunk's file
/// offset, for errors.
fn parse_fmt(payload: &[u8], offset: usize) -> Result<Fmt, WavError> {
    if payload.len() < FMT_BASE_SIZE {
        return Err(WavError::ShortFmt {
            offset,
            len: payload.len(),
            needed: FMT_BASE_SIZE,
        });
    }
    let mut r = LeReader::new(payload);
    let format_tag = r.read_u16()?;
    let channels = r.read_u16()?;
    let sample_rate = r.read_u32()?;
    let _byte_rate = r.read_u32()?;
    let block_align = r.read_u16()?;
    let bits = r.read_u16()?;

    let is_pcm = match format_tag {
        FORMAT_PCM => true,
        FORMAT_EXTENSIBLE => {
            if payload.len() < FMT_EXTENSIBLE_SIZE {
                return Err(WavError::ShortFmt {
                    offset,
                    len: payload.len(),
                    needed: FMT_EXTENSIBLE_SIZE,
                });
            }
            payload[24..40] == SUBFORMAT_PCM
        }
        _ => false,
    };
    if !is_pcm || bits != BITS_PER_SAMPLE {
        return Err(WavError::UnsupportedEncoding {
            offset,
            format_tag,
            bits,
        });
    }
    if channels == 0 || u32::from(block_align) != u32::from(channels) * 2 {
        return Err(WavError::InvalidLayout {
            offset,
            channels,
            block_align,
        });
    }
    Ok(Fmt {
        channels,
        sample_rate,
        block_align,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn fmt_payload(tag: u16, channels: u16, rate: u32, bits: u16) -> Vec<u8> {
        let align = channels * bits / 8;
        let mut v = Vec::new();
        v.extend_from_slice(&tag.to_le_bytes());
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&(rate * u32::from(align)).to_le_bytes());
        v.extend_from_slice(&align.to_le_bytes());
        v.extend_from_slice(&bits.to_le_bytes());
        v
    }

    fn riff(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut body = b"WAVE".to_vec();
        for (id, payload) in chunks {
            body.extend_from_slice(*id);
            body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            body.extend_from_slice(payload);
            if payload.len() % 2 == 1 {
                body.push(0);
            }
        }
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    fn pcm(samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    #[test]
    fn decodes_stereo_32k_pcm() -> TestResult {
        let bytes = riff(&[
            (b"fmt ", fmt_payload(FORMAT_PCM, 2, 32_000, 16)),
            (b"data", pcm(&[1, -2, 300, -32768])),
        ]);
        let audio = parse(&bytes)?;
        assert_eq!(audio.sample_rate, 32_000);
        assert_eq!(audio.channels, 2);
        assert_eq!(audio.samples, [1, -2, 300, -32768]);
        Ok(())
    }

    #[test]
    fn skips_unknown_and_odd_sized_chunks() -> TestResult {
        let bytes = riff(&[
            (b"fmt ", fmt_payload(FORMAT_PCM, 1, 44_100, 16)),
            (b"LIST", vec![1, 2, 3]),
            (b"data", pcm(&[7, 8])),
        ]);
        let audio = parse(&bytes)?;
        assert_eq!(audio.channels, 1);
        assert_eq!(audio.samples, [7, 8]);
        Ok(())
    }

    #[test]
    fn accepts_extensible_pcm() -> TestResult {
        let mut fmt = fmt_payload(FORMAT_EXTENSIBLE, 2, 48_000, 16);
        fmt.extend_from_slice(&22u16.to_le_bytes()); // cbSize
        fmt.extend_from_slice(&16u16.to_le_bytes()); // valid bits
        fmt.extend_from_slice(&3u32.to_le_bytes()); // channel mask
        fmt.extend_from_slice(&SUBFORMAT_PCM);
        let bytes = riff(&[(b"fmt ", fmt), (b"data", pcm(&[1, 2]))]);
        assert_eq!(parse(&bytes)?.sample_rate, 48_000);
        Ok(())
    }

    #[test]
    fn rejects_non_pcm_and_other_bit_depths() {
        let float = riff(&[(b"fmt ", fmt_payload(3, 2, 44_100, 32)), (b"data", vec![])]);
        assert!(matches!(
            parse(&float),
            Err(WavError::UnsupportedEncoding { .. })
        ));
        let pcm24 = riff(&[
            (b"fmt ", fmt_payload(FORMAT_PCM, 2, 44_100, 24)),
            (b"data", vec![]),
        ]);
        assert!(matches!(
            parse(&pcm24),
            Err(WavError::UnsupportedEncoding { .. })
        ));
    }

    #[test]
    fn rejects_missing_chunks_and_bad_magic() {
        let no_data = riff(&[(b"fmt ", fmt_payload(FORMAT_PCM, 2, 44_100, 16))]);
        assert!(matches!(
            parse(&no_data),
            Err(WavError::MissingChunk("data"))
        ));
        assert!(matches!(
            parse(b"OggS\0\0\0\0\0\0\0\0"),
            Err(WavError::NotRiffWave { .. })
        ));
    }

    #[test]
    fn rejects_truncated_data_chunk() {
        let mut bytes = riff(&[
            (b"fmt ", fmt_payload(FORMAT_PCM, 2, 44_100, 16)),
            (b"data", pcm(&[1, 2, 3, 4])),
        ]);
        bytes.truncate(bytes.len() - 2);
        assert!(matches!(
            parse(&bytes),
            Err(WavError::TruncatedChunk { .. })
        ));
    }

    #[test]
    fn rejects_partial_frames() {
        let bytes = riff(&[
            (b"fmt ", fmt_payload(FORMAT_PCM, 2, 44_100, 16)),
            (b"data", pcm(&[1, 2, 3])),
        ]);
        assert!(matches!(parse(&bytes), Err(WavError::UnalignedData { .. })));
    }
}
