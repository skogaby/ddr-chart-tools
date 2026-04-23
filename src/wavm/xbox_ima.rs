//! XBOX-IMA ADPCM decoder.
//!
//! Decodes interleaved stereo XBOX-IMA blocks into 16-bit PCM samples.
//! Algorithm ported from vgmstream's `ima_decoder.c`.
//!
//! Block layout (stereo, 0x48 bytes total):
//!   - 4 bytes: channel 0 header (i16le history, u8 step_index, u8 reserved)
//!   - 4 bytes: channel 1 header
//!   - 32 bytes: channel 0 nibble data (4-byte interleaved chunks)
//!   - 32 bytes: channel 1 nibble data (4-byte interleaved chunks)
//!
//! Nibble data is interleaved in 4-byte chunks per channel:
//!   [4 bytes ch0][4 bytes ch1][4 bytes ch0][4 bytes ch1]...
//!
//! Each block produces 64 samples per channel (1 from header + 63 from
//! nibbles; the last nibble is skipped per spec).

/// Stereo block size in bytes.
pub const STEREO_BLOCK_SIZE: usize = 0x48;

/// Samples decoded per channel per block.
pub const SAMPLES_PER_BLOCK: usize = 64;

static STEP_TABLE: [i16; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14,
    16, 17, 19, 21, 23, 25, 28, 31,
    34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143,
    157, 173, 190, 209, 230, 253, 279, 307,
    337, 371, 408, 449, 494, 544, 598, 658,
    724, 796, 876, 963, 1060, 1166, 1282, 1411,
    1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024,
    3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484,
    7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899,
    15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794,
    32767,
];

static INDEX_TABLE: [i8; 16] = [
    -1, -1, -1, -1, 2, 4, 6, 8,
    -1, -1, -1, -1, 2, 4, 6, 8,
];

fn clamp16(v: i32) -> i16 {
    v.clamp(-32768, 32767) as i16
}

/// Expand one 4-bit nibble into a PCM sample, updating history and step index.
fn expand_nibble(nibble: u8, hist: &mut i32, step_index: &mut i32) -> i16 {
    let step = STEP_TABLE[*step_index as usize] as i32;
    let mut delta = step >> 3;
    if nibble & 1 != 0 { delta += step >> 2; }
    if nibble & 2 != 0 { delta += step >> 1; }
    if nibble & 4 != 0 { delta += step; }
    if nibble & 8 != 0 { delta = -delta; }
    *hist += delta;
    let sample = clamp16(*hist);
    *hist = sample as i32;
    *step_index += INDEX_TABLE[nibble as usize] as i32;
    *step_index = (*step_index).clamp(0, 88);
    sample
}

/// Decode one stereo XBOX-IMA block (0x48 bytes) into interleaved PCM.
/// Appends `SAMPLES_PER_BLOCK * 2` samples (L, R, L, R, ...) to `out`.
pub fn decode_stereo_block(block: &[u8], out: &mut Vec<i16>) {
    debug_assert!(block.len() >= STEREO_BLOCK_SIZE);

    let mut ch_samples: [[i16; SAMPLES_PER_BLOCK]; 2] = [[0; SAMPLES_PER_BLOCK]; 2];

    // Each channel is decoded independently; `ch` indexes both the
    // header offset and the nibble-chunk interleave pattern.
    #[allow(clippy::needless_range_loop)]
    for ch in 0..2usize {
        let header_off = ch * 4;
        let mut hist = i16::from_le_bytes([block[header_off], block[header_off + 1]]) as i32;
        let mut step_index = block[header_off + 2] as i32;
        step_index = step_index.clamp(0, 88);

        // First sample is the header history value.
        ch_samples[ch][0] = hist as i16;

        // Nibble data: 4-byte chunks interleaved between channels.
        // Channel 0 chunks at offsets 8, 16, 24, 32, 40, 48, 56, 64
        // Channel 1 chunks at offsets 12, 20, 28, 36, 44, 52, 60, 68
        let mut sample_idx = 1usize;
        for chunk in 0..8usize {
            let chunk_off = 8 + chunk * 8 + ch * 4;
            for byte_in_chunk in 0..4usize {
                let byte = block[chunk_off + byte_in_chunk];
                // Low nibble first, then high nibble.
                if sample_idx < SAMPLES_PER_BLOCK {
                    ch_samples[ch][sample_idx] = expand_nibble(byte & 0x0F, &mut hist, &mut step_index);
                    sample_idx += 1;
                }
                if sample_idx < SAMPLES_PER_BLOCK {
                    ch_samples[ch][sample_idx] = expand_nibble(byte >> 4, &mut hist, &mut step_index);
                    sample_idx += 1;
                }
            }
        }
    }

    // Interleave: L0 R0 L1 R1 ...
    for (l, r) in ch_samples[0].iter().zip(ch_samples[1].iter()) {
        out.push(*l);
        out.push(*r);
    }
}

