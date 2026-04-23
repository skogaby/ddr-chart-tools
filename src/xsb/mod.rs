//! XSB (XACT Sound Bank) template-based writer.
//!
//! DDR World's XSBs are structurally uniform for 4-char song codes.
//! Rather than implementing a full XSB compiler, this module patches a
//! static template (extracted from `fizz.xsb`) with the song's 4-char
//! code at the known variable offsets.
//!
//! The template has these variable regions zeroed:
//! - `0x08..0x10` — 8-byte timestamp (always written as zeros)
//! - `0x4a..0x4e` — cue name (4 bytes, null-padded)
//! - `0x8a..0x8e` — wave bank name (4 bytes)
//! - `0x13a..0x13e` — name-table entry 1 (main)
//! - `0x13f..0x143` — name-table entry 2 (preview prefix)

use std::io::Write;

use thiserror::Error;

/// The 326-byte XSB template with name fields zeroed.
const TEMPLATE: &[u8; 326] = include_bytes!("template.bin");

/// Offsets of the 4-byte name fields to patch.
const NAME_OFFSETS: [usize; 4] = [0x4a, 0x8a, 0x13a, 0x13f];

#[derive(Debug, Error)]
pub enum XsbError {
    #[error("song code must be 1-4 ASCII alphanumeric characters, got {code:?}")]
    BadCode { code: String },

    #[error("write error: {0}")]
    Write(#[from] std::io::Error),
}

/// Write an XSB for the given 4-char song code.
///
/// `code` must be 1–4 ASCII alphanumeric characters. Shorter codes are
/// null-padded to 4 bytes.
pub fn write(code: &str, out: &mut impl Write) -> Result<(), XsbError> {
    let bytes = code.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 4
        || !bytes.iter().all(|b| b.is_ascii_alphanumeric())
    {
        return Err(XsbError::BadCode {
            code: code.to_string(),
        });
    }

    let mut buf = *TEMPLATE;

    // Patch each name field with the code, null-padded to 4 bytes.
    let mut name = [0u8; 4];
    name[..bytes.len()].copy_from_slice(bytes);

    for &off in &NAME_OFFSETS {
        buf[off..off + 4].copy_from_slice(&name);
    }

    out.write_all(&buf)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_is_326_bytes() {
        assert_eq!(TEMPLATE.len(), 326);
    }

    #[test]
    fn template_name_regions_are_zeroed() {
        for &off in &NAME_OFFSETS {
            assert_eq!(&TEMPLATE[off..off + 4], &[0, 0, 0, 0]);
        }
        // Timestamp region also zeroed.
        assert_eq!(&TEMPLATE[0x08..0x10], &[0u8; 8]);
    }

    #[test]
    fn write_patches_all_name_fields() {
        let mut out = Vec::new();
        write("fizz", &mut out).unwrap();
        assert_eq!(out.len(), 326);

        for &off in &NAME_OFFSETS {
            assert_eq!(&out[off..off + 4], b"fizz");
        }
    }

    #[test]
    fn write_pads_short_code() {
        let mut out = Vec::new();
        write("ab", &mut out).unwrap();

        for &off in &NAME_OFFSETS {
            assert_eq!(&out[off..off + 4], b"ab\x00\x00");
        }
    }

    #[test]
    fn write_rejects_empty_code() {
        let mut out = Vec::new();
        assert!(matches!(write("", &mut out), Err(XsbError::BadCode { .. })));
    }

    #[test]
    fn write_rejects_too_long_code() {
        let mut out = Vec::new();
        assert!(matches!(
            write("abcde", &mut out),
            Err(XsbError::BadCode { .. })
        ));
    }

    #[test]
    fn write_rejects_non_alphanumeric() {
        let mut out = Vec::new();
        assert!(matches!(
            write("a-b!", &mut out),
            Err(XsbError::BadCode { .. })
        ));
    }

    #[test]
    fn timestamp_region_stays_zeroed() {
        let mut out = Vec::new();
        write("test", &mut out).unwrap();
        assert_eq!(&out[0x08..0x10], &[0u8; 8]);
    }
}
