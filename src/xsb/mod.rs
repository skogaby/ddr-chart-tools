//! XSB (XACT2 Sound Bank) writer for DDR World.
//!
//! Generates a fully-formed sound bank from scratch for a given song code.
//! The output conforms to the DDR profile (one wave bank, two simple cues —
//! main + preview — two sounds, 16 hash buckets) described in full at
//! `docs/xsb_format.md`.
//!
//! The binary layout is:
//!
//! ```text
//! [Header]             0x00..0x4a  magic, versions, counts, section offsets
//! [Soundbank name]     0x4a..0x8a  64-byte null-padded ASCII
//! [Wavebank name]      0x8a..0xca  64-byte null-padded ASCII
//! [Sound entries]      0xca..+58   SIMPLE(19) + COMPLEX(39)
//! [Simple cues]        +10         2 × 5 bytes, point at sound entries
//! [Hash table]         +32         16 × u16, cue indices by hashed name
//! [Name index]         +12         2 × (u32 name_off, u16 next_in_chain)
//! [Cue name strings]   +N          "{code}\0{code}_s\0"
//! ```
//!
//! Total size: 326 bytes for a 4-char code (328 for 5 chars).
//!
//! The engine validates a CRC-16 over bytes `[0x12..]` stored at `0x08`; if
//! it doesn't match, the sound bank is silently rejected and audio goes
//! dark. The CRC and the cue-name hash function were reverse-engineered
//! from `xactengine2_10.dll`; see `docs/xsb_format.md` for references.

use std::io::Write;

use thiserror::Error;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// File magic: "SDBK" (little-endian).
const MAGIC: u32 = 0x4B42_4453;
/// XSB content/tool version for XACT2 v2.10 (DDR World).
const VERSION: u16 = 0x002B;
/// Windows platform byte.
const PLATFORM: u8 = 0x01;

/// Fixed header size — the section offsets embed byte offsets starting after
/// this region.
const HEADER_SIZE: usize = 0x4A;
/// Fixed-width soundbank and wave bank name fields.
const NAME_FIELD_LEN: usize = 0x40;

/// DDR profile uses exactly these counts.
const SIMPLE_CUE_COUNT: u16 = 2;
const COMPLEX_CUE_COUNT: u16 = 0;
/// Hash bucket count — always `max(16, simple_cues + complex_cues)`.
const HASH_BUCKET_COUNT: u16 = 16;
const WAVEBANK_COUNT: u8 = 1;
const SOUND_COUNT: u16 = 2;

/// Per-sound entry sizes (fixed shape for the DDR profile).
const SIMPLE_SOUND_SIZE: usize = 19;
const COMPLEX_SOUND_SIZE: usize = 39;
const CUE_ENTRY_SIZE: usize = 5;
const NAME_INDEX_ENTRY_SIZE: usize = 6;

/// Sentinels.
const EMPTY_BUCKET: u16 = 0xFFFF;
const END_OF_CHAIN: u16 = 0xFFFF;
const NO_OFFSET: i32 = -1;

/// Maximum supported song-code length. The name fields are 64 bytes so the
/// hard limit is 63, but the DDR audio-file naming convention caps codes at
/// ~5 characters in practice. We accept up to 16 defensively.
const MAX_CODE_LEN: usize = 16;

