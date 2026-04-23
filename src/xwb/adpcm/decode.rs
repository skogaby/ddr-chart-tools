//! MS-ADPCM block decoder.
//!
//! Each block has a 7-byte-per-channel header followed by 4-bit
//! nibbles. The XWB variant groups header fields by type across
//! channels rather than interleaving per channel.
//!
//! Block header layout (for N channels):
//!   [pred_0..pred_N-1]          1 byte each — predictor index
//!   [delta_0..delta_N-1]        2 bytes each (i16 LE) — initial step
//!   [sample1_0..sample1_N-1]    2 bytes each (i16 LE) — newest sample
//!   [sample2_0..sample2_N-1]    2 bytes each (i16 LE) — second-newest
//!
//! Nibble data is interleaved by frame: for each frame, one nibble per
//! channel, high nibble first within each byte.

use super::AdpcmError;
use crate::xwb::container::WaveFormat;

/// Standard MS-ADPCM predictor coefficient pairs (coeff1, coeff2).
const COEFFS: [(i32, i32); 7] = [
    (256, 0),
    (512, -256),
    (0, 0),
    (192, 64),
    (240, 0),
    (460, -208),
    (392, -232),
];

/// Adaptation table indexed by unsigned nibble value (0..16).
const ADAPT: [i32; 16] = [
    230, 230, 230, 230, 307, 409, 512, 614,
    768, 614, 512, 409, 307, 230, 230, 230,
];

/// Per-channel decoder state.
struct ChannelState {
    coeff1: i32,
    coeff2: i32,
    delta: i32,
    sample1: i32,
    sample2: i32,
}

/// Decode all ADPCM blocks into interleaved i16 PCM.
///
/// Returns `(channels, sample_rate, samples)` where `samples` is
/// interleaved: `[L0, R0, L1, R1, ...]` for stereo.
pub fn decode(data: &[u8], format: &WaveFormat) -> Result<Vec<i16>, AdpcmError> {
    let ch = format.channels() as usize;
    let block_align = format.block_align() as usize;
    let spb = format.samples_per_block() as usize;

    if block_align == 0 || ch == 0 || spb == 0 {
        return Ok(Vec::new());
    }

    let num_blocks = data.len() / block_align;
    let mut out = Vec::with_capacity(num_blocks * spb * ch);

    for block_idx in 0..num_blocks {
        let block = &data[block_idx * block_align..][..block_align];
        decode_block(block, ch, spb, &mut out)?;
    }

    Ok(out)
}

