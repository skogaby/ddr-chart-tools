//! MS-ADPCM per-block encoder.
//!
//! Encodes interleaved 16-bit PCM into MS-ADPCM blocks compatible with
//! XWB v43 / DDR World. Uses the same 7 standard predictor coefficient
//! pairs and adaptation table as the decoder.
//!
//! Predictor selection: for each channel, simulate encoding the full
//! block with each of the 7 predictors and pick the one with the
//! lowest total squared error.

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
    230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230,
];

/// Encode interleaved i16 PCM into MS-ADPCM blocks.
///
/// `samples` must be interleaved (`[L0, R0, L1, R1, ...]` for stereo).
/// Returns raw ADPCM bytes suitable for an XWB wave-data segment.
pub fn encode(samples: &[i16], format: &WaveFormat) -> Result<Vec<u8>, AdpcmError> {
    let ch = format.channels() as usize;
    let block_align = format.block_align() as usize;
    let spb = format.samples_per_block() as usize;

    if ch == 0 || block_align == 0 || spb == 0 {
        return Ok(Vec::new());
    }

    // De-interleave into per-channel buffers.
    let total_frames = samples.len() / ch;
    let mut channels: Vec<Vec<i16>> = (0..ch).map(|_| Vec::with_capacity(total_frames)).collect();
    for frame in samples.chunks_exact(ch) {
        for (c, &s) in frame.iter().enumerate() {
            channels[c].push(s);
        }
    }

    // Pad each channel to a multiple of spb.
    for chan in &mut channels {
        let rem = chan.len() % spb;
        if rem != 0 {
            chan.resize(chan.len() + spb - rem, 0);
        }
    }

    let num_blocks = channels[0].len() / spb;
    let mut out = Vec::with_capacity(num_blocks * block_align);

    for block_idx in 0..num_blocks {
        encode_block(&channels, ch, block_idx, spb, block_align, &mut out)?;
    }

    Ok(out)
}

/// Encode one block across all channels.
fn encode_block(
    channels: &[Vec<i16>],
    ch: usize,
    block_idx: usize,
    spb: usize,
    block_align: usize,
    out: &mut Vec<u8>,
) -> Result<(), AdpcmError> {
    let start = block_idx * spb;
    let block_start = out.len();

    // Per-channel: select predictor and compute initial state.
    struct ChannelEnc {
        pred_idx: u8,
        delta: i32,
        s1: i16,
        s2: i16,
    }

    let mut ch_enc: Vec<ChannelEnc> = Vec::with_capacity(ch);
    for chan in channels {
        let block_samples = &chan[start..start + spb];
        let (pred_idx, delta) = select_predictor(block_samples);
        ch_enc.push(ChannelEnc {
            pred_idx,
            delta,
            s1: block_samples[1],
            s2: block_samples[0],
        });
    }

    // -- Write grouped header --
    // Predictor indices.
    for ce in &ch_enc {
        out.push(ce.pred_idx);
    }
    // Deltas.
    for ce in &ch_enc {
        out.extend_from_slice(&(ce.delta as i16).to_le_bytes());
    }
    // Sample1 values.
    for ce in &ch_enc {
        out.extend_from_slice(&ce.s1.to_le_bytes());
    }
    // Sample2 values.
    for ce in &ch_enc {
        out.extend_from_slice(&ce.s2.to_le_bytes());
    }

    // -- Encode nibbles --
    // Track encoder state per channel (mirrors decoder state).
    struct EncState {
        coeff1: i32,
        coeff2: i32,
        delta: i32,
        s1: i32,
        s2: i32,
    }

    let mut states: Vec<EncState> = ch_enc
        .iter()
        .map(|ce| {
            let (c1, c2) = COEFFS[ce.pred_idx as usize];
            EncState {
                coeff1: c1,
                coeff2: c2,
                delta: ce.delta,
                s1: ce.s1 as i32,
                s2: ce.s2 as i32,
            }
        })
        .collect();

    let remaining = spb - 2;
    let mut nibble_buf: Vec<u8> = Vec::new();
    let mut pending_high: Option<u8> = None;

    for frame in 0..remaining {
        for (c, st) in states.iter_mut().enumerate() {
            let actual = channels[c][start + 2 + frame] as i32;
            let predicted = (st.s1 * st.coeff1 + st.s2 * st.coeff2) >> 8;
            let error = actual - predicted;

            // Quantize to 4-bit signed nibble (-8..7).
            let nibble_signed = if st.delta == 0 {
                0i32
            } else {
                (error / st.delta).clamp(-8, 7)
            };
            let nibble = (nibble_signed & 0xF) as u8;

            // Reconstruct (must match decoder exactly).
            let reconstructed = (predicted + nibble_signed * st.delta).clamp(-32768, 32767);
            st.s2 = st.s1;
            st.s1 = reconstructed;
            st.delta = ((st.delta * ADAPT[nibble as usize]) >> 8).max(16);

            // Pack nibbles: high nibble first.
            match pending_high {
                None => pending_high = Some(nibble),
                Some(hi) => {
                    nibble_buf.push((hi << 4) | nibble);
                    pending_high = None;
                }
            }
        }
    }
    // Flush any trailing high nibble.
    if let Some(hi) = pending_high {
        nibble_buf.push(hi << 4);
    }

    out.extend_from_slice(&nibble_buf);

    // Pad block to exact block_align.
    let written = out.len() - block_start;
    if written < block_align {
        out.resize(out.len() + block_align - written, 0);
    }

    Ok(())
}