/// Byte offset of the CRC field in the header.
const CRC_OFFSET: usize = 0x08;
/// First byte covered by the CRC (everything after CRC + timestamp).
const CRC_DATA_START: usize = 0x12;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum XsbError {
    #[error("song code must be 1-{max} ASCII alphanumeric characters, got {code:?}", max = MAX_CODE_LEN)]
    BadCode { code: String },

    #[error("write error: {0}")]
    Write(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Write a complete XSB sound bank for the given song `code`.
///
/// `code` must be 1 to 16 ASCII alphanumeric characters. It is written into
/// the soundbank name, wavebank name, and as the main cue name; the preview
/// cue is named `{code}_s`.
///
/// The resulting file contains a CRC-16 validating its own contents, a hash
/// table for O(1) cue lookup by name, and references to the companion XWB's
/// wave entries (main at index 1, preview at index 0, in one wave bank).
pub fn write(code: &str, out: &mut impl Write) -> Result<(), XsbError> {
    let code_b = validate_code(code)?;
    let buf = build_xsb(code_b);
    out.write_all(&buf)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Section byte offsets for a given code length.
struct Layout {
    soundbank_name: usize,
    wavebank_name: usize,
    sound: usize,
    simple_cue: usize,
    hash_table: usize,
    name_index: usize,
    cue_names: usize,
    total_size: usize,
    /// Length of the `{code}\0{code}_s\0` blob.
    cue_name_table_len: usize,
}

impl Layout {
    fn compute(code_len: usize) -> Self {
        let cue_name_table_len = code_len + 1 + code_len + 2 + 1; // code + \0 + code + "_s" + \0

        let soundbank_name = HEADER_SIZE;
        let wavebank_name = soundbank_name + NAME_FIELD_LEN;
        let sound = wavebank_name + NAME_FIELD_LEN;
        let simple_cue = sound + SIMPLE_SOUND_SIZE + COMPLEX_SOUND_SIZE;
        let hash_table = simple_cue + (SIMPLE_CUE_COUNT as usize) * CUE_ENTRY_SIZE;
        let name_index = hash_table + (HASH_BUCKET_COUNT as usize) * 2;
        let cue_names = name_index + (SIMPLE_CUE_COUNT as usize) * NAME_INDEX_ENTRY_SIZE;
        let total_size = cue_names + cue_name_table_len;

        Self {
            soundbank_name,
            wavebank_name,
            sound,
            simple_cue,
            hash_table,
            name_index,
            cue_names,
            total_size,
            cue_name_table_len,
        }
    }
}

fn build_xsb(code: &[u8]) -> Vec<u8> {
    let layout = Layout::compute(code.len());
    let mut buf = vec![0u8; layout.total_size];

    write_header(&mut buf, &layout);
    write_fixed_name(&mut buf, layout.soundbank_name, code);
    write_fixed_name(&mut buf, layout.wavebank_name, code);
    write_sounds(&mut buf, layout.sound);
    write_cues(&mut buf, &layout);
    write_hash_and_names(&mut buf, &layout, code);
    write_crc(&mut buf);

    buf
}

fn write_header(buf: &mut [u8], layout: &Layout) {
    // The CRC at 0x08..0x0A is filled in last by `write_crc`. The 64-bit
    // timestamp at 0x0A..0x12 is left zero — the engine doesn't validate it.
    buf[0x00..0x04].copy_from_slice(&MAGIC.to_le_bytes());
    buf[0x04..0x06].copy_from_slice(&VERSION.to_le_bytes()); // content_version
    buf[0x06..0x08].copy_from_slice(&VERSION.to_le_bytes()); // tool_version
    buf[0x12] = PLATFORM;
    buf[0x13..0x15].copy_from_slice(&SIMPLE_CUE_COUNT.to_le_bytes());
    buf[0x15..0x17].copy_from_slice(&COMPLEX_CUE_COUNT.to_le_bytes());
    // 0x17..0x19 unknown: must be 0 (already zeroed)
    buf[0x19..0x1B].copy_from_slice(&HASH_BUCKET_COUNT.to_le_bytes());
    buf[0x1B] = WAVEBANK_COUNT;
    buf[0x1C..0x1E].copy_from_slice(&SOUND_COUNT.to_le_bytes());
    buf[0x1E..0x20].copy_from_slice(&(layout.cue_name_table_len as u16).to_le_bytes());
    // 0x20..0x22 unknown: must be 0 (already zeroed)

    put_i32(buf, 0x22, layout.simple_cue as i32);
    put_i32(buf, 0x26, NO_OFFSET); // complex_cue_off
    put_i32(buf, 0x2A, layout.cue_names as i32);
    put_i32(buf, 0x2E, NO_OFFSET); // unknown, must be -1
    put_i32(buf, 0x32, NO_OFFSET); // variation_off
    put_i32(buf, 0x36, NO_OFFSET); // transition_off
    put_i32(buf, 0x3A, layout.wavebank_name as i32);
    put_i32(buf, 0x3E, layout.hash_table as i32);
    put_i32(buf, 0x42, layout.name_index as i32);
    put_i32(buf, 0x46, layout.sound as i32);
}

fn write_fixed_name(buf: &mut [u8], offset: usize, code: &[u8]) {
    buf[offset..offset + code.len()].copy_from_slice(code);
    // Trailing bytes of the 64-byte field are already zero from vec init.
}

// ---------------------------------------------------------------------------
// Sound entry byte sequences (DDR profile)
// ---------------------------------------------------------------------------
//
// These are the fixed byte sequences written into the two sound-entry slots.
// The layout is hardcoded from the DDR profile described in
// `docs/xsb_format.md` § "Sound entries" — fields haven't been parameterized
// because no two stock DDR XSBs actually vary them (apart from one byte in
// the preview track, noted below).

/// SIMPLE sound entry for the main track.
///
/// Layout: flags=0x04 (simple+rpc), category=4, volume=180, pitch=0,
/// priority=0, entry_length=19, wave_index=1, wavebank_index=0, then a
/// 7-byte RPC reference (len=7, count=1, code=0xF8).
const SIMPLE_SOUND_BYTES: [u8; SIMPLE_SOUND_SIZE] = [
    0x04, 0x04, 0x00, 0xB4, 0x00, 0x00, 0x00, 0x13, 0x00, 0x01, 0x00, 0x00, 0x07, 0x00, 0x01, 0xF8,
    0x00, 0x00, 0x00,
];

/// COMPLEX sound entry for the preview track with loop event.
///
/// Layout: flags=0x05 (complex+rpc), category=3, volume=180, pitch=0,
/// priority=0, entry_length=39, track_count=1, followed by the 29-byte
/// preview track template. The track template is byte-identical across all
/// 12 stock DDR XSBs except for byte at offset 8 of the track body, which
/// varies as 0xE0 / 0xE5 / 0xF3 — purpose unconfirmed (see
/// `docs/xsb_format.md` § "Known Unknowns"). We use the majority 0xE0.
const COMPLEX_SOUND_BYTES: [u8; COMPLEX_SOUND_SIZE] = [
    // sound prefix (10 bytes): flags, cat, vol, pitch, prio, entry_len, track_count
    0x05, 0x03, 0x00, 0xB4, 0x00, 0x00, 0x00, 0x27, 0x00, 0x01,
    // track RPC preamble (7 bytes)
    0x07, 0x00, 0x01, 0xF8, 0x00, 0x00, 0x00,
    // track body (22 bytes): track volume, mystery byte (majority 0xE0),
    // then the fixed loop-event / clip encoding.
    0xB4, 0xE0, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x20, 0x00, 0x00, 0xFF, 0x0C, 0x00, 0x00,
    0x00, 0xFF, 0x00, 0x00, 0x00, 0x00,
];

/// Write the two sound entries: COMPLEX preview first, then SIMPLE main.
///
/// The order matters: the XACT2 engine in DDR World only plays audio when
/// the preview (complex) sound is laid out first and cue index 0 points at
/// it. Stock DDR XSBs are split roughly evenly between this ordering and
/// the inverse (simple-first, cue 0 = main — as seen in `fizz`, `somd`,
/// `vill`, `bknh2`), but in-game testing shows that only the complex-first
/// layout works. The other stock files may be accepted by virtue of
/// matching some other XACT state we can't observe from the file alone.
/// Rather than try to characterise that, we always emit the layout that is
/// empirically known to work.
fn write_sounds(buf: &mut [u8], sound_off: usize) {
    buf[sound_off..sound_off + COMPLEX_SOUND_SIZE].copy_from_slice(&COMPLEX_SOUND_BYTES);
    let main_off = sound_off + COMPLEX_SOUND_SIZE;
    buf[main_off..main_off + SIMPLE_SOUND_SIZE].copy_from_slice(&SIMPLE_SOUND_BYTES);
}

/// Write the two simple cue entries pointing at the two sound entries.
/// Cue 0 = preview (COMPLEX, first in the sound block), cue 1 = main (SIMPLE).
fn write_cues(buf: &mut [u8], layout: &Layout) {
    let preview_sound_off = layout.sound as u32;
    let main_sound_off = (layout.sound + COMPLEX_SOUND_SIZE) as u32;

    let cue0 = layout.simple_cue;
    buf[cue0] = 0x04; // flags: playable sound cue
    buf[cue0 + 1..cue0 + 5].copy_from_slice(&preview_sound_off.to_le_bytes());

    let cue1 = cue0 + CUE_ENTRY_SIZE;
    buf[cue1] = 0x04;
    buf[cue1 + 1..cue1 + 5].copy_from_slice(&main_sound_off.to_le_bytes());
}

/// Write the hash table, name index, and cue name strings.
///
/// Cue names are laid out as `{code}_s\0{code}\0` — matching the cue order
/// (cue 0 = preview → `{code}_s`, cue 1 = main → `{code}`).
fn write_hash_and_names(buf: &mut [u8], layout: &Layout, code: &[u8]) {
    // Cue 0 name = "{code}_s", cue 1 name = "{code}"
    let cue0_name_off = layout.cue_names; // "{code}_s"
    let cue1_name_off = cue0_name_off + code.len() + 2 + 1; // skip code+"_s"+\0

    buf[cue0_name_off..cue0_name_off + code.len()].copy_from_slice(code);
    buf[cue0_name_off + code.len()..cue0_name_off + code.len() + 2].copy_from_slice(b"_s");
    // Separator \0 is already zero-init.
    buf[cue1_name_off..cue1_name_off + code.len()].copy_from_slice(code);
    // Trailing \0 is zero-init.

    // Compute buckets for both names and insert with chaining.
    //   Cue 0 = preview = "{code}_s"
    //   Cue 1 = main    = "{code}"
    let mut name_s = Vec::with_capacity(code.len() + 2);
    name_s.extend_from_slice(code);
    name_s.extend_from_slice(b"_s");
    let bucket0 = cue_name_hash_bucket(&name_s, HASH_BUCKET_COUNT);
    let bucket1 = cue_name_hash_bucket(code, HASH_BUCKET_COUNT);

    // Initialize hash table to EMPTY_BUCKET.
    for b in 0..HASH_BUCKET_COUNT as usize {
        let off = layout.hash_table + b * 2;
        buf[off..off + 2].copy_from_slice(&EMPTY_BUCKET.to_le_bytes());
    }

    // Next-in-chain for each cue, resolved by insertion-order bucket fill.
    let mut next = [END_OF_CHAIN, END_OF_CHAIN];
    insert_into_chain(buf, layout.hash_table, bucket0, 0, &mut next);
    insert_into_chain(buf, layout.hash_table, bucket1, 1, &mut next);

    // Name index entries: (u32 name_offset, u16 next_in_chain)
    let name_offs = [cue0_name_off as u32, cue1_name_off as u32];
    for i in 0..SIMPLE_CUE_COUNT as usize {
        let entry = layout.name_index + i * NAME_INDEX_ENTRY_SIZE;
        buf[entry..entry + 4].copy_from_slice(&name_offs[i].to_le_bytes());
        buf[entry + 4..entry + 6].copy_from_slice(&next[i].to_le_bytes());
    }
}

/// Insert `cue_index` into the hash table's chain at `bucket`, setting
/// `next[...]` entries as needed.
fn insert_into_chain(
    buf: &mut [u8],
    hash_table_off: usize,
    bucket: u16,
    cue_index: u16,
    next: &mut [u16; 2],
) {
    let bucket_off = hash_table_off + bucket as usize * 2;
    let head = u16::from_le_bytes(buf[bucket_off..bucket_off + 2].try_into().unwrap());
    if head == EMPTY_BUCKET {
        buf[bucket_off..bucket_off + 2].copy_from_slice(&cue_index.to_le_bytes());
    } else {
        // Walk chain to its tail and append.
        let mut tail = head as usize;
        while next[tail] != END_OF_CHAIN {
            tail = next[tail] as usize;
        }
        next[tail] = cue_index;
    }
}

/// Compute the CRC-16 over bytes `[0x12..]` and store it at offset `0x08`.
fn write_crc(buf: &mut [u8]) {
    let crc = xact_crc16(&buf[CRC_DATA_START..]);
    buf[CRC_OFFSET..CRC_OFFSET + 2].copy_from_slice(&crc.to_le_bytes());
}

// ---------------------------------------------------------------------------
// Hash and CRC (from xactengine2_10.dll)
// ---------------------------------------------------------------------------

/// XACT2 cue-name hash bucket.
///
/// Matches `xactengine2_10.dll` function `FUN_0040fad0` called from
/// `GetCueIndex`. Per character: `h = 3*h + (h >> 1) + c`, all wrapping u16.
/// Bucket = hash % bucket_count (unsigned — the DLL uses signed IDIV but for
/// ASCII-derived u16 values the result is identical).
///
/// See `docs/xsb_format.md` § "Cue Name Hash" for the derivation.
fn cue_name_hash_bucket(name: &[u8], bucket_count: u16) -> u16 {
    let mut h: u16 = 0;
    for &c in name {
        h = h
            .wrapping_mul(3)
            .wrapping_add(h >> 1)
            .wrapping_add(c as u16);
    }
    h % bucket_count
}

/// CRC-16 used by the XACT2 engine to validate XSB contents.
///
/// Extracted from `xactengine2_10.dll` function `FUN_00424200`. The engine
/// stores `!crc` at offset 0x08 and rejects the bank silently if it fails.
fn xact_crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc = CRC_TABLE[((b as u16) ^ crc) as usize & 0xFF] ^ (crc >> 8);
    }
    !crc
}

#[rustfmt::skip]
const CRC_TABLE: [u16; 256] = [
    0x0000, 0x1189, 0x2312, 0x329b, 0x4624, 0x57ad, 0x6536, 0x74bf,
    0x8c48, 0x9dc1, 0xaf5a, 0xbed3, 0xca6c, 0xdbe5, 0xe97e, 0xf8f7,
    0x1081, 0x0108, 0x3393, 0x221a, 0x56a5, 0x472c, 0x75b7, 0x643e,
    0x9cc9, 0x8d40, 0xbfdb, 0xae52, 0xdaed, 0xcb64, 0xf9ff, 0xe876,
    0x2102, 0x308b, 0x0210, 0x1399, 0x6726, 0x76af, 0x4434, 0x55bd,
    0xad4a, 0xbcc3, 0x8e58, 0x9fd1, 0xeb6e, 0xfae7, 0xc87c, 0xd9f5,
    0x3183, 0x200a, 0x1291, 0x0318, 0x77a7, 0x662e, 0x54b5, 0x453c,
    0xbdcb, 0xac42, 0x9ed9, 0x8f50, 0xfbef, 0xea66, 0xd8fd, 0xc974,
    0x4204, 0x538d, 0x6116, 0x709f, 0x0420, 0x15a9, 0x2732, 0x36bb,
    0xce4c, 0xdfc5, 0xed5e, 0xfcd7, 0x8868, 0x99e1, 0xab7a, 0xbaf3,
    0x5285, 0x430c, 0x7197, 0x601e, 0x14a1, 0x0528, 0x37b3, 0x263a,
    0xdecd, 0xcf44, 0xfddf, 0xec56, 0x98e9, 0x8960, 0xbbfb, 0xaa72,
    0x6306, 0x728f, 0x4014, 0x519d, 0x2522, 0x34ab, 0x0630, 0x17b9,
    0xef4e, 0xfec7, 0xcc5c, 0xddd5, 0xa96a, 0xb8e3, 0x8a78, 0x9bf1,
    0x7387, 0x620e, 0x5095, 0x411c, 0x35a3, 0x242a, 0x16b1, 0x0738,
    0xffcf, 0xee46, 0xdcdd, 0xcd54, 0xb9eb, 0xa862, 0x9af9, 0x8b70,
    0x8408, 0x9581, 0xa71a, 0xb693, 0xc22c, 0xd3a5, 0xe13e, 0xf0b7,
    0x0840, 0x19c9, 0x2b52, 0x3adb, 0x4e64, 0x5fed, 0x6d76, 0x7cff,
    0x9489, 0x8500, 0xb79b, 0xa612, 0xd2ad, 0xc324, 0xf1bf, 0xe036,
    0x18c1, 0x0948, 0x3bd3, 0x2a5a, 0x5ee5, 0x4f6c, 0x7df7, 0x6c7e,
    0xa50a, 0xb483, 0x8618, 0x9791, 0xe32e, 0xf2a7, 0xc03c, 0xd1b5,
    0x2942, 0x38cb, 0x0a50, 0x1bd9, 0x6f66, 0x7eef, 0x4c74, 0x5dfd,
    0xb58b, 0xa402, 0x9699, 0x8710, 0xf3af, 0xe226, 0xd0bd, 0xc134,
    0x39c3, 0x284a, 0x1ad1, 0x0b58, 0x7fe7, 0x6e6e, 0x5cf5, 0x4d7c,
    0xc60c, 0xd785, 0xe51e, 0xf497, 0x8028, 0x91a1, 0xa33a, 0xb2b3,
    0x4a44, 0x5bcd, 0x6956, 0x78df, 0x0c60, 0x1de9, 0x2f72, 0x3efb,
    0xd68d, 0xc704, 0xf59f, 0xe416, 0x90a9, 0x8120, 0xb3bb, 0xa232,
    0x5ac5, 0x4b4c, 0x79d7, 0x685e, 0x1ce1, 0x0d68, 0x3ff3, 0x2e7a,
    0xe70e, 0xf687, 0xc41c, 0xd595, 0xa12a, 0xb0a3, 0x8238, 0x93b1,
    0x6b46, 0x7acf, 0x4854, 0x59dd, 0x2d62, 0x3ceb, 0x0e70, 0x1ff9,
    0xf78f, 0xe606, 0xd49d, 0xc514, 0xb1ab, 0xa022, 0x92b9, 0x8330,
    0x7bc7, 0x6a4e, 0x58d5, 0x495c, 0x3de3, 0x2c6a, 0x1ef1, 0x0f78,
];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn validate_code(code: &str) -> Result<&[u8], XsbError> {
    let bytes = code.as_bytes();
    if bytes.is_empty()
        || bytes.len() > MAX_CODE_LEN
        || !bytes.iter().all(|b| b.is_ascii_alphanumeric())
    {
        return Err(XsbError::BadCode {
            code: code.to_string(),
        });
    }
    Ok(bytes)
}

fn put_i32(buf: &mut [u8], offset: usize, value: i32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Read a little-endian value from the generated XSB.
    fn read_u16(buf: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap())
    }
    fn read_u32(buf: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap())
    }
    fn read_i32(buf: &[u8], offset: usize) -> i32 {
        i32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap())
    }

    fn write_to_vec(code: &str) -> Vec<u8> {
        let mut v = Vec::new();
        write(code, &mut v).unwrap();
        v
    }

    // ---- Hash function verification --------------------------------------

    #[test]
    fn hash_is_deterministic_and_length_sensitive() {
        assert_ne!(
            cue_name_hash_bucket(b"test", 16),
            cue_name_hash_bucket(b"test_s", 16),
            "a name and its _s variant must generally map to different buckets"
        );
        assert_eq!(
            cue_name_hash_bucket(b"abcd", 16),
            cue_name_hash_bucket(b"abcd", 16)
        );
    }

    // ---- Output layout ---------------------------------------------------

    #[test]
    fn output_size_matches_spec() {
        assert_eq!(write_to_vec("test").len(), 326, "4-char code");
        assert_eq!(write_to_vec("t").len(), 320, "1-char code (326 - 6)");
        assert_eq!(write_to_vec("abcde").len(), 328, "5-char code");
    }

    #[test]
    fn header_constants_match_spec() {
        let buf = write_to_vec("test");
        assert_eq!(read_u32(&buf, 0x00), MAGIC);
        assert_eq!(read_u16(&buf, 0x04), VERSION);
        assert_eq!(read_u16(&buf, 0x06), VERSION);
        assert_eq!(buf[0x12], PLATFORM);
        assert_eq!(read_u16(&buf, 0x13), SIMPLE_CUE_COUNT);
        assert_eq!(read_u16(&buf, 0x15), COMPLEX_CUE_COUNT);
        assert_eq!(read_u16(&buf, 0x17), 0, "unknown@0x17 must be 0");
        assert_eq!(read_u16(&buf, 0x19), HASH_BUCKET_COUNT);
        assert_eq!(buf[0x1B], WAVEBANK_COUNT);
        assert_eq!(read_u16(&buf, 0x1C), SOUND_COUNT);
        assert_eq!(read_u16(&buf, 0x20), 0, "unknown@0x20 must be 0");
        assert_eq!(read_i32(&buf, 0x26), NO_OFFSET, "complex_cue_off = -1");
        assert_eq!(read_i32(&buf, 0x2E), NO_OFFSET, "unknown@0x2e = -1");
        assert_eq!(read_i32(&buf, 0x32), NO_OFFSET, "variation_off = -1");
        assert_eq!(read_i32(&buf, 0x36), NO_OFFSET, "transition_off = -1");
    }

    #[test]
    fn section_offsets_are_consistent_and_ordered() {
        let buf = write_to_vec("test");
        let sound = read_i32(&buf, 0x46);
        let simple_cue = read_i32(&buf, 0x22);
        let cue_hash = read_i32(&buf, 0x3E);
        let name_idx = read_i32(&buf, 0x42);
        let cue_names = read_i32(&buf, 0x2A);
        let wavebank_name = read_i32(&buf, 0x3A);

        assert_eq!(wavebank_name, 0x8A);
        assert_eq!(sound, 0xCA);
        assert!(sound < simple_cue);
        assert!(simple_cue < cue_hash);
        assert!(cue_hash < name_idx);
        assert!(name_idx < cue_names);
        assert!((cue_names as usize) < buf.len());
    }

    #[test]
    fn soundbank_and_wavebank_names_carry_code() {
        let buf = write_to_vec("test");
        assert_eq!(&buf[0x4A..0x4E], b"test");
        assert_eq!(&buf[0x4E..0x8A], &[0u8; 0x3C]); // null-padded
        assert_eq!(&buf[0x8A..0x8E], b"test");
        assert_eq!(&buf[0x8E..0xCA], &[0u8; 0x3C]);
    }

    #[test]
    fn sound_entries_have_expected_flags_and_categories() {
        let buf = write_to_vec("test");
        let sound = read_i32(&buf, 0x46) as usize;

        // COMPLEX preview is first (sound block starts with the preview sound).
        assert_eq!(buf[sound], 0x05, "complex sound flags");
        assert_eq!(read_u16(&buf, sound + 1), 3, "preview category");
        assert_eq!(read_u16(&buf, sound + 7), COMPLEX_SOUND_SIZE as u16);

        // SIMPLE main follows immediately after.
        let simple = sound + COMPLEX_SOUND_SIZE;
        assert_eq!(buf[simple], 0x04, "simple sound flags");
        assert_eq!(read_u16(&buf, simple + 1), 4, "main category");
        assert_eq!(read_u16(&buf, simple + 7), SIMPLE_SOUND_SIZE as u16);
    }

    #[test]
    fn cues_point_at_sound_entries() {
        let buf = write_to_vec("test");
        let sound = read_i32(&buf, 0x46) as u32;
        let simple_cue = read_i32(&buf, 0x22) as usize;

        assert_eq!(buf[simple_cue], 0x04);
        assert_eq!(
            read_u32(&buf, simple_cue + 1),
            sound,
            "cue 0 -> preview sound (first in sound block)"
        );

        let cue1 = simple_cue + CUE_ENTRY_SIZE;
        assert_eq!(buf[cue1], 0x04);
        assert_eq!(
            read_u32(&buf, cue1 + 1),
            sound + COMPLEX_SOUND_SIZE as u32,
            "cue 1 -> main sound"
        );
    }

    #[test]
    fn cue_name_strings_are_null_terminated_pair() {
        let buf = write_to_vec("test");
        let cue_names = read_i32(&buf, 0x2A) as usize;
        let len = read_u16(&buf, 0x1E) as usize;
        // Cue 0 (preview, "{code}_s") is listed first in the name table.
        assert_eq!(&buf[cue_names..cue_names + len], b"test_s\0test\0");
    }

    #[test]
    fn hash_table_populates_buckets_for_both_cue_names() {
        let buf = write_to_vec("test");
        let hash_off = read_i32(&buf, 0x3E) as usize;

        let bucket_main = cue_name_hash_bucket(b"test", 16) as usize;
        let bucket_prev = cue_name_hash_bucket(b"test_s", 16) as usize;

        let val_main = read_u16(&buf, hash_off + bucket_main * 2);
        let val_prev = read_u16(&buf, hash_off + bucket_prev * 2);

        // Each cue name's bucket must hold a valid cue index (0 or 1), or
        // (for collision) one of them may point into a chain.
        assert_ne!(val_main, EMPTY_BUCKET, "main bucket must not be empty");
        assert_ne!(val_prev, EMPTY_BUCKET, "preview bucket must not be empty");

        // All 16 buckets together must hold exactly 2 non-empty entries
        // when the two names don't collide, or 1 entry (head of chain) when
        // they do. Either way, the sum of cue indices reachable from the
        // table must be {0, 1}.
        let mut reached = Vec::new();
        let name_idx_off = read_i32(&buf, 0x42) as usize;
        for b in 0..16 {
            let head = read_u16(&buf, hash_off + b * 2);
            let mut cur = head;
            while cur != EMPTY_BUCKET {
                reached.push(cur);
                let entry = name_idx_off + cur as usize * NAME_INDEX_ENTRY_SIZE;
                cur = read_u16(&buf, entry + 4); // next_in_chain
            }
        }
        reached.sort();
        assert_eq!(reached, vec![0, 1], "both cue indices must be reachable");
    }

    #[test]
    fn name_index_points_at_the_correct_strings() {
        let buf = write_to_vec("test");
        let idx_off = read_i32(&buf, 0x42) as usize;
        let cue_names = read_i32(&buf, 0x2A) as u32;

        // Cue 0 name ("test_s") is at the start of the cue-name blob; cue 1
        // ("test") follows after "test_s\0" (7 bytes).
        let name0_off = read_u32(&buf, idx_off);
        let name1_off = read_u32(&buf, idx_off + NAME_INDEX_ENTRY_SIZE);

        assert_eq!(name0_off, cue_names);
        assert_eq!(name1_off, cue_names + 7); // "test_s\0"

        // Read the null-terminated strings at those offsets
        let read_cstr = |o: u32| -> &[u8] {
            let start = o as usize;
            let end = start + buf[start..].iter().position(|&b| b == 0).unwrap();
            &buf[start..end]
        };
        assert_eq!(read_cstr(name0_off), b"test_s");
        assert_eq!(read_cstr(name1_off), b"test");
    }

    // ---- CRC -------------------------------------------------------------

    #[test]
    fn crc_is_computed_and_nonzero() {
        let buf = write_to_vec("test");
        let crc = read_u16(&buf, CRC_OFFSET);
        assert_ne!(crc, 0, "CRC must not be left as zero");
    }

    #[test]
    fn crc_validates_when_recomputed_from_output() {
        let buf = write_to_vec("test");
        let stored = read_u16(&buf, CRC_OFFSET);
        let recomputed = xact_crc16(&buf[CRC_DATA_START..]);
        assert_eq!(stored, recomputed, "output CRC must self-validate");
    }

    #[test]
    fn crc_changes_with_code() {
        let a = write_to_vec("aaaa");
        let b = write_to_vec("bbbb");
        assert_ne!(
            &a[CRC_OFFSET..CRC_OFFSET + 2],
            &b[CRC_OFFSET..CRC_OFFSET + 2],
            "different codes must produce different CRCs"
        );
    }

    // ---- Input validation ------------------------------------------------

    #[test]
    fn rejects_empty_code() {
        let mut out = Vec::new();
        assert!(matches!(write("", &mut out), Err(XsbError::BadCode { .. })));
    }

    #[test]
    fn rejects_too_long_code() {
        let mut out = Vec::new();
        let long = "a".repeat(MAX_CODE_LEN + 1);
        assert!(matches!(
            write(&long, &mut out),
            Err(XsbError::BadCode { .. })
        ));
    }

    #[test]
    fn rejects_non_alphanumeric() {
        let mut out = Vec::new();
        assert!(matches!(
            write("a-b!", &mut out),
            Err(XsbError::BadCode { .. })
        ));
    }

    #[test]
    fn accepts_boundary_lengths() {
        assert!(write(&"a".repeat(MAX_CODE_LEN), &mut Vec::new()).is_ok());
        assert!(write("a", &mut Vec::new()).is_ok());
    }
}
