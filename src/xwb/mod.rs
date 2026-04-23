//! XWB (XACT Wave Bank) v43 container parse and write.
//!
//! Handles the binary container framing: WBND header, five-segment
//! layout, per-entry metadata with `WAVEBANKMINIWAVEFORMAT` bitfield
//! packing, and entry name tables. The `adpcm` submodule decodes
//! MS-ADPCM blocks to PCM.

pub mod adpcm;
pub mod container;

pub use container::{WaveFormat, XwbBank, XwbEntry, XwbError};

use crate::model::AudioBuffer;

/// Parse an XWB file into a bank with raw audio data per entry.
pub fn parse(bytes: &[u8]) -> Result<XwbBank, XwbError> {
    container::parse(bytes)
}

/// Write an XWB file from a bank with raw audio data per entry.
pub fn write(bank: &XwbBank, out: &mut impl std::io::Write) -> Result<(), XwbError> {
    container::write(bank, out)
}

/// Parse an XWB file and decode the main (longest) entry to PCM.
///
/// The main track is selected by data length — the longest entry is
/// the full song, shorter entries are previews. This matches the
/// heuristic used by StepManiPaX rather than assuming entry order.
pub fn parse_audio(bytes: &[u8]) -> Result<AudioBuffer, XwbError> {
    let bank = container::parse(bytes)?;
    let entry = bank
        .entries
        .iter()
        .max_by_key(|e| e.data.len())
        .ok_or(XwbError::NoEntries)?;
    let samples =
        adpcm::decode::decode(&entry.data, &entry.format).map_err(XwbError::Adpcm)?;
    Ok(AudioBuffer {
        samples,
        sample_rate: entry.format.sample_rate(),
        channels: entry.format.channels() as u16,
    })
}
