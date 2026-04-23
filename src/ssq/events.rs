//! Type-2 event-stream chunk parser (spec §4).
//!
//! Events are preserved opaquely as `(tick, code, arg)` triples so that
//! DDR→DDR round-trips reproduce non-canonical event sequences
//! verbatim. DDR→SM5 conversion ignores them — no cross-format mapping
//! is defined.

use crate::util::io::LeReader;

use super::chunk::ChunkHeader;
use super::SsqError;

/// One entry from a type-2 event chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SsqEvent {
    pub tick: i32,
    pub code: u8,
    pub arg: u8,
}

/// Parse a type-2 event chunk into its raw event list.
pub fn parse_events_chunk(
    header: &ChunkHeader,
    body: &[u8],
    chunk_offset: usize,
) -> Result<Vec<SsqEvent>, SsqError> {
    if header.param2 != 1 {
        log::warn!(
            "events chunk at byte {chunk_offset} has param2={} (expected 1)",
            header.param2
        );
    }

    let entry_count = usize::from(header.param3);
    let expected_body = entry_count.checked_mul(6).ok_or(SsqError::MalformedChunk {
        offset: chunk_offset,
        reason: format!("events entry count {entry_count} overflows body size"),
    })?;
    // The body may have up to 3 trailing zero-pad bytes so the total
    // chunk length is dword-aligned. Accept any size in
    // `expected_body ..= expected_body + 3` and ignore the tail.
    if body.len() < expected_body || body.len() > expected_body + 3 {
        return Err(SsqError::MalformedChunk {
            offset: chunk_offset,
            reason: format!(
                "events body size {} does not match {entry_count} entries × 6 bytes (+ up to 3 pad)",
                body.len()
            ),
        });
    }

    if entry_count == 0 {
        return Ok(Vec::new());
    }

    let mut reader = LeReader::new(body);
    let mut ticks = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        ticks.push(reader.read_u32().map_err(SsqError::Io)? as i32);
    }
    let mut events = Vec::with_capacity(entry_count);
    for tick in ticks {
        let code = reader.read_u8().map_err(SsqError::Io)?;
        let arg = reader.read_u8().map_err(SsqError::Io)?;
        events.push(SsqEvent { tick, code, arg });
    }

    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_events_chunk(
        param2: u16,
        ticks: &[i32],
        codes: &[(u8, u8)],
    ) -> (ChunkHeader, Vec<u8>) {
        assert_eq!(ticks.len(), codes.len());
        let n = ticks.len() as u16;
        let header = ChunkHeader {
            length: 12 + 6 * u32::from(n),
            ty: 2,
            param2,
            param3: n,
            param4: 0,
        };
        let mut body = Vec::new();
        for t in ticks {
            body.extend_from_slice(&(*t as u32).to_le_bytes());
        }
        for (c, a) in codes {
            body.push(*c);
            body.push(*a);
        }
        (header, body)
    }

    #[test]
    fn parses_canonical_event_sequence() {
        // Matches docs §4.4 canonical 6-entry pattern seen in most DDR World files.
        let ticks = [0, 0, 4096, 4096, 315_392, 319_488];
        let codes = [(1, 4), (2, 1), (2, 2), (2, 5), (2, 3), (2, 4)];
        let (h, body) = build_events_chunk(1, &ticks, &codes);
        let events = parse_events_chunk(&h, &body, 0).unwrap();
        assert_eq!(events.len(), 6);
        assert_eq!(
            events[0],
            SsqEvent {
                tick: 0,
                code: 1,
                arg: 4
            }
        );
        assert_eq!(
            events[5],
            SsqEvent {
                tick: 319_488,
                code: 2,
                arg: 4,
            }
        );
    }

    #[test]
    fn preserves_non_canonical_code1_events() {
        // Docs §4.4: 82 files have code-1 events with non-canonical args.
        let ticks = [0, 12345];
        let codes = [(1, 4), (1, 17)];
        let (h, body) = build_events_chunk(1, &ticks, &codes);
        let events = parse_events_chunk(&h, &body, 0).unwrap();
        assert_eq!(events[1].code, 1);
        assert_eq!(events[1].arg, 17);
        assert_eq!(events[1].tick, 12345);
    }

    #[test]
    fn zero_entries_returns_empty_vec() {
        let h = ChunkHeader {
            length: 12,
            ty: 2,
            param2: 1,
            param3: 0,
            param4: 0,
        };
        assert!(parse_events_chunk(&h, &[], 0).unwrap().is_empty());
    }

    #[test]
    fn body_size_mismatch_is_rejected() {
        let h = ChunkHeader {
            length: 12 + 6 * 2,
            ty: 2,
            param2: 1,
            param3: 2,
            param4: 0,
        };
        let short_body = vec![0u8; 6]; // should be 12
        let err = parse_events_chunk(&h, &short_body, 0).unwrap_err();
        assert!(matches!(err, SsqError::MalformedChunk { .. }));
    }

    #[test]
    fn unexpected_param2_warns_but_parses() {
        // 1523/1523 real files have param2=1, but the parser shouldn't crash
        // if a file deviates.
        let (h, body) = build_events_chunk(42, &[0], &[(1, 4)]);
        let events = parse_events_chunk(&h, &body, 0).unwrap();
        assert_eq!(events.len(), 1);
    }
}
