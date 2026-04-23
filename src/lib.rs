//! Library surface for `ddr-chart-tools`.
//!
//! Exposes the crate's public items so integration tests under `tests/`
//! and the `main.rs` binary can link against a single source of truth.

pub mod cli;
pub mod error;
pub mod job;
pub mod model;
pub mod ogg;
pub mod sm;
pub mod ssc;
pub mod ssq;
pub mod ssq_legacy;
pub mod util;
pub mod wavm;
pub mod xsb;
pub mod xwb;

pub use error::Error;