/// Calculate the total number of PCM samples (per channel) from a byte
/// length of stereo XBOX-IMA data.
pub fn bytes_to_samples(byte_len: usize) -> usize {
    let full_blocks = byte_len / STEREO_BLOCK_SIZE;
    full_blocks * SAMPLES_PER_BLOCK
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal stereo block with known header values and all-zero
    /// nibble data. Zero nibbles produce delta = step>>3 with sign=0,
    /// so samples drift upward slightly from the header value.
    fn make_silent_block(hist_l: i16, hist_r: i16) -> [u8; STEREO_BLOCK_SIZE] {
        let mut block = [0u8; STEREO_BLOCK_SIZE];
        let l = hist_l.to_le_bytes();
        let r = hist_r.to_le_bytes();
        block[0] = l[0]; block[1] = l[1]; block[2] = 0; // step_index=0
        block[4] = r[0]; block[5] = r[1]; block[6] = 0;
        block
    }

    #[test]
    fn decode_produces_correct_sample_count() {
        let block = make_silent_block(0, 0);
        let mut out = Vec::new();
        decode_stereo_block(&block, &mut out);
        // 64 samples per channel * 2 channels = 128 interleaved samples
        assert_eq!(out.len(), SAMPLES_PER_BLOCK * 2);
    }

    #[test]
    fn header_sample_is_first_output() {
        let block = make_silent_block(1000, -500);
        let mut out = Vec::new();
        decode_stereo_block(&block, &mut out);
        assert_eq!(out[0], 1000);  // L channel first sample
        assert_eq!(out[1], -500);  // R channel first sample
    }

    #[test]
    fn bytes_to_samples_full_blocks() {
        // 2 full stereo blocks
        assert_eq!(bytes_to_samples(STEREO_BLOCK_SIZE * 2), SAMPLES_PER_BLOCK * 2);
    }

    #[test]
    fn bytes_to_samples_partial_block_ignored() {
        // 1 full block + partial
        assert_eq!(bytes_to_samples(STEREO_BLOCK_SIZE + 10), SAMPLES_PER_BLOCK);
    }

    #[test]
    fn expand_nibble_positive_delta() {
        let mut hist: i32 = 0;
        let mut idx: i32 = 0;
        // nibble=7 (bits 0,1,2 set): delta = step>>3 + step>>2 + step>>1 + step
        // step_table[0]=7: delta = 0 + 1 + 3 + 7 = 11
        let s = expand_nibble(0x07, &mut hist, &mut idx);
        assert_eq!(s, 11);
        assert_eq!(hist, 11);
    }

    #[test]
    fn expand_nibble_negative_delta() {
        let mut hist: i32 = 100;
        let mut idx: i32 = 0;
        // nibble=0xF (all bits): delta = 0+1+3+7 = 11, negated = -11
        let s = expand_nibble(0x0F, &mut hist, &mut idx);
        assert_eq!(s, 89); // 100 - 11
    }

    #[test]
    fn step_index_clamps_to_valid_range() {
        let mut hist: i32 = 0;
        let mut idx: i32 = 0;
        // nibble=0 -> index_table[0] = -1, so idx becomes -1 -> clamped to 0
        expand_nibble(0x00, &mut hist, &mut idx);
        assert_eq!(idx, 0);

        // nibble=7 -> index_table[7] = 8, idx becomes 8
        idx = 85;
        expand_nibble(0x07, &mut hist, &mut idx);
        assert_eq!(idx, 88); // clamped to max
    }
}
