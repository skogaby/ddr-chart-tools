//! Sound-effect bank-pair generator.
//!
//! Turns one short audio file into the XACT wave-bank + sound-bank pair a mod
//! can hand straight to DDR World's XACT 2 engine via
//! `IXACT2Engine::CreateInMemoryWaveBank` + `CreateSoundBank`, and play by cue
//! name.
//!
//! This module owns only the orchestration — decode, encode, assemble, write.
//! The container framing belongs to [`crate::xwb`], the codec to
//! [`crate::xwb::adpcm`], and the sound bank to [`crate::xsb::write_se`].
//!
//! # Why these constants and not others
//!
//! The engine validates an in-memory wave bank strictly and, for the sound
//! bank, *silently* rejects a bad CRC — audio simply goes dark with no error.
//! Every field below is therefore either a hard requirement of the engine's
//! validator or a value copied from the game's own shipped banks:
//!
//! - **MS-ADPCM, mono, 44100 Hz, `block_align_raw` 48.** Every wave entry in
//!   every DDR wave bank on disk is MS-ADPCM; there is not one PCM entry
//!   anywhere, so the PCM playback path in this engine build is entirely
//!   unexercised. Mono in an in-memory bank *is* exercised — it is what the
//!   cabinet plays when you insert a coin.
//! - **`TYPE_BUFFER`** (flags bit 0 clear). The bank-type bit is a hard gate in
//!   both directions: `CreateInMemoryWaveBank` rejects a streaming bank and the
//!   file-backed path rejects a buffer bank.
//! - **`header_version` 42** and **`entry_name_element_size` 64** are checked
//!   unconditionally, the latter even when no entry names are used.
//! - **`build_time` 0**, so that generation is reproducible. The engine never
//!   validates it.
//!
//! Input must already be mono 44100 Hz; this module will not resample or
//! downmix, because doing so silently would hide a mistake in the caller's
//! pipeline, and doing so with an external tool would make the output depend on
//! that tool's version and break reproducibility.

use std::fs;
use std::path::{Path, PathBuf};

use log::info;
use thiserror::Error;

use crate::error::Error;
use crate::ogg;
use crate::xsb;
use crate::xwb::{self, adpcm, WaveFormat, XwbBank, XwbEntry};

// ---------------------------------------------------------------------------
// Format constants
// ---------------------------------------------------------------------------

/// Packed `WAVEBANKMINIWAVEFORMAT`: MS-ADPCM (codec 2), 1 channel, 44100 Hz,
/// `block_align_raw` 48 — the only codec configuration DDR's authoring tool
/// ever emits. Derived, not stored: `block_align` 70 bytes, 128 samples/block.
const SE_FORMAT_BITS: u32 = 2 | (1 << 2) | (SE_SAMPLE_RATE << 5) | (48 << 23);

/// Required channel count of the input.
const SE_CHANNELS: u16 = 1;
/// Required sample rate of the input.
const SE_SAMPLE_RATE: u32 = 44100;
/// Decoded samples per ADPCM block at the format above.
const SAMPLES_PER_BLOCK: u32 = 128;

/// `TYPE_BUFFER` (bit 0 clear) + `ENTRYNAMES` (bit 16) + bit 19 — byte for byte
/// the value both stock in-memory banks carry. Bit 16 must agree with a
/// non-empty entry-name segment, which the container writer guarantees.
const SE_BANK_FLAGS: u32 = 0x0009_0000;
/// The engine requires exactly 42.
const SE_HEADER_VERSION: u32 = 42;
/// Required unconditionally, even though nothing looks the names up.
const SE_ENTRY_NAME_ELEMENT_SIZE: u32 = 64;
/// The validator's minimum, and what both stock in-memory banks use. A larger
/// alignment would only pad the blob.
const SE_ALIGNMENT: u32 = 4;

