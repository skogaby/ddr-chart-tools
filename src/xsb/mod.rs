//! XSB (XACT2 Sound Bank) writer for DDR World.
//!
//! Generates a fully-formed sound bank from scratch. Two profiles are
//! supported, differing only in which sound entries they carry and which cues
//! address them:
//!
//! - [`write`] — the **song** profile: one wave bank, two simple cues (main +
//!   preview), a simple sound on mix category 4 and a complex sound on
//!   category 3, both carrying a runtime-parameter-curve reference. This is
//!   the shape every stock DDR song sound bank uses, and it is what the
//!   chart-conversion pipeline emits.
//! - [`write_se`] — the **sound-effect** profile: one wave bank, one simple
//!   cue, and one *bare* sound entry on mix category 6 with no
//!   runtime-parameter curve. This is the shape 129 of the 138 sounds in the
//!   game's own `se_normal.xsb` gameplay-SE bank use. Emitting the song
//!   profile for a sound effect would put it on the **music** mix bus and
//!   attach a curve referencing global audio state a caller never sets.
//!
//! Both are described in full at `docs/xsb_format.md`; only the song profile's
//! sound entries are covered there in detail.
//!
//! The binary layout, for the song profile:
//!
//! ```text
//! [Header]             0x00..0x4a  magic, versions, counts, section offsets
//! [Soundbank name]     0x4a..0x8a  64-byte null-padded ASCII
//! [Wavebank name]      0x8a..0xca  64-byte null-padded ASCII
//! [Sound entries]      0xca..+58   COMPLEX(39) + SIMPLE(19)
//! [Simple cues]        +10         2 × 5 bytes, point at sound entries
//! [Hash table]         +32         16 × u16, cue indices by hashed name
//! [Name index]         +12         2 × (u32 name_off, u16 next_in_chain)
//! [Cue name strings]   +N          "{code}_s\0{code}\0"
//! ```
//!
//! Total size: 326 bytes for a 4-char code (328 for 5 chars). The SE profile
//! is the same shape with a single 12-byte sound, one cue and one name: 262
//! bytes for a 4-char name.
//!
//! Every section offset in the header must equal the running byte cursor
//! *exactly*, and the cue-name string table must run exactly to the end of the
//! file — the engine's validator checks both — so the layout is computed from
//! the profile rather than assumed.
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

/// DDR never uses complex cues — the complex *sound* in the song profile is
/// still addressed by a simple cue.
const COMPLEX_CUE_COUNT: u16 = 0;
/// Hash bucket count. The format requires `max(16, simple_cues +
/// complex_cues)`; both profiles have well under 16 cues, so it is always the
/// floor of 16. Enforced below.
const HASH_BUCKET_COUNT: u16 = 16;
const WAVEBANK_COUNT: u8 = 1;

/// Per-sound entry sizes.
const SIMPLE_SOUND_SIZE: usize = 19;
const COMPLEX_SOUND_SIZE: usize = 39;
/// The bare SE sound entry: the 9-byte common prefix plus a `u16 wave_index`
/// and a `u8 wavebank_index`, with no trailing runtime-parameter-curve block.
const SE_SOUND_SIZE: usize = 12;
const CUE_ENTRY_SIZE: usize = 5;
const NAME_INDEX_ENTRY_SIZE: usize = 6;

/// Cue flags: bits 0 and 1 clear (no variation/transition table), bit 2 set
/// (playable sound cue). The engine's validator requires exactly this.
const CUE_FLAG_SOUND: u8 = 0x04;

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

/// Write a complete XSB sound bank for the given song `code` (song profile).
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
    let buf = build_xsb(&SONG_PROFILE, code_b);
    out.write_all(&buf)?;
    Ok(())
}

