//! Auxiliary chunk handling (types 4, 5, 9, 17).
//!
//! These chunks appear in SSQs authored by older DDR pipelines (type 4
//! and type 5 only in TPS=150 files; type 9 in one file; type 17 in 13
//! files). They carry effect scripting, stage-lamp cues, and section
//! markers that the DDR World step engine does not consume. Authoring
//! tools targeting modern DDR should not emit them.
//!
//! This parser does not preserve their contents. It emits [`AuxMeta`]
//! records describing what was dropped, which the caller can surface
//! in log output alongside the source filename.

/// Metadata describing an auxiliary chunk that was dropped during parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuxMeta {
    pub ty: u16,
    pub offset: usize,
    pub size: u32,
}
