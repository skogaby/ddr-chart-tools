//! SSQ chunk-header framing.
//!
//! Every SSQ chunk starts with a 12-byte header (spec §2). This module
//! reads headers and implements the two termination conditions shared
//! by the game's chunk-lookup loops (spec §2.1, §2.3):
//!
//! - `length == 0` → file terminator
//! - `param2 == 0xFFFF` → forward-compat sentinel, abort walk

use crate::util::io::{IoError, LeReader};

use super::SsqError;

/// 12-byte SSQ chunk header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkHeader {
    pub length: u32,
    pub ty: u16,
    pub param2: u16,
    pub param3: u16,
    pub param4: u16,
}

impl ChunkHeader {
    pub const HEADER_SIZE: u32 = 12;

    /// Body size (chunk length minus header).
    #[must_use]
    pub fn body_size(&self) -> u32 {
        self.length - Self::HEADER_SIZE
    }
}

/// Read the next chunk header from `reader`, or `None` at end of file.
///
/// The DDR World game engine treats `param2 == 0xFFFF` as a walk-abort
/// sentinel, but this tool reads past it — legacy SSQs may use that
/// value on real chunks, and skipping them would lose data. The modern
/// writer never emits `0xFFFF`, so output is always DDR World-safe.
///
/// Returns the header's starting byte offset on success for error reporting.
pub fn read_header(reader: &mut LeReader) -> Result<Option<(usize, ChunkHeader)>, SsqError> {
    let offset = reader.position();

    if reader.remaining() < 4 {
        return Ok(None);
    }
    let length = reader.read_u32().map_err(chunk_io)?;
    if length == 0 {
        return Ok(None);
    }
    if length < ChunkHeader::HEADER_SIZE {
        return Err(SsqError::MalformedChunk {
            offset,
            reason: format!("chunk length {length} is smaller than 12-byte header"),
        });
    }
    if length % 4 != 0 {
        return Err(SsqError::MalformedChunk {
            offset,
            reason: format!("chunk length {length} is not dword-aligned"),
        });
    }

    let ty = reader.read_u16().map_err(chunk_io)?;
    let param2 = reader.read_u16().map_err(chunk_io)?;
    let param3 = reader.read_u16().map_err(chunk_io)?;
    let param4 = reader.read_u16().map_err(chunk_io)?;

    Ok(Some((
        offset,
        ChunkHeader {
            length,
            ty,
            param2,
            param3,
            param4,
        },
    )))
}

fn chunk_io(err: IoError) -> SsqError {
    SsqError::Io(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_bytes(length: u32, ty: u16, param2: u16, param3: u16, param4: u16) -> Vec<u8> {
        let mut v = Vec::with_capacity(12);
        v.extend_from_slice(&length.to_le_bytes());
        v.extend_from_slice(&ty.to_le_bytes());
        v.extend_from_slice(&param2.to_le_bytes());
        v.extend_from_slice(&param3.to_le_bytes());
        v.extend_from_slice(&param4.to_le_bytes());
        v
    }

    #[test]
    fn reads_valid_header() {
        let bytes = header_bytes(20, 1, 1000, 2, 0);
        let mut r = LeReader::new(&bytes);
        let (off, h) = read_header(&mut r).unwrap().unwrap();
        assert_eq!(off, 0);
        assert_eq!(h.length, 20);
        assert_eq!(h.ty, 1);
        assert_eq!(h.param2, 1000);
        assert_eq!(h.body_size(), 8);
    }

    #[test]
    fn length_zero_returns_none() {
        let bytes = [0u8; 4]; // length = 0
        let mut r = LeReader::new(&bytes);
        assert!(read_header(&mut r).unwrap().is_none());
    }

    #[test]
    fn empty_buffer_returns_none() {
        let bytes: [u8; 0] = [];
        let mut r = LeReader::new(&bytes);
        assert!(read_header(&mut r).unwrap().is_none());
    }

    #[test]
    fn ffff_param2_is_read_normally() {
        let bytes = header_bytes(12, 3, 0xFFFF, 5, 0);
        let mut r = LeReader::new(&bytes);
        let (_, h) = read_header(&mut r).unwrap().unwrap();
        assert_eq!(h.ty, 3);
        assert_eq!(h.param2, 0xFFFF);
        assert_eq!(h.param3, 5);
    }

    #[test]
    fn length_below_header_size_is_malformed() {
        let bytes = header_bytes(8, 1, 1000, 0, 0);
        let mut r = LeReader::new(&bytes);
        let err = read_header(&mut r).unwrap_err();
        assert!(matches!(err, SsqError::MalformedChunk { .. }));
    }

    #[test]
    fn non_dword_aligned_length_is_malformed() {
        let bytes = header_bytes(13, 1, 1000, 0, 0);
        let mut r = LeReader::new(&bytes);
        let err = read_header(&mut r).unwrap_err();
        assert!(matches!(err, SsqError::MalformedChunk { .. }));
    }
}