/// Write a complete XSB sound bank holding a single sound effect (SE profile).
///
/// `name` must be 1 to 16 ASCII alphanumeric characters and is written into
/// the soundbank name, the wavebank name, and as the one cue's name. The
/// engine matches a sound bank to its wave bank by name and resolves cues with
/// a byte-exact `strcmp`, so the companion XWB's internal bank name must be
/// `name` **including case**, and callers must play the cue under exactly that
/// name.
///
/// The single cue plays wave index **0** of wave bank 0 through a bare sound
/// entry on mix category 6 — the gameplay sound-effect bus — with no
/// runtime-parameter curve attached. Use this rather than [`write`] for
/// anything that is not song audio; see the module documentation for why.
pub fn write_se(name: &str, out: &mut impl Write) -> Result<(), XsbError> {
    let name_b = validate_code(name)?;
    let buf = build_xsb(&SE_PROFILE, name_b);
    out.write_all(&buf)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Profiles
// ---------------------------------------------------------------------------

/// One cue: which sound entry it plays, and the suffix appended to the bank
/// name to form the cue's own name.
struct CueSpec {
    sound: usize,
    name_suffix: &'static str,
}

/// Which sound entries a bank carries and which cues address them.
///
/// This is the *only* thing that differs between the two profiles. The header
/// constants, both 64-byte name fields, the hash table, the name index and the
/// CRC are common, so they are written once and driven from here.
struct Profile {
    /// Sound-entry byte templates, in the order they are laid out.
    sounds: &'static [&'static [u8]],
    /// Cues, in the order they are laid out.
    cues: &'static [CueSpec],
}

/// Song profile: complex preview sound first, then the simple main sound, with
/// cue 0 addressing the preview. See [`write_sounds`] for why that order.
const SONG_PROFILE: Profile = Profile {
    sounds: &[&COMPLEX_SOUND_BYTES, &SIMPLE_SOUND_BYTES],
    cues: &[
        CueSpec {
            sound: 0,
            name_suffix: "_s",
        },
        CueSpec {
            sound: 1,
            name_suffix: "",
        },
    ],
};

/// Sound-effect profile: one bare sound, one cue named after the bank itself.
const SE_PROFILE: Profile = Profile {
    sounds: &[&SE_SIMPLE_SOUND_BYTES],
    cues: &[CueSpec {
        sound: 0,
        name_suffix: "",
    }],
};

// The format requires `total_cues == max(16, cue_count)`, and this writer
// always emits the floor. Adding a profile with more cues than that would
// produce a bank the engine rejects, so fail the build instead.
const _: () = assert!(
    SONG_PROFILE.cues.len() <= HASH_BUCKET_COUNT as usize
        && SE_PROFILE.cues.len() <= HASH_BUCKET_COUNT as usize,
    "a profile has more cues than HASH_BUCKET_COUNT; total_cues must be max(16, cue_count)"
);

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Section byte offsets for a given profile and bank-name length.
struct Layout {
    soundbank_name: usize,
    wavebank_name: usize,
    sound: usize,
    simple_cue: usize,
    hash_table: usize,
    name_index: usize,
    cue_names: usize,
    total_size: usize,
    /// Length of the packed, NUL-terminated cue-name blob.
    cue_name_table_len: usize,
}

