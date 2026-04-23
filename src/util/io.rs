//! Little-endian byte-slice reader with offset tracking.
//!
//! Binary formats parsed by this crate (SSQ, XWB, WAVM) are strictly
//! little-endian and frequently need a byte offset in error messages.
//! `LeReader` wraps a `&[u8]` with a cursor and reports the current
//! position on every EOF, so callers can surface "unexpected EOF at
//! byte N" without threading an offset counter through every call site.

use thiserror::Error;

/// Errors from reading past the end of the underlying buffer.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IoError {
    /// The reader was asked for more bytes than remain in the buffer.
    #[error(
        "unexpected end of buffer at byte {offset}: wanted {wanted} byte(s), {remaining} remaining"
    )]
    UnexpectedEof {
        offset: usize,
        wanted: usize,
        remaining: usize,
    },
}

/// Cursor over a byte slice that reads little-endian integers and
/// tracks the current offset for diagnostic purposes.
#[derive(Debug, Clone)]
pub struct LeReader<'a> {
    buf: &'a [u8],
    offset: usize,
}

impl<'a> LeReader<'a> {
    /// Create a reader positioned at the start of `buf`.
    #[must_use]
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, offset: 0 }
    }

    /// Current read position, in bytes from the start of the buffer.
    #[must_use]
    pub fn position(&self) -> usize {
        self.offset
    }

    /// Number of unread bytes remaining.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.offset
    }

    /// Advance the cursor by `len` bytes and return the consumed slice.
    pub fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], IoError> {
        let end = self.offset.checked_add(len).ok_or(IoError::UnexpectedEof {
            offset: self.offset,
            wanted: len,
            remaining: self.remaining(),
        })?;
        if end > self.buf.len() {
            return Err(IoError::UnexpectedEof {
                offset: self.offset,
                wanted: len,
                remaining: self.remaining(),
            });
        }
        let slice = &self.buf[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    /// Read a single unsigned byte.
    pub fn read_u8(&mut self) -> Result<u8, IoError> {
        let bytes = self.read_bytes(1)?;
        Ok(bytes[0])
    }

    /// Read a little-endian `u16`.
    pub fn read_u16(&mut self) -> Result<u16, IoError> {
        let bytes = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    /// Read a little-endian `u32`.
    pub fn read_u32(&mut self) -> Result<u32, IoError> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_consume_and_advance_offset() {
        let buf = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
        let mut r = LeReader::new(&buf);

        assert_eq!(r.read_u8().unwrap(), 0x01);
        assert_eq!(r.position(), 1);

        assert_eq!(r.read_u16().unwrap(), 0x0302);
        assert_eq!(r.position(), 3);

        assert_eq!(r.read_u32().unwrap(), 0x0706_0504);
        assert_eq!(r.position(), 7);

        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn read_bytes_returns_borrowed_slice() {
        let buf = [0xAA, 0xBB, 0xCC, 0xDD];
        let mut r = LeReader::new(&buf);
        let slice = r.read_bytes(3).unwrap();
        assert_eq!(slice, &[0xAA, 0xBB, 0xCC]);
        assert_eq!(r.position(), 3);
    }

    #[test]
    fn eof_on_u8_reports_offset() {
        let buf = [];
        let mut r = LeReader::new(&buf);
        let err = r.read_u8().unwrap_err();
        assert_eq!(
            err,
            IoError::UnexpectedEof {
                offset: 0,
                wanted: 1,
                remaining: 0,
            }
        );
    }

    #[test]
    fn eof_on_u16_when_one_byte_left() {
        let buf = [0x42];
        let mut r = LeReader::new(&buf);
        let err = r.read_u16().unwrap_err();
        assert_eq!(
            err,
            IoError::UnexpectedEof {
                offset: 0,
                wanted: 2,
                remaining: 1,
            }
        );
    }

    #[test]
    fn eof_on_u32_mid_buffer() {
        let buf = [0xAA; 6];
        let mut r = LeReader::new(&buf);
        r.read_u32().unwrap();
        let err = r.read_u32().unwrap_err();
        assert_eq!(
            err,
            IoError::UnexpectedEof {
                offset: 4,
                wanted: 4,
                remaining: 2,
            }
        );
    }

    #[test]
    fn read_bytes_zero_length_succeeds_and_does_not_advance() {
        let buf = [0x01, 0x02];
        let mut r = LeReader::new(&buf);
        let slice = r.read_bytes(0).unwrap();
        assert!(slice.is_empty());
        assert_eq!(r.position(), 0);
    }

    #[test]
    fn read_bytes_beyond_buffer_reports_eof() {
        let buf = [0x01, 0x02];
        let mut r = LeReader::new(&buf);
        let err = r.read_bytes(5).unwrap_err();
        assert_eq!(
            err,
            IoError::UnexpectedEof {
                offset: 0,
                wanted: 5,
                remaining: 2,
            }
        );
    }

    #[test]
    fn failed_read_does_not_advance_offset() {
        let buf = [0x42];
        let mut r = LeReader::new(&buf);
        let _ = r.read_u16();
        assert_eq!(r.position(), 0);
        // Subsequent successful read still works.
        assert_eq!(r.read_u8().unwrap(), 0x42);
    }
}
