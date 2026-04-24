//! XWB v43 container structures, parser, and writer.
//!
//! The file layout is:
//!
//! ```text
//! [Header 52 bytes]  magic "WBND" + version + 5 segment descriptors
//! [Segment 0]        Bank data: flags, entry count, name, format info
//! [Segment 1]        Entry metadata: 24 bytes per entry
//! [Segment 2]        Seek tables (empty for ADPCM)
//! [Segment 3]        Entry names: fixed-width null-padded ASCII
//! [padding]          Zero-fill to alignment boundary
//! [Segment 4]        Wave data: raw codec bytes
//! ```

use std::io::Write;

use thiserror::Error;

use crate::util::io::{IoError, LeReader};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAGIC: &[u8; 4] = b"WBND";
const VERSION: u32 = 43;
const HEADER_SIZE: usize = 52;
const SEGMENT_COUNT: usize = 5;
const BANK_NAME_LEN: usize = 64;
const ENTRY_META_SIZE: u32 = 24;
/// Bank data is 96 bytes in XACT2 header_version 42 (DDR's format):
/// flags(4) + count(4) + name(64) + meta_size(4) + name_size(4) +
/// align(4) + compact(4) + build_time(8).
const BANK_DATA_SIZE: usize = 96;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum XwbError {
    #[error("invalid magic at byte 0: expected WBND")]
    BadMagic,

    #[error("unsupported version {version} (expected {VERSION})")]
    UnsupportedVersion { version: u32 },

    #[error("segment {index} extends beyond file (offset {offset} + length {length} > file size {file_len})")]
    SegmentOutOfBounds {
        index: usize,
        offset: usize,
        length: usize,
        file_len: usize,
    },

    #[error("entry {index} data extends beyond wave-data segment")]
    EntryDataOutOfBounds { index: usize },

    #[error("bank contains no entries")]
    NoEntries,

    #[error("ADPCM decode error: {0}")]
    Adpcm(#[from] crate::xwb::adpcm::AdpcmError),

    #[error("read error: {0}")]
    Io(#[from] IoError),

    #[error("write error: {0}")]
    Write(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// WAVEBANKMINIWAVEFORMAT — packed 32-bit format descriptor
// ---------------------------------------------------------------------------

/// Packed 32-bit audio format descriptor.
///
/// Bit layout (LSB first):
/// - `[0:1]`   codec (2 bits): 0=PCM, 1=XMA, 2=ADPCM, 3=WMA
/// - `[2:4]`   channels (3 bits)
/// - `[5:22]`  sample rate (18 bits)
/// - `[23:30]` block align raw (8 bits)
/// - `[31]`    bits-per-sample flag (1 bit): 0=8-bit, 1=16-bit
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaveFormat(u32);

impl WaveFormat {
    pub const CODEC_ADPCM: u8 = 2;

    #[must_use]
    pub fn from_packed(bits: u32) -> Self {
        Self(bits)
    }

    #[must_use]
    pub fn packed(self) -> u32 {
        self.0
    }

    #[must_use]
    pub fn codec(self) -> u8 {
        (self.0 & 0x3) as u8
    }

    #[must_use]
    pub fn channels(self) -> u8 {
        ((self.0 >> 2) & 0x7) as u8
    }

    #[must_use]
    pub fn sample_rate(self) -> u32 {
        (self.0 >> 5) & 0x3_FFFF
    }

    #[must_use]
    pub fn block_align_raw(self) -> u8 {
        ((self.0 >> 23) & 0xFF) as u8
    }

    #[must_use]
    pub fn bits_per_sample_flag(self) -> u8 {
        ((self.0 >> 31) & 0x1) as u8
    }

    /// Actual block alignment in bytes for ADPCM:
    /// `(raw + 22) * channels`.
    #[must_use]
    pub fn block_align(self) -> u32 {
        (self.block_align_raw() as u32 + 22) * self.channels() as u32
    }

    /// Decoded samples per ADPCM block:
    /// `((block_align - 7 * channels) * 8) / (4 * channels) + 2`.
    #[must_use]
    pub fn samples_per_block(self) -> u32 {
        let ba = self.block_align();
        let ch = self.channels() as u32;
        if ch == 0 || ba < 7 * ch {
            return 0;
        }
        ((ba - 7 * ch) * 8) / (4 * ch) + 2
    }
}

// ---------------------------------------------------------------------------
// Bank and entry types
// ---------------------------------------------------------------------------

/// A parsed XWB wave bank.
#[derive(Debug, Clone)]
pub struct XwbBank {
    pub header_version: u32,
    pub flags: u32,
    /// Raw 64-byte bank name field (null-padded ASCII).
    pub name: [u8; BANK_NAME_LEN],
    pub entry_name_element_size: u32,
    pub alignment: u32,
    pub compact_format: u32,
    /// 64-bit build timestamp (XACT2 header_version 42 uses 8 bytes).
    pub build_time: u64,
    pub entries: Vec<XwbEntry>,
}

impl XwbBank {
    /// Bank name as a UTF-8 string (trimmed at first null).
    #[must_use]
    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(BANK_NAME_LEN);
        std::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
}

/// One wave entry in the bank.
#[derive(Debug, Clone)]
pub struct XwbEntry {
    pub flags_and_duration: u32,
    pub format: WaveFormat,
    /// Raw codec bytes (e.g. MS-ADPCM blocks).
    pub data: Vec<u8>,
    pub loop_start: u32,
    pub loop_length: u32,
    /// Raw entry name bytes (fixed-width, null-padded).
    pub name_bytes: Vec<u8>,
}

impl XwbEntry {
    /// Entry name as a UTF-8 string (trimmed at first null).
    #[must_use]
    pub fn name_str(&self) -> &str {
        let end = self.name_bytes.iter().position(|&b| b == 0).unwrap_or(self.name_bytes.len());
        std::str::from_utf8(&self.name_bytes[..end]).unwrap_or("")
    }
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

/// Parse an XWB v43 file from raw bytes.
pub fn parse(bytes: &[u8]) -> Result<XwbBank, XwbError> {
    let mut r = LeReader::new(bytes);

    // -- Header (52 bytes) --
    let magic = r.read_bytes(4)?;
    if magic != MAGIC {
        return Err(XwbError::BadMagic);
    }
    let version = r.read_u32()?;
    if version != VERSION {
        return Err(XwbError::UnsupportedVersion { version });
    }
    let header_version = r.read_u32()?;

    let mut seg_offset = [0u32; SEGMENT_COUNT];
    let mut seg_length = [0u32; SEGMENT_COUNT];
    for i in 0..SEGMENT_COUNT {
        seg_offset[i] = r.read_u32()?;
        seg_length[i] = r.read_u32()?;
    }

    // Validate segment bounds.
    for i in 0..SEGMENT_COUNT {
        let o = seg_offset[i] as usize;
        let l = seg_length[i] as usize;
        if o.saturating_add(l) > bytes.len() {
            return Err(XwbError::SegmentOutOfBounds {
                index: i,
                offset: o,
                length: l,
                file_len: bytes.len(),
            });
        }
    }

    // -- Segment 0: bank data --
    let seg0 = &bytes[seg_offset[0] as usize..][..seg_length[0] as usize];
    let mut br = LeReader::new(seg0);
    let flags = br.read_u32()?;
    let entry_count = br.read_u32()?;
    let mut name = [0u8; BANK_NAME_LEN];
    name.copy_from_slice(br.read_bytes(BANK_NAME_LEN)?);
    let _entry_metadata_element_size = br.read_u32()?;
    let entry_name_element_size = br.read_u32()?;
    let alignment = br.read_u32()?;
    let compact_format = br.read_u32()?;
    let build_time_lo = br.read_u32()?;
    let build_time_hi = br.read_u32()?;
    let build_time = (build_time_hi as u64) << 32 | build_time_lo as u64;

    // -- Segment 1: entry metadata --
    let seg1 = &bytes[seg_offset[1] as usize..][..seg_length[1] as usize];

    // -- Segment 3: entry names --
    let seg3 = &bytes[seg_offset[3] as usize..][..seg_length[3] as usize];

    // -- Segment 4: wave data --
    let seg4 = &bytes[seg_offset[4] as usize..][..seg_length[4] as usize];

    // -- Parse entries --
    let mut entries = Vec::with_capacity(entry_count as usize);
    for i in 0..entry_count as usize {
        let meta_start = i * ENTRY_META_SIZE as usize;
        let meta_end = meta_start + ENTRY_META_SIZE as usize;
        if meta_end > seg1.len() {
            return Err(XwbError::EntryDataOutOfBounds { index: i });
        }
        let mut er = LeReader::new(&seg1[meta_start..meta_end]);
        let flags_and_duration = er.read_u32()?;
        let format = WaveFormat::from_packed(er.read_u32()?);
        let data_offset = er.read_u32()? as usize;
        let data_length = er.read_u32()? as usize;
        let loop_start = er.read_u32()?;
        let loop_length = er.read_u32()?;

        if data_offset.saturating_add(data_length) > seg4.len() {
            return Err(XwbError::EntryDataOutOfBounds { index: i });
        }
        let data = seg4[data_offset..data_offset + data_length].to_vec();

        // Entry name.
        let name_size = entry_name_element_size as usize;
        let name_bytes = if name_size > 0 {
            let ns = i * name_size;
            let ne = ns + name_size;
            if ne <= seg3.len() {
                seg3[ns..ne].to_vec()
            } else {
                vec![0u8; name_size]
            }
        } else {
            Vec::new()
        };

        entries.push(XwbEntry {
            flags_and_duration,
            format,
            data,
            loop_start,
            loop_length,
            name_bytes,
        });
    }

    Ok(XwbBank {
        header_version,
        flags,
        name,
        entry_name_element_size,
        alignment,
        compact_format,
        build_time,
        entries,
    })
}

// ---------------------------------------------------------------------------
// Write
// ---------------------------------------------------------------------------

/// Write an XWB v43 file.
///
/// Segment layout: header → seg0 → seg1 → seg2(empty) → seg3 → pad → seg4.
pub fn write(bank: &XwbBank, out: &mut impl Write) -> Result<(), XwbError> {
    let entry_count = bank.entries.len() as u32;
    let name_elem_size = bank.entry_name_element_size;
    let align = if bank.alignment == 0 { 1 } else { bank.alignment } as usize;

    // Compute segment sizes.
    let seg0_len = BANK_DATA_SIZE;
    let seg1_len = entry_count as usize * ENTRY_META_SIZE as usize;
    let seg2_len: usize = 0;
    let seg3_len = entry_count as usize * name_elem_size as usize;

    // Compute segment offsets.
    let seg0_off = HEADER_SIZE;
    let seg1_off = seg0_off + seg0_len;
    let seg2_off = seg1_off + seg1_len;
    let seg3_off = seg2_off + seg2_len;
    let seg4_off = round_up(seg3_off + seg3_len, align);
    // Compute aligned data offsets within the wave-data segment.
    // Streaming wave banks align each entry's data start to the bank
    // alignment boundary (typically 2048) for sector-aligned reads.
    let mut data_offsets: Vec<usize> = Vec::with_capacity(bank.entries.len());
    let mut data_cursor: usize = 0;
    for entry in &bank.entries {
        data_offsets.push(data_cursor);
        data_cursor = round_up(data_cursor + entry.data.len(), align);
    }
    // Total wave-data segment size: last entry's offset + its data length
    // (no trailing padding needed after the final entry).
    let seg4_len: usize = if let Some(last) = bank.entries.last() {
        data_offsets.last().unwrap_or(&0) + last.data.len()
    } else {
        0
    };

    // -- Header --
    out.write_all(MAGIC)?;
    out.write_all(&VERSION.to_le_bytes())?;
    out.write_all(&bank.header_version.to_le_bytes())?;
    for (off, len) in [
        (seg0_off, seg0_len),
        (seg1_off, seg1_len),
        (seg2_off, seg2_len),
        (seg3_off, seg3_len),
        (seg4_off, seg4_len),
    ] {
        out.write_all(&(off as u32).to_le_bytes())?;
        out.write_all(&(len as u32).to_le_bytes())?;
    }

    // -- Segment 0: bank data --
    out.write_all(&bank.flags.to_le_bytes())?;
    out.write_all(&entry_count.to_le_bytes())?;
    out.write_all(&bank.name)?;
    out.write_all(&ENTRY_META_SIZE.to_le_bytes())?;
    out.write_all(&name_elem_size.to_le_bytes())?;
    out.write_all(&bank.alignment.to_le_bytes())?;
    out.write_all(&bank.compact_format.to_le_bytes())?;
    out.write_all(&bank.build_time.to_le_bytes())?;

    // -- Segment 1: entry metadata --
    for (entry, &doff) in bank.entries.iter().zip(&data_offsets) {
        out.write_all(&entry.flags_and_duration.to_le_bytes())?;
        out.write_all(&entry.format.packed().to_le_bytes())?;
        out.write_all(&(doff as u32).to_le_bytes())?;
        out.write_all(&(entry.data.len() as u32).to_le_bytes())?;
        out.write_all(&entry.loop_start.to_le_bytes())?;
        out.write_all(&entry.loop_length.to_le_bytes())?;
    }

    // -- Segment 2: seek tables (empty) --

    // -- Segment 3: entry names --
    for entry in &bank.entries {
        let ns = name_elem_size as usize;
        if entry.name_bytes.len() >= ns {
            out.write_all(&entry.name_bytes[..ns])?;
        } else {
            out.write_all(&entry.name_bytes)?;
            let pad = ns - entry.name_bytes.len();
            out.write_all(&vec![0u8; pad])?;
        }
    }

    // -- Padding to alignment boundary --
    let written_so_far = seg3_off + seg3_len;
    let pad_len = seg4_off - written_so_far;
    if pad_len > 0 {
        out.write_all(&vec![0u8; pad_len])?;
    }

    // -- Segment 4: wave data (with inter-entry alignment padding) --
    let mut wave_cursor: usize = 0;
    for (entry, &doff) in bank.entries.iter().zip(&data_offsets) {
        if doff > wave_cursor {
            out.write_all(&vec![0u8; doff - wave_cursor])?;
        }
        out.write_all(&entry.data)?;
        wave_cursor = doff + entry.data.len();
    }

    Ok(())
}

fn round_up(value: usize, alignment: usize) -> usize {
    if alignment <= 1 {
        return value;
    }
    value.div_ceil(alignment) * alignment
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid XWB v43 file with the given entries.
    fn build_xwb(
        bank_name: &str,
        entry_name_size: u32,
        alignment: u32,
        entries: &[(&str, &[u8], WaveFormat)],
    ) -> Vec<u8> {
        let mut name_field = [0u8; BANK_NAME_LEN];
        for (i, &b) in bank_name.as_bytes().iter().enumerate().take(BANK_NAME_LEN) {
            name_field[i] = b;
        }

        let entry_count = entries.len();
        let seg0_len = BANK_DATA_SIZE;
        let seg1_len = entry_count * ENTRY_META_SIZE as usize;
        let seg3_len = entry_count * entry_name_size as usize;
        let seg0_off = HEADER_SIZE;
        let seg1_off = seg0_off + seg0_len;
        let seg2_off = seg1_off + seg1_len;
        let seg3_off = seg2_off; // seg2 is empty
        let align = if alignment == 0 { 1 } else { alignment } as usize;
        let seg4_off = round_up(seg3_off + seg3_len, align);

        // Compute aligned data offsets within wave-data segment.
        let mut data_offsets = Vec::with_capacity(entry_count);
        let mut cursor: usize = 0;
        for (_, data, _) in entries {
            data_offsets.push(cursor);
            cursor = round_up(cursor + data.len(), align);
        }
        let total_data: usize = if let Some((_, last, _)) = entries.last() {
            data_offsets.last().unwrap_or(&0) + last.len()
        } else {
            0
        };

        let mut buf = Vec::with_capacity(seg4_off + total_data);

        // Header
        buf.extend_from_slice(MAGIC);
        buf.extend_from_slice(&VERSION.to_le_bytes());
        buf.extend_from_slice(&42u32.to_le_bytes()); // header_version
        for (off, len) in [
            (seg0_off, seg0_len),
            (seg1_off, seg1_len),
            (seg2_off, 0usize),
            (seg3_off, seg3_len),
            (seg4_off, total_data),
        ] {
            buf.extend_from_slice(&(off as u32).to_le_bytes());
            buf.extend_from_slice(&(len as u32).to_le_bytes());
        }

        // Seg 0: bank data
        buf.extend_from_slice(&0u32.to_le_bytes()); // flags
        buf.extend_from_slice(&(entry_count as u32).to_le_bytes());
        buf.extend_from_slice(&name_field);
        buf.extend_from_slice(&ENTRY_META_SIZE.to_le_bytes());
        buf.extend_from_slice(&entry_name_size.to_le_bytes());
        buf.extend_from_slice(&alignment.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes()); // compact_format
        buf.extend_from_slice(&0u64.to_le_bytes()); // build_time

        // Seg 1: entry metadata
        for (i, (_, data, fmt)) in entries.iter().enumerate() {
            buf.extend_from_slice(&0u32.to_le_bytes()); // flags_and_duration
            buf.extend_from_slice(&fmt.packed().to_le_bytes());
            buf.extend_from_slice(&(data_offsets[i] as u32).to_le_bytes());
            buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
            buf.extend_from_slice(&0u32.to_le_bytes()); // loop_start
            buf.extend_from_slice(&0u32.to_le_bytes()); // loop_length
        }

        // Seg 3: entry names
        for (ename, _, _) in entries {
            let mut nb = vec![0u8; entry_name_size as usize];
            for (i, &b) in ename.as_bytes().iter().enumerate().take(nb.len()) {
                nb[i] = b;
            }
            buf.extend_from_slice(&nb);
        }

        // Padding to seg4
        buf.resize(seg4_off, 0);

        // Seg 4: wave data (with inter-entry alignment padding)
        for (i, (_, data, _)) in entries.iter().enumerate() {
            buf.resize(seg4_off + data_offsets[i], 0);
            buf.extend_from_slice(data);
        }

        buf
    }

    /// DDR-style WAVEBANKMINIWAVEFORMAT: ADPCM, 2ch, 44100Hz, raw=48.
    fn ddr_format() -> WaveFormat {
        // codec=2, channels=2, sample_rate=44100, block_align_raw=48, bps_flag=0
        let bits: u32 = 2 | (2 << 2) | (44100 << 5) | (48 << 23);
        WaveFormat::from_packed(bits)
    }

    #[test]
    fn wave_format_round_trips() {
        let fmt = ddr_format();
        assert_eq!(fmt.codec(), 2);
        assert_eq!(fmt.channels(), 2);
        assert_eq!(fmt.sample_rate(), 44100);
        assert_eq!(fmt.block_align_raw(), 48);
        assert_eq!(fmt.block_align(), (48 + 22) * 2); // 140
        assert_eq!(fmt.samples_per_block(), 128);
        assert_eq!(WaveFormat::from_packed(fmt.packed()), fmt);
    }

    #[test]
    fn parse_write_round_trip_single_entry() {
        let audio = vec![0xABu8; 280]; // 2 ADPCM blocks
        let raw = build_xwb("test", 64, 2048, &[("main", &audio, ddr_format())]);

        let bank = parse(&raw).unwrap();
        assert_eq!(bank.entries.len(), 1);
        assert_eq!(bank.entries[0].name_str(), "main");
        assert_eq!(bank.entries[0].data, audio);
        assert_eq!(bank.entries[0].format, ddr_format());

        let mut out = Vec::new();
        write(&bank, &mut out).unwrap();
        assert_eq!(out, raw, "round-trip must be byte-identical");
    }

    #[test]
    fn parse_write_round_trip_two_entries() {
        let main_audio = vec![0x11u8; 1400]; // 10 blocks
        let preview_audio = vec![0x22u8; 420]; // 3 blocks
        let raw = build_xwb(
            "fizz",
            64,
            2048,
            &[
                ("fizz", &main_audio, ddr_format()),
                ("fizz_s", &preview_audio, ddr_format()),
            ],
        );

        let bank = parse(&raw).unwrap();
        assert_eq!(bank.name_str(), "fizz");
        assert_eq!(bank.entries.len(), 2);
        assert_eq!(bank.entries[0].name_str(), "fizz");
        assert_eq!(bank.entries[0].data, main_audio);
        assert_eq!(bank.entries[1].name_str(), "fizz_s");
        assert_eq!(bank.entries[1].data, preview_audio);

        let mut out = Vec::new();
        write(&bank, &mut out).unwrap();
        assert_eq!(out, raw, "round-trip must be byte-identical");
    }

    #[test]
    fn parse_rejects_bad_magic() {
        let mut raw = build_xwb("x", 64, 2048, &[("a", &[0u8; 140], ddr_format())]);
        raw[0] = b'X'; // corrupt magic
        assert!(matches!(parse(&raw), Err(XwbError::BadMagic)));
    }

    #[test]
    fn parse_rejects_wrong_version() {
        let mut raw = build_xwb("x", 64, 2048, &[("a", &[0u8; 140], ddr_format())]);
        raw[4..8].copy_from_slice(&42u32.to_le_bytes());
        assert!(matches!(
            parse(&raw),
            Err(XwbError::UnsupportedVersion { version: 42 })
        ));
    }

    #[test]
    fn parse_rejects_truncated_header() {
        let raw = b"WBND";
        assert!(parse(raw).is_err());
    }

    #[test]
    fn wave_format_zero_channels_samples_per_block_is_zero() {
        let fmt = WaveFormat::from_packed(0);
        assert_eq!(fmt.samples_per_block(), 0);
    }

    #[test]
    fn round_up_works() {
        assert_eq!(round_up(0, 2048), 0);
        assert_eq!(round_up(1, 2048), 2048);
        assert_eq!(round_up(2048, 2048), 2048);
        assert_eq!(round_up(2049, 2048), 4096);
        assert_eq!(round_up(100, 1), 100);
    }
}