/// Decode a single ADPCM block, appending interleaved samples to `out`.
fn decode_block(
    block: &[u8],
    ch: usize,
    samples_per_block: usize,
    out: &mut Vec<i16>,
) -> Result<(), AdpcmError> {
    let header_size = 7 * ch;
    if block.len() < header_size {
        return Err(AdpcmError::BlockTooShort {
            expected: header_size,
            actual: block.len(),
        });
    }

    // -- Parse grouped header --
    let mut states = Vec::with_capacity(ch);
    let mut off = 0;

    // Predictor indices (1 byte each).
    let pred_indices: Vec<u8> = (0..ch).map(|i| block[off + i]).collect();
    off += ch;

    // Deltas (i16 LE each).
    let mut deltas = Vec::with_capacity(ch);
    for _ in 0..ch {
        deltas.push(i16::from_le_bytes([block[off], block[off + 1]]) as i32);
        off += 2;
    }

    // Sample1 values (i16 LE each).
    let mut s1 = Vec::with_capacity(ch);
    for _ in 0..ch {
        s1.push(i16::from_le_bytes([block[off], block[off + 1]]) as i32);
        off += 2;
    }

    // Sample2 values (i16 LE each).
    for c in 0..ch {
        let s2 = i16::from_le_bytes([block[off], block[off + 1]]) as i32;
        off += 2;

        let pi = pred_indices[c];
        if pi as usize >= COEFFS.len() {
            return Err(AdpcmError::BadPredictor { index: pi });
        }
        let (c1, c2) = COEFFS[pi as usize];

        states.push(ChannelState {
            coeff1: c1,
            coeff2: c2,
            delta: deltas[c],
            sample1: s1[c],
            sample2: s2,
        });
    }

    // -- Output the two header samples (sample2 first, then sample1) --
    for st in &states {
        out.push(st.sample2 as i16);
    }
    for st in &states {
        out.push(st.sample1 as i16);
    }

    // -- Decode nibble data --
    let nibble_data = &block[off..];
    let remaining_frames = samples_per_block.saturating_sub(2);
    let mut byte_idx = 0;
    let mut high = true; // high nibble first

    for _ in 0..remaining_frames {
        for st in &mut states {
            if byte_idx >= nibble_data.len() {
                out.push(0);
                continue;
            }

            let nibble = if high {
                (nibble_data[byte_idx] >> 4) & 0x0F
            } else {
                let n = nibble_data[byte_idx] & 0x0F;
                byte_idx += 1;
                n
            };
            high = !high;

            let signed = if nibble >= 8 {
                nibble as i32 - 16
            } else {
                nibble as i32
            };

            let predicted =
                ((st.sample1 * st.coeff1 + st.sample2 * st.coeff2) >> 8) + signed * st.delta;
            let clamped = predicted.clamp(-32768, 32767);

            out.push(clamped as i16);

            st.sample2 = st.sample1;
            st.sample1 = clamped;
            st.delta = ((st.delta * ADAPT[nibble as usize]) >> 8).max(16);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal 1-channel ADPCM block.
    /// predictor=2 (coeffs 0,0) so output = nibble_signed * delta.
    fn build_mono_block(delta: i16, s1: i16, s2: i16, nibbles: &[u8]) -> Vec<u8> {
        let mut block = Vec::new();
        block.push(2); // predictor index 2 → (0, 0)
        block.extend_from_slice(&delta.to_le_bytes());
        block.extend_from_slice(&s1.to_le_bytes());
        block.extend_from_slice(&s2.to_le_bytes());
        block.extend_from_slice(nibbles);
        block
    }

    /// Build a WaveFormat for mono, 128 samples/block.
    fn mono_format() -> WaveFormat {
        // codec=2, channels=1, rate=44100, raw_align=((block_align/1)-22)=block_align-22
        // For 1ch 128spb: block_align = 7 + (128-2)/2 = 7+63 = 70
        // raw = 70/1 - 22 = 48
        let bits: u32 = 2 | (1 << 2) | (44100 << 5) | (48 << 23);
        WaveFormat::from_packed(bits)
    }

    #[test]
    fn mono_format_params() {
        let f = mono_format();
        assert_eq!(f.channels(), 1);
        assert_eq!(f.block_align(), 70); // (48+22)*1
        assert_eq!(f.samples_per_block(), 128); // ((70-7)*8)/4 + 2
    }

    #[test]
    fn decode_mono_silence_block() {
        // predictor=2 (0,0), delta=1, s1=0, s2=0, all-zero nibbles
        let nibble_bytes = vec![0u8; 63]; // 126 nibbles = 126 frames
        let block = build_mono_block(1, 0, 0, &nibble_bytes);
        assert_eq!(block.len(), 70);

        let fmt = mono_format();
        let pcm = decode(&block, &fmt).unwrap();
        assert_eq!(pcm.len(), 128);
        // First two samples are s2=0, s1=0; rest are 0 (nibble=0, coeff=0)
        assert!(pcm.iter().all(|&s| s == 0));
    }

    #[test]
    fn decode_mono_known_nibbles() {
        // predictor=2 (0,0), delta=100, s1=500, s2=200
        // With coeff=(0,0): predicted = signed_nibble * delta
        // First nibble byte 0x31 → high=3, low=1
        // Frame 0: nibble=3 → predicted = 3*100 = 300
        // Frame 1: nibble=1 → predicted = 1*100 = 100 (delta updated after frame 0)
        let nibble_bytes = {
            let mut v = vec![0u8; 63];
            v[0] = 0x31; // high nibble=3, low nibble=1
            v
        };
        let block = build_mono_block(100, 500, 200, &nibble_bytes);
        let fmt = mono_format();
        let pcm = decode(&block, &fmt).unwrap();

        assert_eq!(pcm[0], 200);  // s2
        assert_eq!(pcm[1], 500);  // s1
        assert_eq!(pcm[2], 300);  // nibble=3, signed=3, 3*100=300
        // After frame 0: delta = max(16, (100*ADAPT[3])>>8) = max(16, (100*230)>>8) = 89
        assert_eq!(pcm[3], 89);   // nibble=1, signed=1, 1*89=89
    }

    #[test]
    fn decode_stereo_interleaved() {
        // 2-channel block: predictor=2 for both, delta=1, s1/s2 differ per channel
        let mut block = Vec::new();
        // Predictors
        block.push(2); // ch0
        block.push(2); // ch1
        // Deltas
        block.extend_from_slice(&1i16.to_le_bytes()); // ch0
        block.extend_from_slice(&1i16.to_le_bytes()); // ch1
        // Sample1
        block.extend_from_slice(&100i16.to_le_bytes()); // ch0
        block.extend_from_slice(&200i16.to_le_bytes()); // ch1
        // Sample2
        block.extend_from_slice(&10i16.to_le_bytes()); // ch0
        block.extend_from_slice(&20i16.to_le_bytes()); // ch1
        // Nibble data: all zeros
        // For 2ch, 128spb: block_align = (48+22)*2 = 140, header = 14
        // nibble bytes = 140 - 14 = 126
        block.resize(140, 0);

        // Use the DDR format (2ch)
        let bits: u32 = 2 | (2 << 2) | (44100 << 5) | (48 << 23);
        let fmt = WaveFormat::from_packed(bits);
        let pcm = decode(&block, &fmt).unwrap();

        // 128 frames × 2 channels = 256 samples
        assert_eq!(pcm.len(), 256);
        // Frame 0: [ch0.s2, ch1.s2] = [10, 20]
        assert_eq!(pcm[0], 10);
        assert_eq!(pcm[1], 20);
        // Frame 1: [ch0.s1, ch1.s1] = [100, 200]
        assert_eq!(pcm[2], 100);
        assert_eq!(pcm[3], 200);
    }

    #[test]
    fn decode_rejects_bad_predictor() {
        let mut block = vec![0u8; 70];
        block[0] = 7; // invalid predictor index
        let fmt = mono_format();
        assert!(matches!(
            decode(&block, &fmt),
            Err(AdpcmError::BadPredictor { index: 7 })
        ));
    }

    #[test]
    fn decode_empty_data() {
        let fmt = mono_format();
        let pcm = decode(&[], &fmt).unwrap();
        assert!(pcm.is_empty());
    }

    #[test]
    fn decode_clamps_to_i16_range() {
        // predictor=2 (0,0), delta=32767, s1=0, s2=0
        // nibble=7 → signed=7, predicted = 7*32767 = 229369 → clamped to 32767
        let mut nibble_bytes = vec![0u8; 63];
        nibble_bytes[0] = 0x70; // high=7, low=0
        let block = build_mono_block(32767, 0, 0, &nibble_bytes);
        let fmt = mono_format();
        let pcm = decode(&block, &fmt).unwrap();
        assert_eq!(pcm[2], 32767); // clamped
    }

    #[test]
    fn decode_negative_nibble() {
        // predictor=2 (0,0), delta=100, s1=0, s2=0
        // nibble=0xF → signed = 15-16 = -1, predicted = -1*100 = -100
        let mut nibble_bytes = vec![0u8; 63];
        nibble_bytes[0] = 0xF0; // high=15, low=0
        let block = build_mono_block(100, 0, 0, &nibble_bytes);
        let fmt = mono_format();
        let pcm = decode(&block, &fmt).unwrap();
        assert_eq!(pcm[2], -100);
    }
}