impl Layout {
    fn compute(profile: &Profile, name_len: usize) -> Self {
        let sound_block_len: usize = profile.sounds.iter().map(|s| s.len()).sum();
        // Each cue contributes its name plus a NUL terminator.
        let cue_name_table_len: usize = profile
            .cues
            .iter()
            .map(|c| name_len + c.name_suffix.len() + 1)
            .sum();

        let soundbank_name = HEADER_SIZE;
        let wavebank_name = soundbank_name + NAME_FIELD_LEN;
        let sound = wavebank_name + (WAVEBANK_COUNT as usize) * NAME_FIELD_LEN;
        let simple_cue = sound + sound_block_len;
        let hash_table = simple_cue + profile.cues.len() * CUE_ENTRY_SIZE;
        let name_index = hash_table + (HASH_BUCKET_COUNT as usize) * 2;
        let cue_names = name_index + profile.cues.len() * NAME_INDEX_ENTRY_SIZE;
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

/// File offset of sound entry `index`, i.e. the `sb_code` a cue stores to
/// reference it.
fn sound_offset(profile: &Profile, layout: &Layout, index: usize) -> usize {
    let preceding: usize = profile.sounds.iter().take(index).map(|s| s.len()).sum();
    layout.sound + preceding
}

fn build_xsb(profile: &Profile, name: &[u8]) -> Vec<u8> {
    let layout = Layout::compute(profile, name.len());
    let mut buf = vec![0u8; layout.total_size];

    write_header(&mut buf, profile, &layout);
    write_fixed_name(&mut buf, layout.soundbank_name, name);
    write_fixed_name(&mut buf, layout.wavebank_name, name);
    write_sounds(&mut buf, profile, &layout);
    write_cues(&mut buf, profile, &layout);
    write_hash_and_names(&mut buf, profile, &layout, name);
    write_crc(&mut buf);

    buf
}

fn write_header(buf: &mut [u8], profile: &Profile, layout: &Layout) {
    // The CRC at 0x08..0x0A is filled in last by `write_crc`. The 64-bit
    // timestamp at 0x0A..0x12 is left zero — the engine doesn't validate it.
    buf[0x00..0x04].copy_from_slice(&MAGIC.to_le_bytes());
    buf[0x04..0x06].copy_from_slice(&VERSION.to_le_bytes()); // content_version
    buf[0x06..0x08].copy_from_slice(&VERSION.to_le_bytes()); // tool_version
    buf[0x12] = PLATFORM;
    buf[0x13..0x15].copy_from_slice(&(profile.cues.len() as u16).to_le_bytes());
    buf[0x15..0x17].copy_from_slice(&COMPLEX_CUE_COUNT.to_le_bytes());
    // 0x17..0x19 unknown: must be 0 (already zeroed)
    buf[0x19..0x1B].copy_from_slice(&HASH_BUCKET_COUNT.to_le_bytes());
    buf[0x1B] = WAVEBANK_COUNT;
    buf[0x1C..0x1E].copy_from_slice(&(profile.sounds.len() as u16).to_le_bytes());
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

fn write_fixed_name(buf: &mut [u8], offset: usize, name: &[u8]) {
    buf[offset..offset + name.len()].copy_from_slice(name);
    // Trailing bytes of the 64-byte field are already zero from vec init.
}

// ---------------------------------------------------------------------------
// Sound entry byte sequences
// ---------------------------------------------------------------------------
//
// These are the fixed byte sequences written into the sound-entry slots. The
// song profile's two are specified in `docs/xsb_format.md` § "Sound entries";
// the SE profile's is not (that document covers song banks only), so its
// provenance is recorded on the constant itself. Fields haven't been
// parameterized because no two stock DDR XSBs actually vary them (apart from
// one byte in the preview track, noted below).

/// SIMPLE sound entry for a song's main track.
///
/// Layout: flags=0x04 (simple+rpc), category=4, volume=180, pitch=0,
/// priority=0, entry_length=19, wave_index=1, wavebank_index=0, then a
/// 7-byte RPC reference (len=7, count=1, code=0xF8).
const SIMPLE_SOUND_BYTES: [u8; SIMPLE_SOUND_SIZE] = [
    0x04, 0x04, 0x00, 0xB4, 0x00, 0x00, 0x00, 0x13, 0x00, 0x01, 0x00, 0x00, 0x07, 0x00, 0x01, 0xF8,
    0x00, 0x00, 0x00,
];

/// Bare SIMPLE sound entry for a sound effect.
///
/// Layout: flags=0x00 (simple, **no** RPC block), category=6, volume=254,
/// pitch=0, priority=0, entry_length=12, wave_index=0, wavebank_index=0.
///
/// The values follow the game's own gameplay-SE bank, `se_normal.xsb`, where
/// all 138 sounds are category 6 and 129 of them use exactly this bare 12-byte
/// form. Volume 254 is what the system bank's `SYS_COIN` and `X_sys_OK1` use —
/// a sound effect that has to be heard over the music wants the top of the
/// range, and the mix category still gates it behind the cabinet's SE volume.
#[rustfmt::skip]
const SE_SIMPLE_SOUND_BYTES: [u8; SE_SOUND_SIZE] = [
    0x00,        // flags: simple, no runtime-parameter curve
    0x06, 0x00,  // category = 6 (gameplay SE mix bus)
    0xFE,        // volume = 254
    0x00, 0x00,  // pitch = 0
    0x00,        // priority = 0
    0x0C, 0x00,  // entry_length = 12
    0x00, 0x00,  // wave_index = 0
    0x00,        // wavebank_index = 0
];

/// COMPLEX sound entry for a song's preview track with loop event.
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

/// Write the profile's sound entries, packed back to back.
///
/// Order matters for the song profile: the XACT2 engine in DDR World only
/// plays audio when the preview (complex) sound is laid out first and cue
/// index 0 points at it. Stock DDR XSBs are split roughly evenly between this
/// ordering and the inverse (simple-first, cue 0 = main — as seen in `fizz`,
/// `somd`, `vill`, `bknh2`), but in-game testing shows that only the
/// complex-first layout works. The other stock files may be accepted by virtue
/// of matching some other XACT state we can't observe from the file alone.
/// Rather than try to characterise that, we always emit the layout that is
/// empirically known to work.
fn write_sounds(buf: &mut [u8], profile: &Profile, layout: &Layout) {
    let mut off = layout.sound;
    for sound in profile.sounds {
        buf[off..off + sound.len()].copy_from_slice(sound);
        off += sound.len();
    }
}

/// Write the simple cue entries, each pointing at its profile's sound entry.
fn write_cues(buf: &mut [u8], profile: &Profile, layout: &Layout) {
    for (i, cue) in profile.cues.iter().enumerate() {
        let off = layout.simple_cue + i * CUE_ENTRY_SIZE;
        let sb_code = sound_offset(profile, layout, cue.sound) as u32;
        buf[off] = CUE_FLAG_SOUND;
        buf[off + 1..off + 5].copy_from_slice(&sb_code.to_le_bytes());
    }
}

/// Write the hash table, name index, and cue name strings.
///
/// Cue names are `{name}{suffix}`, packed NUL-terminated in cue order — so for
/// the song profile the table reads `"{code}_s\0{code}\0"`, matching the cue
/// order (cue 0 = preview → `{code}_s`, cue 1 = main → `{code}`).
fn write_hash_and_names(buf: &mut [u8], profile: &Profile, layout: &Layout, name: &[u8]) {
    // Cue name strings, remembering where each one landed.
    let mut cue_names: Vec<(usize, Vec<u8>)> = Vec::with_capacity(profile.cues.len());
    let mut off = layout.cue_names;
    for cue in profile.cues {
        let mut full = Vec::with_capacity(name.len() + cue.name_suffix.len());
        full.extend_from_slice(name);
        full.extend_from_slice(cue.name_suffix.as_bytes());
        buf[off..off + full.len()].copy_from_slice(&full);
        // The NUL terminator is already zero from the vec init.
        let start = off;
        off += full.len() + 1;
        cue_names.push((start, full));
    }

    // Initialize hash table to EMPTY_BUCKET.
    for b in 0..HASH_BUCKET_COUNT as usize {
        let bucket_off = layout.hash_table + b * 2;
        buf[bucket_off..bucket_off + 2].copy_from_slice(&EMPTY_BUCKET.to_le_bytes());
    }

    // Next-in-chain for each cue, resolved by insertion-order bucket fill.
    let mut next = vec![END_OF_CHAIN; profile.cues.len()];
    for (i, (_, full)) in cue_names.iter().enumerate() {
        let bucket = cue_name_hash_bucket(full, HASH_BUCKET_COUNT);
        insert_into_chain(buf, layout.hash_table, bucket, i as u16, &mut next);
    }

    // Name index entries: (u32 name_offset, u16 next_in_chain)
    for (i, (name_off, _)) in cue_names.iter().enumerate() {
        let entry = layout.name_index + i * NAME_INDEX_ENTRY_SIZE;
        buf[entry..entry + 4].copy_from_slice(&(*name_off as u32).to_le_bytes());
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
    next: &mut [u16],
) {
    let bucket_off = hash_table_off + bucket as usize * 2;
    let head = u16::from_le_bytes([buf[bucket_off], buf[bucket_off + 1]]);
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
    fn read_i16(buf: &[u8], offset: usize) -> i16 {
        i16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap())
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
        assert_eq!(read_u16(&buf, 0x13), 2, "simple_cue_count: main + preview");
        assert_eq!(read_u16(&buf, 0x15), COMPLEX_CUE_COUNT);
        assert_eq!(read_u16(&buf, 0x17), 0, "unknown@0x17 must be 0");
        assert_eq!(read_u16(&buf, 0x19), HASH_BUCKET_COUNT);
        assert_eq!(buf[0x1B], WAVEBANK_COUNT);
        assert_eq!(read_u16(&buf, 0x1C), 2, "sound_count: simple + complex");
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

    // ---- SE profile ------------------------------------------------------

    /// Where the SE profile's cue-name strings begin. Every section ahead of
    /// them is fixed-size (header 0x4A + two 64-byte name fields + one 12-byte
    /// sound + one 5-byte cue + 16 buckets + one 6-byte name-index entry), so
    /// this offset never moves.
    const SE_CUE_NAMES_OFFSET: usize = 0x101;

    fn write_se_to_vec(name: &str) -> Vec<u8> {
        let mut v = Vec::new();
        write_se(name, &mut v).unwrap();
        v
    }

    /// Total size of an SE bank for a name of `n` bytes: the fixed prefix plus
    /// the one NUL-terminated cue name.
    fn se_size(name_len: usize) -> usize {
        SE_CUE_NAMES_OFFSET + name_len + 1
    }

    #[test]
    fn se_profile_header_declares_one_cue_one_sound_one_wavebank() {
        let buf = write_se_to_vec("asti");

        assert_eq!(read_u32(&buf, 0x00), MAGIC);
        assert_eq!(read_u16(&buf, 0x04), VERSION, "content_version");
        assert_eq!(read_u16(&buf, 0x06), VERSION, "tool_version");
        assert_eq!(buf[0x12], PLATFORM, "flags byte: cue-name table present");
        assert_eq!(read_u16(&buf, 0x13), 1, "simple_cue_count");
        assert_eq!(read_u16(&buf, 0x15), 0, "complex_cue_count");
        assert_eq!(read_u16(&buf, 0x17), 0, "unknown@0x17 must be 0");
        assert_eq!(buf[0x1B], WAVEBANK_COUNT, "wavebank_count");
        assert_eq!(read_u16(&buf, 0x1C), 1, "sound_count");
        assert_eq!(read_u16(&buf, 0x20), 0, "unknown@0x20 must be 0");
        assert_eq!(read_i32(&buf, 0x26), NO_OFFSET, "complex_cue_off = -1");
        assert_eq!(read_i32(&buf, 0x2E), NO_OFFSET, "unknown@0x2e = -1");
        assert_eq!(read_i32(&buf, 0x32), NO_OFFSET, "variation_off = -1");
        assert_eq!(read_i32(&buf, 0x36), NO_OFFSET, "transition_off = -1");
        assert_eq!(buf.len(), se_size(4), "4-char name");
    }

    /// The engine's XSB validator requires each section offset to equal the
    /// running cursor *exactly*, and the cue-name string table to run exactly
    /// to EOF with a NUL as the final byte. Assert the computed layout rather
    /// than trusting it.
    #[test]
    fn se_profile_section_offsets_equal_the_running_cursor() {
        let buf = write_se_to_vec("asti");

        assert_eq!(read_i32(&buf, 0x3A), 0x8A, "wavebank_name_off");
        assert_eq!(read_i32(&buf, 0x46), 0xCA, "sound_off = 0x8A + 1*64");
        assert_eq!(read_i32(&buf, 0x22), 0xD6, "simple_cue_off = 0xCA + 12");
        assert_eq!(read_i32(&buf, 0x3E), 0xDB, "cue_hash_off = 0xD6 + 1*5");
        assert_eq!(read_i32(&buf, 0x42), 0xFB, "name_index_off = 0xDB + 16*2");
        assert_eq!(read_i32(&buf, 0x2A), 0x101, "cue_name_off = 0xFB + 1*6");

        let cue_names = read_i32(&buf, 0x2A) as usize;
        assert_eq!(
            read_u16(&buf, 0x1E) as usize,
            buf.len() - cue_names,
            "cue-name table must run exactly to EOF"
        );
        assert_eq!(buf[buf.len() - 1], 0, "last byte must be NUL");
    }

    #[test]
    fn se_profile_sound_entry_is_bare_category_six_at_wave_index_zero() {
        let buf = write_se_to_vec("asti");
        let sound = read_i32(&buf, 0x46) as usize;

        assert_eq!(buf[sound] & 0x01, 0, "bit 0 clear => simple, not complex");
        assert_eq!(buf[sound] & 0x04, 0, "bit 2 clear => no RPC curve");
        assert_eq!(buf[sound], 0x00, "the bare SE form's whole flags byte");
        // Category 6 is the gameplay-SE mix bus: all 138 sounds in the game's
        // own se_normal.xsb use it. Volume 254 matches the system bank's
        // SYS_COIN / X_sys_OK1.
        assert_eq!(read_u16(&buf, sound + 1), 6, "category");
        assert_eq!(buf[sound + 3], 0xFE, "volume");
        assert_eq!(read_i16(&buf, sound + 4), 0, "pitch");
        assert_eq!(buf[sound + 6], 0, "priority");
        assert_eq!(
            read_u16(&buf, sound + 7),
            SE_SOUND_SIZE as u16,
            "entry_length"
        );
        assert_eq!(read_u16(&buf, sound + 9), 0, "wave_index");
        assert_eq!(buf[sound + 11], 0, "wavebank_index");

        // No room for a trailing RPC block or a second sound: the whole sound
        // section is the 12-byte entry and the cue array starts immediately.
        let simple_cue = read_i32(&buf, 0x22) as usize;
        assert_eq!(simple_cue - sound, SE_SOUND_SIZE);
    }

    #[test]
    fn se_profile_still_clamps_the_bucket_count_to_sixteen() {
        let buf = write_se_to_vec("asti");
        assert_eq!(
            read_u16(&buf, 0x19),
            HASH_BUCKET_COUNT,
            "total_cues = max(16, simple + complex) = 16 even with one cue"
        );
        let hash = read_i32(&buf, 0x3E) as usize;
        let name_index = read_i32(&buf, 0x42) as usize;
        assert_eq!(name_index - hash, HASH_BUCKET_COUNT as usize * 2);
    }

    #[test]
    fn se_profile_writes_the_name_identically_in_both_name_fields() {
        let buf = write_se_to_vec("AsTi");

        assert_eq!(&buf[0x4A..0x4E], b"AsTi", "soundbank name, case preserved");
        assert_eq!(&buf[0x4E..0x8A], &[0u8; 0x3C], "null-padded to 64");
        assert_eq!(&buf[0x8A..0x8E], b"AsTi", "wavebank name, case preserved");
        assert_eq!(&buf[0x8E..0xCA], &[0u8; 0x3C], "null-padded to 64");

        // The engine matches a sound bank to its wave bank by name; the match
        // is byte-exact, so the two fields must be indistinguishable.
        assert_eq!(&buf[0x4A..0x8A], &buf[0x8A..0xCA]);
    }

    #[test]
    fn se_profile_cue_resolves_by_name_through_the_hash_table() {
        let buf = write_se_to_vec("asti");
        let hash = read_i32(&buf, 0x3E) as usize;
        let name_index = read_i32(&buf, 0x42) as usize;
        let cue_names = read_i32(&buf, 0x2A) as usize;

        let bucket = cue_name_hash_bucket(b"asti", HASH_BUCKET_COUNT) as usize;
        assert_eq!(
            read_u16(&buf, hash + bucket * 2),
            0,
            "the name's bucket must hold cue index 0"
        );
        for b in 0..HASH_BUCKET_COUNT as usize {
            if b != bucket {
                assert_eq!(
                    read_u16(&buf, hash + b * 2),
                    EMPTY_BUCKET,
                    "bucket {b} must be empty (validator: 0xFFFF or < cue count)"
                );
            }
        }

        assert_eq!(
            read_u32(&buf, name_index) as usize,
            cue_names,
            "name_offset"
        );
        assert_eq!(
            read_u16(&buf, name_index + 4),
            END_OF_CHAIN,
            "single cue => chain of length 1"
        );
        assert_eq!(&buf[cue_names..], b"asti\0", "the whole cue-name table");
    }

    #[test]
    fn se_profile_cue_points_at_the_sound_entry() {
        let buf = write_se_to_vec("asti");
        let sound = read_i32(&buf, 0x46) as u32;
        let cue = read_i32(&buf, 0x22) as usize;

        // Validator: bits 0 and 1 clear, bit 2 set.
        assert_eq!(buf[cue], 0x04, "playable sound cue");
        assert_eq!(read_u32(&buf, cue + 1), sound);
    }

    #[test]
    fn se_profile_crc_validates_when_recomputed_from_output() {
        let buf = write_se_to_vec("asti");
        let stored = read_u16(&buf, CRC_OFFSET);
        assert_ne!(stored, 0, "CRC must not be left as zero");
        assert_eq!(stored, xact_crc16(&buf[CRC_DATA_START..]));
    }

    #[test]
    fn se_profile_crc_changes_with_name() {
        let a = write_se_to_vec("aaaa");
        let b = write_se_to_vec("bbbb");
        assert_ne!(
            &a[CRC_OFFSET..CRC_OFFSET + 2],
            &b[CRC_OFFSET..CRC_OFFSET + 2]
        );
    }

    #[test]
    fn se_profile_accepts_boundary_name_lengths() {
        assert_eq!(write_se_to_vec("a").len(), se_size(1));
        let max = "a".repeat(MAX_CODE_LEN);
        assert_eq!(write_se_to_vec(&max).len(), se_size(MAX_CODE_LEN));
    }

    #[test]
    fn se_profile_rejects_invalid_names() {
        for bad in ["", "a-b!", "as ti"] {
            assert!(
                matches!(
                    write_se(bad, &mut Vec::new()),
                    Err(XsbError::BadCode { .. })
                ),
                "expected BadCode for {bad:?}"
            );
        }
        let long = "a".repeat(MAX_CODE_LEN + 1);
        assert!(matches!(
            write_se(&long, &mut Vec::new()),
            Err(XsbError::BadCode { .. })
        ));
    }

    // ---- Song-profile byte-identity regression ---------------------------

    /// Byte-for-byte output of `write("test")`, captured from the build that
    /// predates the SE profile being added.
    ///
    /// The SE profile shares this module's layout computation, sound-entry
    /// emitter, cue table, hash table and CRC step with the song profile. That
    /// sharing is what keeps the two readable, and it is also the one way the
    /// song profile could silently move by a byte — which would mute the audio
    /// of every song this tool has ever converted, with no error anywhere.
    ///
    /// So this fixture exists to *never* be regenerated. If it fails, the
    /// change that broke it is wrong, not the fixture.
    #[rustfmt::skip]
    const SONG_PROFILE_TEST_GOLDEN: [u8; 326] = [
        0x53, 0x44, 0x42, 0x4B, 0x2B, 0x00, 0x2B, 0x00, 0x9A, 0xD2, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x10, 0x00, 0x01, 0x02, 0x00, 0x0C, 0x00, 0x00, 0x00, 0x04, 0x01,
        0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0x3A, 0x01, 0x00, 0x00, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x8A, 0x00,
        0x00, 0x00, 0x0E, 0x01, 0x00, 0x00, 0x2E, 0x01, 0x00, 0x00, 0xCA, 0x00,
        0x00, 0x00, 0x74, 0x65, 0x73, 0x74, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x74, 0x65, 0x73, 0x74, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x03,
        0x00, 0xB4, 0x00, 0x00, 0x00, 0x27, 0x00, 0x01, 0x07, 0x00, 0x01, 0xF8,
        0x00, 0x00, 0x00, 0xB4, 0xE0, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00,
        0x20, 0x00, 0x00, 0xFF, 0x0C, 0x00, 0x00, 0x00, 0xFF, 0x00, 0x00, 0x00,
        0x00, 0x04, 0x04, 0x00, 0xB4, 0x00, 0x00, 0x00, 0x13, 0x00, 0x01, 0x00,
        0x00, 0x07, 0x00, 0x01, 0xF8, 0x00, 0x00, 0x00, 0x04, 0xCA, 0x00, 0x00,
        0x00, 0x04, 0xF1, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0x00, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xFF, 0x3A, 0x01, 0x00, 0x00, 0xFF, 0xFF, 0x41, 0x01, 0x00, 0x00,
        0xFF, 0xFF, 0x74, 0x65, 0x73, 0x74, 0x5F, 0x73, 0x00, 0x74, 0x65, 0x73,
        0x74, 0x00,
    ];

    #[test]
    fn song_profile_output_is_byte_identical_to_the_captured_golden() {
        let buf = write_to_vec("test");
        assert_eq!(
            buf.len(),
            SONG_PROFILE_TEST_GOLDEN.len(),
            "song-profile output length changed"
        );
        if let Some(i) = buf
            .iter()
            .zip(SONG_PROFILE_TEST_GOLDEN.iter())
            .position(|(a, b)| a != b)
        {
            panic!(
                "song-profile output changed at byte {i:#06x}: got {:#04x}, golden {:#04x}",
                buf[i], SONG_PROFILE_TEST_GOLDEN[i]
            );
        }
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
