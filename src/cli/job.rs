//! Job and Format types consumed by the conversion pipeline.

use std::path::PathBuf;

use clap::ValueEnum;

/// Supported format families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Modern DDR (SSQ + XWB + XSB).
    #[value(name = "DDR")]
    Ddr,
    /// Pre-DDR-World legacy (SSQ + XWB or WAVM). Input only.
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
            Self::DdrLegacy => &["xwb", "wavm"],
            Self::Sm5 => &["ogg"],
        }
    }
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
}
