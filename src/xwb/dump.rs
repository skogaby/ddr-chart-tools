//! Human- and script-readable dump of an XWB wave bank's metadata.
//!
//! Owns the *presentation* of a parsed bank's header, segment table and
//! per-entry metadata. It owns no parsing of its own beyond re-reading the two
//! things [`crate::xwb::container::parse`] discards — the five segment
//! descriptors and `dwEntryMetaDataElementSize` — because those are exactly
//! what a caller needs in order to check a bank against the engine's
//! container-validator rules.
//!
//! # Output format — treat this as an interface
//!
//! [`describe`] emits one `key=value` per line, newline-terminated, no blank
//! lines, in a fixed order. Keys are lowercase ASCII plus `.` and `_`, so a
//! shell script can rely on `grep '^bank.alignment=' | cut -d= -f2`.
//!
//! ```text
//! file.length=<bytes>                        total size of the file
//! bank.name=<string>                         szBankName up to the first NUL
//! bank.name_terminated=0|1                   byte 63 of the 64-byte name field is NUL
//! bank.version=<u32>                         dwVersion (always 43 here)
//! bank.header_version=<u32>                  dwHeaderVersion (must be 42)
//! bank.flags=0x%08X                          dwFlags
//! bank.type=buffer|streaming                 dwFlags bit 0
//! bank.has_entry_names=0|1                   dwFlags bit 16
//! bank.entry_count=<u32>
//! bank.entry_metadata_element_size=<u32>     must be 24 (non-compact)
//! bank.entry_name_element_size=<u32>         must be 64
//! bank.alignment=<u32>                       >= 4, or >= 2048 when streaming
//! bank.compact_format=<u32>
//! bank.build_time=<u64>                      never validated by the engine
//! segment<N>.offset=<u32>                    N = 0..4
//! segment<N>.length=<u32>
//! entry<N>.name=<string>
//! entry<N>.name_terminated=0|1               byte 63 of the entry's name field
//! entry<N>.codec=<u8>                        0=PCM 1=XMA 2=MS-ADPCM
//! entry<N>.channels=<u8>
//! entry<N>.sample_rate=<u32>
//! entry<N>.block_align_raw=<u8>              as stored in the format bitfield
//! entry<N>.block_align=<u32>                 derived: (raw + 22) * channels
//! entry<N>.samples_per_block=<u32>           derived
//! entry<N>.bits_per_sample_flag=0|1          PCM only: 0=8-bit 1=16-bit
//! entry<N>.entry_flags=<u32>                 low nibble of dwFlagsAndDuration
//! entry<N>.duration=<u32>                    dwFlagsAndDuration >> 4, in samples
//! entry<N>.data_offset=<u32>                 relative to segment 4
//! entry<N>.data_length=<u32>
//! entry<N>.loop_start=<u32>
//! entry<N>.loop_length=<u32>
//! ```
//!
//! `entry_flags` and `duration` are reported separately rather than as the
//! packed `dwFlagsAndDuration` word because the engine constrains them
//! independently: the flag nibble must be zero, and the duration must be at
//! least `loop_start + loop_length`.
//!
//! `bank.type` is the field to check first. The bank-type bit is a hard gate in
//! *both* directions — `CreateInMemoryWaveBank` requires it clear and the
//! file-backed path requires it set — so a bank of the wrong type is rejected
//! outright rather than merely misbehaving.

use std::fmt::Write as _;

use crate::xwb::container::{self, XwbBank, XwbError};

/// Header field offsets, per `docs`-level layout: 4-byte magic, `dwVersion`,
/// `dwHeaderVersion`, then five `{offset, length}` segment descriptors.
const VERSION_OFFSET: usize = 0x04;
const SEGMENT_TABLE_OFFSET: usize = 0x0C;
const SEGMENT_COUNT: usize = 5;
/// `dwEntryMetaDataElementSize` sits 0x48 into the BANKDATA segment.
const META_ELEMENT_SIZE_IN_BANKDATA: usize = 0x48;
/// Byte offset of `dwFlagsAndDuration` within a 24-byte entry-metadata record.
const ENTRY_DATA_OFFSET_IN_META: usize = 0x08;
const ENTRY_META_SIZE: usize = 24;
/// Both the bank-name field and each entry-name field are 64 bytes, and the
/// engine requires the last of those bytes to be NUL.
const NAME_FIELD_LEN: usize = 64;

