//! Auxiliary chunk handling (types 4, 5, 9, 17, 18).
//!
//! These chunks appear in SSQs authored by older DDR pipelines (type 4
//! and type 5 only in TPS=150 files; type 9 in one file; type 17 in 13
//! files; type 18 only in DDR Hottest Party 5). They carry effect
//! scripting, stage-lamp cues, and section markers that the DDR World
//! step engine does not consume. Authoring tools targeting modern DDR
//! should not emit them.
//!
//! A type 9 chunk whose `param2` is a step difficulty code is not
//! auxiliary: it is a Hudson-format chart, handled by `ssq::hudson`.
//! Likewise a type 16 chunk with a foot style code is a chart handled by
//! `ssq::hudson_lane`; type 16 chunks with Wii-controller style codes are
//! dropped through here.
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
