//! Top-level error type.
//!
//! One variant per module error. Each format module defines its own
//! `thiserror`-derived error; this enum wraps them via `#[from]` so
//! callers at the CLI boundary can handle any parse/write failure
//! uniformly.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("SM error: {0}")]
    Sm(#[from] crate::sm::SmError),

    #[error("SSQ error: {0}")]
    Ssq(#[from] crate::ssq::SsqError),

    #[error("SSC error: {0}")]
    Ssc(#[from] crate::ssc::SscError),

    #[error("OGG error: {0}")]
    Ogg(#[from] crate::ogg::OggError),

    #[error("WAVM error: {0}")]
    Wavm(#[from] crate::wavm::WavmError),

    #[error("XWB error: {0}")]
    Xwb(#[from] crate::xwb::XwbError),

    #[error("XSB error: {0}")]
    Xsb(#[from] crate::xsb::XsbError),

    #[error("ADPCM error: {0}")]
    Adpcm(#[from] crate::xwb::adpcm::AdpcmError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Placeholder variant kept while not all module errors are wired.
    #[error("not yet implemented")]
    NotImplemented,
}