/// Render an XWB wave bank's metadata as stable `key=value` text.
///
/// The bytes are parsed first, so a malformed bank yields the parser's own
/// error rather than a partial dump. See the module documentation for the
/// output format, which callers may depend on.
pub fn describe(bytes: &[u8]) -> Result<String, XwbError> {
    let bank = container::parse(bytes)?;
    let segments = read_segment_table(bytes)?;
    let version = read_u32(bytes, VERSION_OFFSET)?;
    let meta_element_size = read_u32(
        bytes,
        segments[0].0 as usize + META_ELEMENT_SIZE_IN_BANKDATA,
    )?;

    let mut out = String::new();
    write_bank(&mut out, bytes, &bank, version, meta_element_size);
    write_segments(&mut out, &segments);
    write_entries(&mut out, bytes, &bank, &segments)?;
    Ok(out)
}

fn write_bank(out: &mut String, bytes: &[u8], bank: &XwbBank, version: u32, meta_size: u32) {
    // `writeln!` into a String cannot fail, so the results are discarded
    // deliberately rather than propagated.
    let _ = writeln!(out, "file.length={}", bytes.len());
    let _ = writeln!(out, "bank.name={}", bank.name_str());
    let _ = writeln!(
        out,
        "bank.name_terminated={}",
        bool_bit(bank.name[NAME_FIELD_LEN - 1] == 0)
    );
    let _ = writeln!(out, "bank.version={version}");
    let _ = writeln!(out, "bank.header_version={}", bank.header_version);
    let _ = writeln!(out, "bank.flags=0x{:08X}", bank.flags);
    let _ = writeln!(
        out,
        "bank.type={}",
        if bank.flags & 1 == 0 {
            "buffer"
        } else {
            "streaming"
        }
    );
    let _ = writeln!(
        out,
        "bank.has_entry_names={}",
        bool_bit(bank.flags & (1 << 16) != 0)
    );
    let _ = writeln!(out, "bank.entry_count={}", bank.entries.len());
    let _ = writeln!(out, "bank.entry_metadata_element_size={meta_size}");
    let _ = writeln!(
        out,
        "bank.entry_name_element_size={}",
        bank.entry_name_element_size
    );
    let _ = writeln!(out, "bank.alignment={}", bank.alignment);
    let _ = writeln!(out, "bank.compact_format={}", bank.compact_format);
    let _ = writeln!(out, "bank.build_time={}", bank.build_time);
}

fn write_segments(out: &mut String, segments: &[(u32, u32); SEGMENT_COUNT]) {
    for (i, (offset, length)) in segments.iter().enumerate() {
        let _ = writeln!(out, "segment{i}.offset={offset}");
        let _ = writeln!(out, "segment{i}.length={length}");
    }
}

fn write_entries(
    out: &mut String,
    bytes: &[u8],
    bank: &XwbBank,
    segments: &[(u32, u32); SEGMENT_COUNT],
) -> Result<(), XwbError> {
    for (i, entry) in bank.entries.iter().enumerate() {
        let fmt = entry.format;
        // `data_offset` is the one per-entry field the parser resolves away
        // (it copies the data out), so read it back from segment 1.
        let data_offset = read_u32(
            bytes,
            segments[1].0 as usize + i * ENTRY_META_SIZE + ENTRY_DATA_OFFSET_IN_META,
        )?;
        let name_terminated = entry
            .name_bytes
            .get(NAME_FIELD_LEN - 1)
            .is_some_and(|&b| b == 0);

        let _ = writeln!(out, "entry{i}.name={}", entry.name_str());
        let _ = writeln!(
            out,
            "entry{i}.name_terminated={}",
            bool_bit(name_terminated)
        );
        let _ = writeln!(out, "entry{i}.codec={}", fmt.codec());
        let _ = writeln!(out, "entry{i}.channels={}", fmt.channels());
        let _ = writeln!(out, "entry{i}.sample_rate={}", fmt.sample_rate());
        let _ = writeln!(out, "entry{i}.block_align_raw={}", fmt.block_align_raw());
        let _ = writeln!(out, "entry{i}.block_align={}", fmt.block_align());
        let _ = writeln!(
            out,
            "entry{i}.samples_per_block={}",
            fmt.samples_per_block()
        );
        let _ = writeln!(
            out,
            "entry{i}.bits_per_sample_flag={}",
            fmt.bits_per_sample_flag()
        );
        let _ = writeln!(
            out,
            "entry{i}.entry_flags={}",
            entry.flags_and_duration & 0xF
        );
        let _ = writeln!(out, "entry{i}.duration={}", entry.flags_and_duration >> 4);
        let _ = writeln!(out, "entry{i}.data_offset={data_offset}");
        let _ = writeln!(out, "entry{i}.data_length={}", entry.data.len());
        let _ = writeln!(out, "entry{i}.loop_start={}", entry.loop_start);
        let _ = writeln!(out, "entry{i}.loop_length={}", entry.loop_length);
    }
    Ok(())
}

