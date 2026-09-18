//! CLI argument parsing, validation, and job planning.

pub mod job;

use std::path::PathBuf;

use clap::Parser;
use thiserror::Error;

use crate::util::pair;
use job::{Format, Job};

#[derive(Debug, Error)]
pub enum CliError {
    #[error("--to-format DDR_LEGACY is not supported (legacy output cannot be authored)")]
    LegacyOutputForbidden,

    #[error("unsupported conversion: {from:?} -> {to:?}")]
    UnsupportedConversion { from: Format, to: Format },

    #[error("--chartfile and --audiofile must be specified together")]
    MissingFilePair,

    #[error("batch pairing error for {basename}: {reason}")]
    PairAmbiguity { basename: String, reason: String },

    #[error("no eligible file pairs found in {dir}")]
    NoPairs { dir: PathBuf },

    #[error("--song-code only applies to --to-format DDR (it names the XACT wave bank and cues)")]
    SongCodeRequiresDdrOutput,

    #[error(
        "--song-code needs --chartfile; in batch mode name each input pair after its song code"
    )]
    SongCodeRequiresSingleFile,

    #[error("--song-code must be 1-16 ASCII letters or digits, got {code:?}")]
    BadSongCode { code: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Convert song and chart assets between DDR arcade and StepMania 5 formats.
#[derive(Debug, Parser)]
#[command(name = "ddr-chart-tools", version, about)]
pub struct Cli {
    /// Source format.
    #[arg(long, value_enum)]
    pub from_format: Format,

    /// Target format.
    #[arg(long, value_enum)]
    pub to_format: Format,

    /// Path to a single chart file (requires --audiofile).
    #[arg(long, group = "input_mode")]
    pub chartfile: Option<PathBuf>,

    /// Path to a single audio file (requires --chartfile).
    #[arg(long, requires = "chartfile")]
    pub audiofile: Option<PathBuf>,

    /// Directory of file pairs to convert in batch.
    #[arg(long, group = "input_mode")]
    pub input_folder: Option<PathBuf>,

    /// Directory to write converted files into. Defaults to `./output`
    /// in single-file mode and `<input-folder>/output` in batch mode.
    #[arg(long)]
    pub output_dir: Option<PathBuf>,

    /// Overwrite existing output files.
    #[arg(long, default_value_t = false)]
    pub overwrite: bool,

    /// Increase log verbosity (-v = debug, -vv = trace).
    #[arg(short, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Suppress info-level output (keeps warn and error).
    #[arg(short, long, default_value_t = false)]
    pub quiet: bool,

    /// Add this many milliseconds to the audio-sync offset. Positive
    /// values delay the chart relative to the audio: beat 0 lands N ms
    /// later in the audio (adds N to `tempo_data[0]` in SSQ, subtracts
    /// N/1000 from `#OFFSET` in SSC). Use to correct for consistent
    /// per-platform sync bias (e.g. Ultramix charts on DDR World
    /// commonly need ~+53ms).
    #[arg(long, allow_hyphen_values = true)]
    pub sync_offset_ms: Option<i32>,

    /// DDR song code for the converted song (e.g. `muka`): names the
    /// output `.ssq`/`.xwb`/`.xsb` and the wave bank and cues inside
    /// them. The game plays the cue whose name equals the song's code,
    /// compared byte-for-byte, so this must match the ID the song is
    /// installed under. 1–16 ASCII letters/digits. Single-file mode with
    /// `--to-format DDR` only; in batch mode name each input pair after
    /// its code instead.
    #[arg(long)]
    pub song_code: Option<String>,
}

impl Cli {
    /// Validate semantic rules that clap's derive can't express.
    pub fn validate(&self) -> Result<(), CliError> {
        // DDR_LEGACY output is forbidden.
        if self.to_format == Format::DdrLegacy {
            return Err(CliError::LegacyOutputForbidden);
        }

        // Must have exactly one input mode.
        if self.chartfile.is_none() && self.input_folder.is_none() {
            return Err(CliError::MissingFilePair);
        }

        // --chartfile requires --audiofile.
        if self.chartfile.is_some() && self.audiofile.is_none() {
            return Err(CliError::MissingFilePair);
        }

        // Check supported (from, to) combinations.
        let valid = matches!(
            (self.from_format, self.to_format),
            (Format::Ddr, Format::Sm5)
                | (Format::Sm5, Format::Ddr)
                | (Format::DdrLegacy, Format::Ddr)
                | (Format::DdrLegacy, Format::Sm5)
        );
        if !valid {
            return Err(CliError::UnsupportedConversion {
                from: self.from_format,
                to: self.to_format,
            });
        }

        if let Some(code) = &self.song_code {
            if self.chartfile.is_none() {
                return Err(CliError::SongCodeRequiresSingleFile);
            }
            if self.to_format != Format::Ddr {
                return Err(CliError::SongCodeRequiresDdrOutput);
            }
            if !crate::xsb::is_valid_code(code) {
                return Err(CliError::BadSongCode { code: code.clone() });
            }
        }

        Ok(())
    }

    /// Convert validated CLI args into a list of conversion jobs.
    pub fn into_jobs(self) -> Result<Vec<Job>, CliError> {
        self.into_plan().map(|plan| plan.jobs)
    }

    /// Convert validated CLI args into a [`Plan`]: the jobs to run plus,
    /// in batch mode, the pairing result so the runner can report files
    /// that were skipped for lack of a partner.
    pub fn into_plan(self) -> Result<Plan, CliError> {
        let sync_offset_ms = self.sync_offset_ms.unwrap_or(0);

        if let (Some(chart), Some(audio)) = (self.chartfile, self.audiofile) {
            let output_dir = self.output_dir.unwrap_or_else(|| PathBuf::from("output"));
            return Ok(Plan {
                jobs: vec![Job {
                    from: self.from_format,
                    to: self.to_format,
                    chart_in: chart,
                    audio_in: audio,
                    overwrite: self.overwrite,
                    output_dir,
                    sync_offset_ms,
                    song_code: self.song_code,
                }],
                pairing: None,
            });
        }

        // Batch mode.
        let dir = self.input_folder.as_ref().unwrap();
        let output_dir = self.output_dir.unwrap_or_else(|| dir.join("output"));
        let result = pair::find_pairs(dir, self.from_format)?;

        // Ambiguous files are hard errors per US-5.
        if let Some((basename, paths)) = result.ambiguous.first() {
            let names: Vec<_> = paths.iter().filter_map(|p| p.file_name()).collect();
            return Err(CliError::PairAmbiguity {
                basename: basename.clone(),
                reason: format!("multiple audio files: {names:?}"),
            });
        }

        if result.pairs.is_empty() {
            return Err(CliError::NoPairs { dir: dir.clone() });
        }

        let jobs = result
            .pairs
            .iter()
            .map(|(chart, audio)| Job {
                from: self.from_format,
                to: self.to_format,
                chart_in: chart.clone(),
                audio_in: audio.clone(),
                overwrite: self.overwrite,
                output_dir: output_dir.clone(),
                sync_offset_ms,
                song_code: None,
            })
            .collect();

        Ok(Plan {
            jobs,
            pairing: Some(result),
        })
    }
}

/// What a run will do: the jobs, and in batch mode the directory
/// pairing they came from (so unpaired files can be reported).
#[derive(Debug)]
pub struct Plan {
    pub jobs: Vec<Job>,
    /// `Some` in batch mode, `None` for a single `--chartfile` pair.
    pub pairing: Option<pair::PairResult>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("ddr-chart-tools").chain(args.iter().copied()))
    }

    #[test]
    fn single_file_mode_parses() {
        let c = cli(&[
            "--from-format",
            "DDR",
            "--to-format",
            "SM5",
            "--chartfile",
            "song.ssq",
            "--audiofile",
            "song.xwb",
        ])
        .unwrap();
        assert_eq!(c.from_format, Format::Ddr);
        assert_eq!(c.to_format, Format::Sm5);
        c.validate().unwrap();
    }

    #[test]
    fn batch_mode_parses() {
        let c = cli(&[
            "--from-format",
            "DDR",
            "--to-format",
            "SM5",
            "--input-folder",
            "/tmp/songs",
        ])
        .unwrap();
        assert!(c.chartfile.is_none());
        assert!(c.input_folder.is_some());
        c.validate().unwrap();
    }

    #[test]
    fn rejects_legacy_output() {
        let c = cli(&[
            "--from-format",
            "DDR",
            "--to-format",
            "DDR_LEGACY",
            "--chartfile",
            "x.ssq",
            "--audiofile",
            "x.xwb",
        ])
        .unwrap();
        assert!(matches!(c.validate(), Err(CliError::LegacyOutputForbidden)));
    }

    #[test]
    fn rejects_same_format() {
        let c = cli(&[
            "--from-format",
            "DDR",
            "--to-format",
            "DDR",
            "--chartfile",
            "x.ssq",
            "--audiofile",
            "x.xwb",
        ])
        .unwrap();
        assert!(matches!(
            c.validate(),
            Err(CliError::UnsupportedConversion { .. })
        ));
    }

    #[test]
    fn rejects_chartfile_without_audiofile() {
        let c = cli(&[
            "--from-format",
            "DDR",
            "--to-format",
            "SM5",
            "--chartfile",
            "x.ssq",
        ])
        .unwrap();
        assert!(matches!(c.validate(), Err(CliError::MissingFilePair)));
    }

    #[test]
    fn rejects_no_input_mode() {
        let c = cli(&["--from-format", "DDR", "--to-format", "SM5"]).unwrap();
        assert!(matches!(c.validate(), Err(CliError::MissingFilePair)));
    }

    #[test]
    fn single_file_into_jobs() {
        let c = cli(&[
            "--from-format",
            "SM5",
            "--to-format",
            "DDR",
            "--chartfile",
            "song.ssc",
            "--audiofile",
            "song.ogg",
        ])
        .unwrap();
        c.validate().unwrap();
        let jobs = c.into_jobs().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].from, Format::Sm5);
        assert_eq!(jobs[0].to, Format::Ddr);
    }

    #[test]
    fn verbose_flag_counts() {
        let c = cli(&[
            "--from-format",
            "DDR",
            "--to-format",
            "SM5",
            "--chartfile",
            "x.ssq",
            "--audiofile",
            "x.xwb",
            "-vv",
        ])
        .unwrap();
        assert_eq!(c.verbose, 2);
    }

    #[test]
    fn overwrite_flag() {
        let c = cli(&[
            "--from-format",
            "DDR",
            "--to-format",
            "SM5",
            "--chartfile",
            "x.ssq",
            "--audiofile",
            "x.xwb",
            "--overwrite",
        ])
        .unwrap();
        assert!(c.overwrite);
    }

    #[test]
    fn batch_into_jobs_with_real_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.ssq"), b"").unwrap();
        std::fs::write(dir.path().join("a.xwb"), b"").unwrap();
        std::fs::write(dir.path().join("b.ssq"), b"").unwrap();
        std::fs::write(dir.path().join("b.xwb"), b"").unwrap();

        let c = cli(&[
            "--from-format",
            "DDR",
            "--to-format",
            "SM5",
            "--input-folder",
            dir.path().to_str().unwrap(),
        ])
        .unwrap();
        c.validate().unwrap();
        let jobs = c.into_jobs().unwrap();
        assert_eq!(jobs.len(), 2);
        assert!(jobs.iter().all(|j| j.song_code.is_none()));
    }

    #[test]
    fn song_code_flows_into_single_file_job() {
        let c = cli(&[
            "--from-format",
            "SM5",
            "--to-format",
            "DDR",
            "--chartfile",
            "Mukade.ssc",
            "--audiofile",
            "Mukade.ogg",
            "--song-code",
            "muka",
        ])
        .unwrap();
        c.validate().unwrap();
        let jobs = c.into_jobs().unwrap();
        assert_eq!(jobs[0].song_code.as_deref(), Some("muka"));
    }

    #[test]
    fn song_code_rejected_for_sm5_output() {
        let c = cli(&[
            "--from-format",
            "DDR",
            "--to-format",
            "SM5",
            "--chartfile",
            "x.ssq",
            "--audiofile",
            "x.xwb",
            "--song-code",
            "muka",
        ])
        .unwrap();
        assert!(matches!(
            c.validate(),
            Err(CliError::SongCodeRequiresDdrOutput)
        ));
    }

    #[test]
    fn song_code_must_be_a_valid_cue_name() {
        for bad in ["", "mu ka", "muka!", "abcdefghijklmnopq"] {
            let c = cli(&[
                "--from-format",
                "SM5",
                "--to-format",
                "DDR",
                "--chartfile",
                "x.ssc",
                "--audiofile",
                "x.ogg",
                "--song-code",
                bad,
            ])
            .unwrap();
            assert!(
                matches!(c.validate(), Err(CliError::BadSongCode { .. })),
                "expected BadSongCode for {bad:?}"
            );
        }
    }

    #[test]
    fn song_code_requires_single_file_mode() {
        let c = cli(&[
            "--from-format",
            "SM5",
            "--to-format",
            "DDR",
            "--input-folder",
            "/tmp/songs",
            "--song-code",
            "muka",
        ])
        .unwrap();
        assert!(matches!(
            c.validate(),
            Err(CliError::SongCodeRequiresSingleFile)
        ));
    }
}
