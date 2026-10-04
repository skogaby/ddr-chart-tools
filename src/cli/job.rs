//! Job and Format types consumed by the conversion pipeline.

use std::path::PathBuf;

use clap::ValueEnum;

/// Supported format families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Modern DDR (SSQ + XWB + XSB).
    #[value(name = "DDR")]
    Ddr,
    /// Pre-DDR-World legacy (SSQ + XWB, WAVM, or WAV). Input only.
    #[value(name = "DDR_LEGACY")]
    DdrLegacy,
    /// StepMania 5 (SSC/SM + OGG).
    #[value(name = "SM5")]
    Sm5,
}

impl Format {
    /// Chart file extensions accepted for this format as input.
    pub fn chart_extensions(self) -> &'static [&'static str] {
        match self {
            Self::Ddr | Self::DdrLegacy => &["ssq"],
            Self::Sm5 => &["ssc", "sm"],
        }
    }

    /// Audio file extensions accepted for this format as input.
    pub fn audio_extensions(self) -> &'static [&'static str] {
        match self {
            Self::Ddr => &["xwb"],
            Self::DdrLegacy => &["xwb", "wavm", "wav"],
            Self::Sm5 => &["ogg"],
        }
    }
}

/// What `--auto-sync` does with its measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AutoSyncMode {
    /// Measure how far the chart is from the audio and correct it.
    Apply,
    /// Measure and log the correction only; outputs keep the source sync.
    Report,
}

/// Auto-sync settings for one job (`--auto-sync`, `--auto-sync-max-ms`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoSync {
    pub mode: AutoSyncMode,
    /// Largest correction that may be applied, in whole ms.
    pub max_correction_ms: u32,
}

/// One conversion job: a single chart+audio pair with direction.
#[derive(Debug, Clone)]
pub struct Job {
    pub from: Format,
    pub to: Format,
    pub chart_in: PathBuf,
    pub audio_in: PathBuf,
    pub overwrite: bool,
    /// Directory where output files are written.
    pub output_dir: PathBuf,
    /// Move the whole chart this many milliseconds later relative to
    /// the audio (`--sync-offset-ms`). Applied on every conversion,
    /// after modernization, to every tempo anchor (SSQ output) or to
    /// `#OFFSET` (SSC output).
    pub sync_offset_ms: i32,
    /// Explicit DDR song code (`--song-code`). When set it names the
    /// output files and the XACT wave bank / cues; otherwise the chart's
    /// basename is used when it is a valid code, and a code is derived
    /// from it when not. Only meaningful for `DDR` output.
    pub song_code: Option<String>,
    /// Appended to a song code the tool has to *derive* because the
    /// chart's basename is not a valid code (`--suffix`). Never applied
    /// to `song_code` or to a basename that is already valid. Only
    /// meaningful for `DDR` output.
    pub suffix: Option<String>,
    /// Measure and correct the chart's sync against its audio
    /// (`--auto-sync`). `None` unless the flag was given.
    pub auto_sync: Option<AutoSync>,
}