fn read_segment_table(bytes: &[u8]) -> Result<[(u32, u32); SEGMENT_COUNT], XwbError> {
    let mut segments = [(0u32, 0u32); SEGMENT_COUNT];
    for (i, slot) in segments.iter_mut().enumerate() {
        let base = SEGMENT_TABLE_OFFSET + i * 8;
        *slot = (read_u32(bytes, base)?, read_u32(bytes, base + 4)?);
    }
    Ok(segments)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, XwbError> {
    bytes
        .get(offset..offset + 4)
        .and_then(|s| s.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(XwbError::SegmentOutOfBounds {
            index: 0,
            offset,
            length: 4,
            file_len: bytes.len(),
        })
}

fn bool_bit(value: bool) -> u8 {
    u8::from(value)
}

#[cfg(test)]
mod tests {
    use crate::xwb::container::{self, WaveFormat, XwbBank, XwbEntry};
    use crate::xwb::dump::describe;
    use std::collections::HashMap;

    fn se_format() -> WaveFormat {
        WaveFormat::from_packed(2 | (1 << 2) | (44100 << 5) | (48 << 23))
    }

    fn name_field(s: &str) -> [u8; 64] {
        let mut f = [0u8; 64];
        f[..s.len()].copy_from_slice(s.as_bytes());
        f
    }

    fn entry(name: &str, data: Vec<u8>, total_samples: u32) -> XwbEntry {
        XwbEntry {
            flags_and_duration: total_samples << 4,
            format: se_format(),
            data,
            loop_start: 0,
            loop_length: total_samples,
            name_bytes: name.as_bytes().to_vec(),
        }
    }

    /// A buffer (in-memory) bank shaped like the generator's output.
    fn buffer_bank() -> Vec<u8> {
        let bank = XwbBank {
            header_version: 42,
            flags: 0x0009_0000,
            name: name_field("asti"),
            entry_name_element_size: 64,
            alignment: 4,
            compact_format: 0,
            build_time: 0,
            entries: vec![entry("asti", vec![0xAB; 70 * 3], 384)],
        };
        let mut out = Vec::new();
        container::write(&bank, &mut out).expect("write buffer bank");
        out
    }

    /// A streaming bank with two entries — deliberately *not* shaped like our
    /// own output, to prove the dump is general.
    fn streaming_bank() -> Vec<u8> {
        let bank = XwbBank {
            header_version: 42,
            flags: 0x0009_0001,
            name: name_field("aaaa"),
            entry_name_element_size: 64,
            alignment: 2048,
            compact_format: 0,
            build_time: 0x01CD_EFAD_3E0E_4F23,
            entries: vec![
                entry("aaaa", vec![0x11; 140 * 4], 512),
                entry("aaaa_s", vec![0x22; 140 * 2], 256),
            ],
        };
        let mut out = Vec::new();
        container::write(&bank, &mut out).expect("write streaming bank");
        out
    }

    fn parse_dump(text: &str) -> HashMap<String, String> {
        text.lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn dump_is_byte_stable_across_runs() -> Result<(), Box<dyn std::error::Error>> {
        let bytes = buffer_bank();
        assert_eq!(describe(&bytes)?, describe(&bytes)?);
        Ok(())
    }

    #[test]
    fn every_dump_line_is_a_key_value_pair() -> Result<(), Box<dyn std::error::Error>> {
        let text = describe(&buffer_bank())?;
        assert!(text.ends_with('\n'), "output must be newline-terminated");
        for line in text.lines() {
            assert!(!line.is_empty(), "no blank lines");
            let (k, _) = line.split_once('=').expect("every line is key=value");
            assert!(
                k.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_'),
                "key {k:?} must be greppable"
            );
        }
        Ok(())
    }

    #[test]
    fn dump_reports_the_documented_bank_and_segment_keys() -> Result<(), Box<dyn std::error::Error>>
    {
        let bytes = buffer_bank();
        let d = parse_dump(&describe(&bytes)?);

        assert_eq!(d["file.length"], bytes.len().to_string());
        assert_eq!(d["bank.name"], "asti");
        assert_eq!(d["bank.version"], "43");
        assert_eq!(d["bank.header_version"], "42");
        assert_eq!(d["bank.flags"], "0x00090000");
        assert_eq!(d["bank.type"], "buffer");
        assert_eq!(d["bank.has_entry_names"], "1");
        assert_eq!(d["bank.entry_count"], "1");
        assert_eq!(d["bank.entry_metadata_element_size"], "24");
        assert_eq!(d["bank.entry_name_element_size"], "64");
        assert_eq!(d["bank.alignment"], "4");
        assert_eq!(d["bank.compact_format"], "0");
        assert_eq!(d["bank.build_time"], "0");
        assert_eq!(d["bank.name_terminated"], "1");

        // Segment layout the engine's validator demands of a buffer bank.
        assert_eq!(d["segment0.offset"], "52");
        assert_eq!(d["segment0.length"], "96");
        assert_eq!(d["segment1.offset"], "148"); // 0x94
        assert_eq!(d["segment1.length"], "24");
        assert_eq!(d["segment2.length"], "0");
        assert_eq!(d["segment3.offset"], "172"); // 0x94 + 1*24
        assert_eq!(d["segment3.length"], "64");
        let seg4_off: usize = d["segment4.offset"].parse()?;
        let seg4_len: usize = d["segment4.length"].parse()?;
        assert_eq!(
            bytes.len() - seg4_off,
            seg4_len,
            "wave-data segment must run exactly to EOF"
        );
        Ok(())
    }

    #[test]
    fn dump_reports_the_documented_entry_keys() -> Result<(), Box<dyn std::error::Error>> {
        let d = parse_dump(&describe(&buffer_bank())?);

        assert_eq!(d["entry0.name"], "asti");
        assert_eq!(d["entry0.codec"], "2");
        assert_eq!(d["entry0.channels"], "1");
        assert_eq!(d["entry0.sample_rate"], "44100");
        assert_eq!(d["entry0.block_align_raw"], "48");
        assert_eq!(d["entry0.block_align"], "70");
        assert_eq!(d["entry0.samples_per_block"], "128");
        assert_eq!(d["entry0.bits_per_sample_flag"], "0");
        assert_eq!(d["entry0.entry_flags"], "0");
        assert_eq!(d["entry0.duration"], "384");
        assert_eq!(d["entry0.data_offset"], "0");
        assert_eq!(d["entry0.data_length"], "210");
        assert_eq!(d["entry0.loop_start"], "0");
        assert_eq!(d["entry0.loop_length"], "384");
        assert_eq!(d["entry0.name_terminated"], "1");
        Ok(())
    }

    #[test]
    fn dump_handles_a_foreign_streaming_bank() -> Result<(), Box<dyn std::error::Error>> {
        let d = parse_dump(&describe(&streaming_bank())?);

        assert_eq!(d["bank.type"], "streaming", "flags bit 0 set");
        assert_eq!(d["bank.flags"], "0x00090001");
        assert_eq!(d["bank.alignment"], "2048");
        assert_eq!(d["bank.entry_count"], "2");
        assert_eq!(d["bank.build_time"], 0x01CD_EFAD_3E0E_4F23_u64.to_string());
        assert_eq!(d["entry0.name"], "aaaa");
        assert_eq!(d["entry1.name"], "aaaa_s");
        // Streaming banks align each entry's data start to the bank alignment.
        assert_eq!(d["entry1.data_offset"], "2048");
        Ok(())
    }

    #[test]
    fn dump_rejects_a_non_bank() {
        assert!(describe(b"not a wave bank at all").is_err());
    }
}