/// Try all 7 predictors on a block's samples, return (best_index, initial_delta).
fn select_predictor(samples: &[i16]) -> (u8, i32) {
    if samples.len() < 2 {
        return (0, 16);
    }

    let mut best_idx = 0u8;
    let mut best_err = i64::MAX;
    let mut best_delta = 16i32;

    for (idx, &(c1, c2)) in COEFFS.iter().enumerate() {
        let (err, delta) = simulate(samples, c1, c2);
        if err < best_err {
            best_err = err;
            best_idx = idx as u8;
            best_delta = delta;
        }
    }

    (best_idx, best_delta)
}

/// Simulate encoding a block with given coefficients.
/// Returns (total_squared_error, initial_delta).
fn simulate(samples: &[i16], c1: i32, c2: i32) -> (i64, i32) {
    let s2 = samples[0] as i32;
    let s1 = samples[1] as i32;

    // Initial delta from the first prediction error.
    let pred2 = (s1 * c1 + s2 * c2) >> 8;
    let initial_err = if samples.len() > 2 {
        (samples[2] as i32 - pred2).unsigned_abs() as i32
    } else {
        0
    };
    let mut delta = (initial_err / 4).max(16);

    let mut prev1 = s1;
    let mut prev2 = s2;
    let mut total_err: i64 = 0;

    for &sample in &samples[2..] {
        let actual = sample as i32;
        let predicted = (prev1 * c1 + prev2 * c2) >> 8;
        let error = actual - predicted;

        let nibble_signed = if delta == 0 {
            0
        } else {
            (error / delta).clamp(-8, 7)
        };
        let nibble = (nibble_signed & 0xF) as u8;
        let reconstructed = (predicted + nibble_signed * delta).clamp(-32768, 32767);

        let quant_err = (actual - reconstructed) as i64;
        total_err += quant_err * quant_err;

        prev2 = prev1;
        prev1 = reconstructed;
        delta = ((delta * ADAPT[nibble as usize]) >> 8).max(16);
    }

    (total_err, (initial_err / 4).max(16))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xwb::adpcm::decode;

    fn stereo_format() -> WaveFormat {
        // ADPCM, 2ch, 44100Hz, raw_align=48
        let bits: u32 = 2 | (2 << 2) | (44100 << 5) | (48 << 23);
        WaveFormat::from_packed(bits)
    }

    fn mono_format() -> WaveFormat {
        let bits: u32 = 2 | (1 << 2) | (44100 << 5) | (48 << 23);
        WaveFormat::from_packed(bits)
    }

    #[test]
    fn encode_silence_round_trips() {
        let fmt = mono_format();
        let spb = fmt.samples_per_block() as usize;
        let pcm = vec![0i16; spb];

        let adpcm = encode(&pcm, &fmt).unwrap();
        assert_eq!(adpcm.len(), fmt.block_align() as usize);

        let decoded = decode::decode(&adpcm, &fmt).unwrap();
        assert_eq!(decoded.len(), spb);
        // Silence in → silence out.
        assert!(decoded.iter().all(|&s| s == 0));
    }

    #[test]
    fn encode_decode_round_trip_snr() {
        let fmt = stereo_format();
        let spb = fmt.samples_per_block() as usize;
        let ch = fmt.channels() as usize;

        // Generate a 440 Hz sine wave, 1 block of stereo.
        let total_samples = spb * ch;
        let mut pcm = Vec::with_capacity(total_samples);
        for frame in 0..spb {
            let t = frame as f64 / 44100.0;
            let val = (t * 440.0 * 2.0 * std::f64::consts::PI).sin();
            let sample = (val * 16000.0) as i16;
            pcm.push(sample); // L
            pcm.push(sample); // R
        }

        let adpcm = encode(&pcm, &fmt).unwrap();
        let decoded = decode::decode(&adpcm, &fmt).unwrap();
        assert_eq!(decoded.len(), total_samples);

        // Compute SNR.
        let mut signal_power: f64 = 0.0;
        let mut noise_power: f64 = 0.0;
        for (orig, dec) in pcm.iter().zip(decoded.iter()) {
            let s = *orig as f64;
            let n = (*orig as f64) - (*dec as f64);
            signal_power += s * s;
            noise_power += n * n;
        }
        let snr_db = if noise_power > 0.0 {
            10.0 * (signal_power / noise_power).log10()
        } else {
            f64::INFINITY
        };

        assert!(
            snr_db >= 30.0,
            "SNR {snr_db:.1} dB is below 30 dB threshold"
        );
    }

    #[test]
    fn encode_empty_input() {
        let fmt = stereo_format();
        let adpcm = encode(&[], &fmt).unwrap();
        assert!(adpcm.is_empty());
    }

    #[test]
    fn encode_pads_partial_block() {
        let fmt = mono_format();
        let spb = fmt.samples_per_block() as usize;
        // Feed half a block — encoder should pad to full block.
        let pcm = vec![1000i16; spb / 2];
        let adpcm = encode(&pcm, &fmt).unwrap();
        assert_eq!(adpcm.len(), fmt.block_align() as usize);
    }

    #[test]
    fn encode_multiple_blocks() {
        let fmt = mono_format();
        let spb = fmt.samples_per_block() as usize;
        let pcm = vec![0i16; spb * 3];
        let adpcm = encode(&pcm, &fmt).unwrap();
        assert_eq!(adpcm.len(), fmt.block_align() as usize * 3);
    }
}
