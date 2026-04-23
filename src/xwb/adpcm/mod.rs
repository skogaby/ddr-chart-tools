//! MS-ADPCM codec for XWB wave banks.
//!
//! Decodes Microsoft ADPCM blocks (4-bit adaptive delta, 7 standard
//! predictor coefficient pairs) into interleaved 16-bit PCM. The
//! encoder is added in a later task.

pub mod decode;
pub mod encode;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AdpcmError {
    #[error("block too short: expected at least {expected} bytes, got {actual}")]
    BlockTooShort { expected: usize, actual: usize },

    #[error("predictor index {index} out of range (max 6)")]
    BadPredictor { index: u8 },
}