/// Environment variable naming an extracted stock in-memory SE wave bank, used
/// by the optional shape-comparison test. Documented here so it is greppable
/// from outside the test module.
#[cfg(test)]
const STOCK_XWB_ENV: &str = "DDR_STOCK_XWB";

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum SeBankError {
    #[error(
        "input must be mono, got {channels} channels \
         (downmix it yourself — this tool will not do it silently)"
    )]
    NotMono { channels: u16 },

    #[error(
        "input must be {SE_SAMPLE_RATE} Hz, got {sample_rate} Hz \
         (resample it yourself — this tool will not do it silently)"
    )]
    WrongSampleRate { sample_rate: u32 },

    #[error("input decoded to no samples; a wave bank must contain at least one entry")]
    NoAudio,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// What a successful generation produced.
#[derive(Debug, Clone)]
pub struct SeBankOutput {
    pub xwb_path: PathBuf,
    pub xsb_path: PathBuf,
    pub xwb_len: usize,
    pub xsb_len: usize,
    /// Samples decoded from the input, before block padding.
    pub source_samples: usize,
    /// Whole ADPCM blocks written.
    pub blocks: usize,
    /// `blocks * 128` — the entry's declared duration and loop length.
    pub total_samples: u32,
}

/// Generate `{name}.xwb` + `{name}.xsb` in `out_dir` from an Ogg Vorbis file.
///
/// `input` must decode to **mono 44100 Hz**; anything else is an error rather
/// than something quietly converted. `name` becomes the wave bank's internal
/// name, both of the sound bank's name fields, the single entry's name, and the
/// single cue's name — the engine matches banks by name and resolves cues with a
/// byte-exact `strcmp`, so its case is significant.
///
/// Output is deterministic: the same input file and name produce byte-identical
/// files on any machine.
///
/// Both files are built in memory first, so a rejected name or a bad input
/// cannot leave a half-written pair behind.
pub fn generate(input: &Path, name: &str, out_dir: &Path) -> Result<SeBankOutput, Error> {
    // Build the sound bank first: it validates `name`, so an unusable name
    // fails before any audio work and before anything touches the filesystem.
    let mut xsb_bytes = Vec::new();
    xsb::write_se(name, &mut xsb_bytes)?;

    let audio = ogg::decode::decode(&fs::read(input)?)?;
    if audio.channels != SE_CHANNELS {
        return Err(SeBankError::NotMono {
            channels: audio.channels,
        }
        .into());
    }
    if audio.sample_rate != SE_SAMPLE_RATE {
        return Err(SeBankError::WrongSampleRate {
            sample_rate: audio.sample_rate,
        }
        .into());
    }
    if audio.samples.is_empty() {
        return Err(SeBankError::NoAudio.into());
    }

    let format = WaveFormat::from_packed(SE_FORMAT_BITS);
    let data = adpcm::encode::encode(&audio.samples, &format)?;
    let block_align = format.block_align() as usize;
    // The encoder pads to whole blocks and `block_align` is a non-zero
    // constant here, so this division is exact and cannot divide by zero.
    let blocks = data.len() / block_align;
    let total_samples = blocks as u32 * SAMPLES_PER_BLOCK;

    let bank = XwbBank {
        header_version: SE_HEADER_VERSION,
        flags: SE_BANK_FLAGS,
        name: fixed_name(name),
        entry_name_element_size: SE_ENTRY_NAME_ELEMENT_SIZE,
        alignment: SE_ALIGNMENT,
        compact_format: 0,
        build_time: 0,
        entries: vec![XwbEntry {
            // The low nibble is the entry-flags field and must be zero.
            flags_and_duration: total_samples << 4,
            format,
            data,
            loop_start: 0,
            // `Duration >= loop_start + loop_length` is the engine's only
            // constraint; equality is what stock entries do.
            loop_length: total_samples,
            name_bytes: name.as_bytes().to_vec(),
        }],
    };

    let mut xwb_bytes = Vec::new();
    xwb::write(&bank, &mut xwb_bytes)?;

    fs::create_dir_all(out_dir)?;
    let xwb_path = out_dir.join(format!("{name}.xwb"));
    let xsb_path = out_dir.join(format!("{name}.xsb"));
    fs::write(&xwb_path, &xwb_bytes)?;
    fs::write(&xsb_path, &xsb_bytes)?;

    info!(
        "wrote {} ({} B, {} blocks) and {} ({} B)",
        xwb_path.display(),
        xwb_bytes.len(),
        blocks,
        xsb_path.display(),
        xsb_bytes.len()
    );

    Ok(SeBankOutput {
        xwb_path,
        xsb_path,
        xwb_len: xwb_bytes.len(),
        xsb_len: xsb_bytes.len(),
        source_samples: audio.samples.len(),
        blocks,
        total_samples,
    })
}

/// Copy `name` into the wave bank's fixed 64-byte name field.
///
/// Truncation cannot lose anything: [`xsb::write_se`] has already rejected any
/// name longer than its own maximum, which is far below 64.
fn fixed_name(name: &str) -> [u8; 64] {
    let mut field = [0u8; 64];
    for (slot, &b) in field.iter_mut().zip(name.as_bytes()) {
        *slot = b;
    }
    field
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AudioBuffer;
    use crate::xwb::adpcm;
    use crate::xwb::container;

    const NAME: &str = "asti";
    /// Roughly the length of the assist-tick clap: ~0.21 s at 44.1 kHz.
    const CLAP_FRAMES: usize = 9423;

    /// Write a synthesized Ogg Vorbis file and return its path.
    ///
    /// Synthesizing beats committing a fixture here: it is licence-clean, keeps
    /// the test self-contained, and still exercises the real `ogg::decode`
    /// path the generator uses.
    fn synth_ogg(
        dir: &Path,
        stem: &str,
        frames: usize,
        channels: u16,
        sample_rate: u32,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let ch = channels as usize;
        let mut samples = Vec::with_capacity(frames * ch);
        for f in 0..frames {
            // A decaying 1 kHz burst — broadband enough that a broken encoder
            // shows up as poor SNR rather than accidentally round-tripping.
            let t = f as f64 / sample_rate as f64;
            let envelope = (-t * 25.0).exp();
            let v = (t * 1000.0 * std::f64::consts::TAU).sin() * envelope * 20000.0;
            for _ in 0..ch {
                samples.push(v as i16);
            }
        }
        let audio = AudioBuffer {
            samples,
            sample_rate,
            channels,
        };
        let path = dir.join(format!("{stem}.ogg"));
        let mut bytes = Vec::new();
        crate::ogg::encode::encode(&audio, &mut bytes)?;
        fs::write(&path, &bytes)?;
        Ok(path)
    }

    fn mono_clap(dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
        synth_ogg(dir, "clap", CLAP_FRAMES, 1, 44100)
    }

    fn dump_map(bytes: &[u8]) -> std::collections::HashMap<String, String> {
        crate::xwb::dump::describe(bytes)
            .expect("generated bank must dump")
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // ---- A1: a bank pair is generated from a real input ------------------

    #[test]
    fn generates_a_bank_pair_from_a_mono_44100_ogg() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let input = mono_clap(dir.path())?;
        let out = dir.path().join("banks");

        let result = generate(&input, NAME, &out)?;

        assert!(result.xwb_path.is_file(), "wave bank written");
        assert!(result.xsb_path.is_file(), "sound bank written");
        assert_eq!(result.xwb_path.file_name().unwrap(), "asti.xwb");
        assert_eq!(result.xsb_path.file_name().unwrap(), "asti.xsb");

        let xwb = fs::read(&result.xwb_path)?;
        let bank = container::parse(&xwb)?;
        assert_eq!(bank.entries.len(), 1, "exactly one entry");
        assert_eq!(bank.name_str(), NAME);

        // The SE sound bank's size is fully determined by the name length.
        let xsb = fs::read(&result.xsb_path)?;
        assert_eq!(xsb.len(), 0x101 + NAME.len() + 1);
        assert_eq!(xsb.len(), result.xsb_len);
        assert_eq!(xwb.len(), result.xwb_len);
        Ok(())
    }

    // ---- A2: the container conforms to the engine's validator ------------

    #[test]
    fn wave_bank_header_matches_the_in_memory_profile() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let input = mono_clap(dir.path())?;
        let result = generate(&input, NAME, dir.path())?;
        let d = dump_map(&fs::read(&result.xwb_path)?);

        assert_eq!(d["bank.version"], "43");
        assert_eq!(d["bank.header_version"], "42", "engine requires exactly 42");
        assert_eq!(d["bank.flags"], "0x00090000");
        assert_eq!(
            d["bank.type"], "buffer",
            "CreateInMemoryWaveBank rejects a streaming bank outright"
        );
        assert_eq!(d["bank.has_entry_names"], "1");
        assert_eq!(d["bank.entry_count"], "1");
        assert_eq!(d["bank.entry_metadata_element_size"], "24");
        assert_eq!(d["bank.entry_name_element_size"], "64", "required even so");
        assert_eq!(d["bank.alignment"], "4");
        assert_eq!(d["bank.compact_format"], "0");
        assert_eq!(d["bank.build_time"], "0", "fixed, for reproducibility");
        assert_eq!(d["bank.name_terminated"], "1");
        Ok(())
    }

    #[test]
    fn wave_bank_segment_layout_is_exactly_what_the_validator_demands(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let input = mono_clap(dir.path())?;
        let result = generate(&input, NAME, dir.path())?;
        let xwb = fs::read(&result.xwb_path)?;
        let d = dump_map(&xwb);
        let n = |k: &str| -> usize { d[k].parse().expect("numeric dump field") };

        assert_eq!(n("segment0.offset"), 0x34, "BANKDATA at exactly 0x34");
        assert_eq!(n("segment0.length"), 0x60, "BANKDATA is a fixed 96 bytes");
        assert_eq!(n("segment1.offset"), 0x94, "ENTRYMETADATA at exactly 0x94");
        assert_eq!(n("segment1.length"), 24, "entry_count * 24");
        assert_eq!(n("segment2.length"), 0, "SEEKTABLES must be empty");
        assert_eq!(
            n("segment3.offset"),
            0x94 + 24,
            "ENTRYNAMES immediately after ENTRYMETADATA"
        );
        assert_eq!(n("segment3.length"), 64, "entry_count * 64");
        assert_eq!(
            xwb.len() - n("segment4.offset"),
            n("segment4.length"),
            "wave data must run precisely to the end of the buffer"
        );
        Ok(())
    }

    #[test]
    fn wave_bank_entry_fields_satisfy_the_per_entry_rules() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let input = mono_clap(dir.path())?;
        let result = generate(&input, NAME, dir.path())?;
        let d = dump_map(&fs::read(&result.xwb_path)?);
        let n = |k: &str| -> u64 { d[k].parse().expect("numeric dump field") };

        assert_eq!(n("entry0.entry_flags") & 7, 0, "entry flag bits 0-2 clear");
        assert!(
            n("entry0.duration") >= n("entry0.loop_start") + n("entry0.loop_length"),
            "the only constraint the engine places on Duration"
        );
        assert_eq!(d["entry0.codec"], "2", "MS-ADPCM");
        assert_eq!(d["entry0.channels"], "1");
        assert_eq!(d["entry0.sample_rate"], "44100");
        assert_eq!(d["entry0.block_align_raw"], "48");
        assert_eq!(d["entry0.block_align"], "70");
        assert_eq!(d["entry0.samples_per_block"], "128");
        assert_eq!(d["entry0.bits_per_sample_flag"], "0");
        assert_eq!(d["entry0.name"], NAME, "entry named after the bank");
        assert_eq!(d["entry0.name_terminated"], "1");
        assert_eq!(d["entry0.data_offset"], "0");
        assert_eq!(
            n("entry0.data_length") % 70,
            0,
            "exact whole ADPCM blocks - stock banks are sloppy here, we are not"
        );
        assert_eq!(n("entry0.data_length"), (result.blocks * 70) as u64);
        assert_eq!(n("entry0.duration"), u64::from(result.total_samples));
        Ok(())
    }

    // ---- A3: the audio survives the encode -------------------------------

    #[test]
    fn encoded_audio_decodes_back_faithfully() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let input = mono_clap(dir.path())?;
        let result = generate(&input, NAME, dir.path())?;

        let bank = container::parse(&fs::read(&result.xwb_path)?)?;
        let entry = bank.entries.first().expect("one entry");
        let decoded = adpcm::decode::decode(&entry.data, &entry.format)?;

        assert_eq!(decoded.len(), result.total_samples as usize);
        assert!(
            decoded.len() >= result.source_samples
                && decoded.len() - result.source_samples < SAMPLES_PER_BLOCK as usize,
            "sample count matches the source within one block: {} vs {}",
            decoded.len(),
            result.source_samples
        );
        assert!(
            decoded.iter().any(|&s| s != 0),
            "decode must not be silence"
        );
        assert!(
            !decoded.iter().any(|&s| s == i16::MIN || s == i16::MAX),
            "decode must not clip"
        );

        // Compare against the source PCM. This catches a silently-broken
        // encode (wrong predictor, wrong nibble packing, wrong initial delta)
        // that no container check can see.
        //
        // The threshold is calibrated for *this* test's tonal input, which
        // measures ~47 dB. Real-world numbers on the assist-tick clap — a
        // broadband transient, the hardest case for ADPCM — measured during
        // this task, for the record:
        //   as shipped                          17.4 dB
        //   with the predictor search disabled   16.6 dB  (so it does help)
        //   with a rounding rather than          22.5 dB  (a ~5 dB win that is
        //     truncating quantizer                        out of scope here:
        //                                                 `adpcm::encode` is
        //                                                 shared with the song
        //                                                 conversion path)
        let source = crate::ogg::decode::decode(&fs::read(&input)?)?;
        let mut signal = 0.0f64;
        let mut noise = 0.0f64;
        for (orig, dec) in source.samples.iter().zip(decoded.iter()) {
            let s = f64::from(*orig);
            let n = s - f64::from(*dec);
            signal += s * s;
            noise += n * n;
        }
        let snr_db = 10.0 * (signal / noise).log10();
        assert!(
            snr_db > 30.0,
            "SNR {snr_db:.1} dB indicates a broken encode"
        );
        Ok(())
    }

    // ---- A4: the two banks agree on the wave-bank name -------------------

    #[test]
    fn wave_bank_and_sound_bank_names_are_byte_identical() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let input = mono_clap(dir.path())?;
        // Mixed case, because the engine's name match is byte-exact.
        let result = generate(&input, "AsTi", dir.path())?;

        let bank = container::parse(&fs::read(&result.xwb_path)?)?;
        let xsb = fs::read(&result.xsb_path)?;
        // The XSB's wave-bank-name field is the 64 bytes at 0x8A.
        assert_eq!(&bank.name[..], &xsb[0x8A..0x8A + 64]);
        assert_eq!(bank.name_str(), "AsTi");
        Ok(())
    }

    // ---- A5: generation is deterministic ---------------------------------

    #[test]
    fn generation_is_byte_reproducible() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let input = mono_clap(dir.path())?;

        let a = generate(&input, NAME, &dir.path().join("a"))?;
        let b = generate(&input, NAME, &dir.path().join("b"))?;

        assert_eq!(fs::read(&a.xwb_path)?, fs::read(&b.xwb_path)?);
        assert_eq!(fs::read(&a.xsb_path)?, fs::read(&b.xsb_path)?);
        Ok(())
    }

    // ---- A6: the generated bank matches the stock banks' shape -----------

    #[test]
    fn generated_bank_matches_stock_bank_shape() -> Result<(), Box<dyn std::error::Error>> {
        // Stock banks live inside the game install's ARC containers and are
        // committed to neither repository, so this check is opt-in.
        let Some(stock_path) = std::env::var_os(STOCK_XWB_ENV) else {
            eprintln!(
                "SKIP generated_bank_matches_stock_bank_shape: set {STOCK_XWB_ENV} to an \
                 extracted stock in-memory SE wave bank (.xwb) to run it"
            );
            return Ok(());
        };
        let Ok(stock) = fs::read(&stock_path) else {
            eprintln!(
                "SKIP generated_bank_matches_stock_bank_shape: {STOCK_XWB_ENV} is set but \
                 {stock_path:?} is not readable"
            );
            return Ok(());
        };

        let dir = tempfile::tempdir()?;
        let input = mono_clap(dir.path())?;
        let result = generate(&input, NAME, dir.path())?;

        let ours = dump_map(&fs::read(&result.xwb_path)?);
        let theirs = dump_map(&stock);

        for key in [
            "bank.version",
            "bank.flags",
            "bank.alignment",
            "bank.entry_name_element_size",
            "bank.header_version",
            "bank.entry_metadata_element_size",
        ] {
            assert_eq!(
                ours[key], theirs[key],
                "{key} must match the stock in-memory bank"
            );
        }
        // And the intended differences really are different.
        assert_ne!(ours["bank.name"], theirs["bank.name"]);
        Ok(())
    }

    // ---- A7: bad input fails cleanly -------------------------------------

    #[test]
    fn rejects_stereo_input() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let input = synth_ogg(dir.path(), "stereo", 4096, 2, 44100)?;
        let err = generate(&input, NAME, dir.path()).expect_err("stereo must be rejected");
        assert!(
            matches!(err, Error::SeBank(SeBankError::NotMono { channels: 2 })),
            "expected NotMono, got {err}"
        );
        assert!(err.to_string().contains('2'), "message names the count");
        Ok(())
    }

    #[test]
    fn rejects_wrong_sample_rate() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let input = synth_ogg(dir.path(), "slow", 4096, 1, 48000)?;
        let err = generate(&input, NAME, dir.path()).expect_err("48 kHz must be rejected");
        assert!(
            matches!(
                err,
                Error::SeBank(SeBankError::WrongSampleRate { sample_rate: 48000 })
            ),
            "expected WrongSampleRate, got {err}"
        );
        assert!(err.to_string().contains("48000"), "message names the rate");
        Ok(())
    }

    #[test]
    fn rejects_missing_input_file() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let err = generate(&dir.path().join("absent.ogg"), NAME, dir.path())
            .expect_err("a missing file must be rejected");
        assert!(matches!(err, Error::Io(_)), "expected I/O error, got {err}");
        Ok(())
    }

    #[test]
    fn rejects_a_file_that_is_not_audio() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let input = dir.path().join("notaudio.ogg");
        fs::write(&input, b"this is definitely not an Ogg Vorbis stream")?;
        let err = generate(&input, NAME, dir.path()).expect_err("non-audio must be rejected");
        assert!(
            matches!(err, Error::Ogg(_)),
            "expected Ogg error, got {err}"
        );
        Ok(())
    }

    #[test]
    fn rejects_a_bad_name_without_writing_anything() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let input = mono_clap(dir.path())?;
        let out = dir.path().join("banks");

        let err = generate(&input, "not a name!", &out).expect_err("a bad name must be rejected");
        assert!(
            matches!(err, Error::Xsb(_)),
            "expected XSB error, got {err}"
        );
        assert!(
            !out.exists(),
            "nothing may be written when the name is invalid"
        );
        Ok(())
    }

    #[test]
    fn rejects_empty_audio() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let input = synth_ogg(dir.path(), "empty", 0, 1, 44100)?;
        let err = generate(&input, NAME, dir.path()).expect_err("silence-length 0 is not a sound");
        assert!(
            matches!(err, Error::SeBank(SeBankError::NoAudio)),
            "expected NoAudio, got {err}"
        );
        Ok(())
    }
}
