//! CLI argument parsing, validation, and job planning.

pub mod job;

use std::path::PathBuf;

use clap::Parser;
use thiserror::Error;

use crate::util::pair;
use job::{AutoSync, AutoSyncMode, Format, Job};

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

    #[error("--auto-sync-max-ms only applies together with --auto-sync")]
    AutoSyncMaxRequiresAutoSync,

    #[error(
        "--auto-sync-max-ms must be 1-{} ms, got {ms}",
        crate::sync::MAX_CORRECTION_LIMIT_MS
    )]
    AutoSyncMaxOutOfRange { ms: u32 },

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

    /// Move the whole chart this many milliseconds later relative to
    /// the audio (negative = earlier), on any conversion. Every tempo
    /// anchor moves by N ms in SSQ output; `#OFFSET` decreases by
    /// N/1000 s in SSC output. BPMs and stops are unchanged. Use to
    /// correct a consistent per-platform sync bias.
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

    /// Measure how far the chart is from its audio and move the chart to
    /// match (`apply`, the default when the flag is given alone), or only
    /// log the correction it would make (`report`). Corrections are
    /// bounded by --auto-sync-max-ms; untrustworthy measurements leave
    /// the sync unchanged with a warning. Applied before
    /// --sync-offset-ms.
    #[arg(
        long,
        value_enum,
        num_args = 0..=1,
        default_missing_value = "apply",
        value_name = "MODE"
    )]
    pub auto_sync: Option<AutoSyncMode>,

    /// Largest correction --auto-sync may apply, in milliseconds
    /// (default 60, maximum 200). Wider caps let half-beat misalignments
    /// into the search.
    #[arg(long, value_name = "MS")]
    pub auto_sync_max_ms: Option<u32>,
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

        if let Some(ms) = self.auto_sync_max_ms {
            if self.auto_sync.is_none() {
                return Err(CliError::AutoSyncMaxRequiresAutoSync);
            }
            if ms == 0 || ms > crate::sync::MAX_CORRECTION_LIMIT_MS {
                return Err(CliError::AutoSyncMaxOutOfRange { ms });
            }
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
        let auto_sync = self.auto_sync.map(|mode| AutoSync {
            mode,
            max_correction_ms: self
                .auto_sync_max_ms
                .unwrap_or(crate::sync::DEFAULT_MAX_CORRECTION_MS),
        });

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
                    auto_sync,
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
                auto_sync,
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
    use job::{AutoSync, AutoSyncMode};

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
    fn sync_offset_reaches_sm5_and_ddr_jobs() {
        // Both directions used to ignore the bias; lock down that it is
        // threaded into their jobs now that it is honored.
        for (from, to, chart, audio) in [
            ("SM5", "DDR", "song.ssc", "song.ogg"),
            ("DDR", "SM5", "song.ssq", "song.xwb"),
        ] {
            for bias in ["12", "-12"] {
                let c = cli(&[
                    "--from-format",
                    from,
                    "--to-format",
                    to,
                    "--chartfile",
                    chart,
                    "--audiofile",
                    audio,
                    "--sync-offset-ms",
                    bias,
                ])
                .unwrap();
                c.validate().unwrap();
                let jobs = c.into_jobs().unwrap();
                assert_eq!(
                    jobs[0].sync_offset_ms.to_string(),
                    bias,
                    "{from} -> {to} with --sync-offset-ms {bias}"
                );
            }
        }
    }

    fn auto_sync_cli(extra: &[&str]) -> Result<Cli, clap::Error> {
        let mut args = vec![
            "--from-format",
            "SM5",
            "--to-format",
            "DDR",
            "--chartfile",
            "song.ssc",
            "--audiofile",
            "song.ogg",
        ];
        args.extend_from_slice(extra);
        cli(&args)
    }

    #[test]
    fn auto_sync_absent_by_default() {
        let c = auto_sync_cli(&[]).unwrap();
        c.validate().unwrap();
        assert!(c.into_jobs().unwrap()[0].auto_sync.is_none());
    }

    #[test]
    fn auto_sync_bare_flag_applies_with_default_cap() {
        let c = auto_sync_cli(&["--auto-sync"]).unwrap();
        c.validate().unwrap();
        let jobs = c.into_jobs().unwrap();
        assert_eq!(
            jobs[0].auto_sync,
            Some(AutoSync {
                mode: AutoSyncMode::Apply,
                max_correction_ms: crate::sync::DEFAULT_MAX_CORRECTION_MS,
            })
        );
    }

    #[test]
    fn auto_sync_bare_flag_before_another_flag() {
        let c = cli(&[
            "--from-format",
            "SM5",
            "--to-format",
            "DDR",
            "--auto-sync",
            "--chartfile",
            "song.ssc",
            "--audiofile",
            "song.ogg",
        ])
        .unwrap();
        assert_eq!(c.auto_sync, Some(AutoSyncMode::Apply));
    }

    #[test]
    fn auto_sync_report_mode() {
        let c = auto_sync_cli(&["--auto-sync", "report"]).unwrap();
        c.validate().unwrap();
        let jobs = c.into_jobs().unwrap();
        assert_eq!(
            jobs[0].auto_sync.map(|a| a.mode),
            Some(AutoSyncMode::Report)
        );
    }

    #[test]
    fn auto_sync_max_ms_sets_cap() {
        let c = auto_sync_cli(&["--auto-sync", "--auto-sync-max-ms", "90"]).unwrap();
        c.validate().unwrap();
        let jobs = c.into_jobs().unwrap();
        assert_eq!(jobs[0].auto_sync.map(|a| a.max_correction_ms), Some(90));
    }

    #[test]
    fn auto_sync_max_requires_auto_sync() {
        let c = auto_sync_cli(&["--auto-sync-max-ms", "90"]).unwrap();
        assert!(matches!(
            c.validate(),
            Err(CliError::AutoSyncMaxRequiresAutoSync)
        ));
    }

    #[test]
    fn auto_sync_max_out_of_range() {
        for ms in ["0", "201"] {
            let c = auto_sync_cli(&["--auto-sync", "--auto-sync-max-ms", ms]).unwrap();
            assert!(
                matches!(c.validate(), Err(CliError::AutoSyncMaxOutOfRange { .. })),
                "{ms} should be rejected"
            );
        }
    }

    #[test]
    fn auto_sync_accepts_legacy_input() {
        let c = cli(&[
            "--from-format",
            "DDR_LEGACY",
            "--to-format",
            "DDR",
            "--chartfile",
            "x_all.ssq",
            "--audiofile",
            "x.wavm",
            "--auto-sync",
        ])
        .unwrap();
        c.validate().unwrap();
        assert!(c.into_jobs().unwrap()[0].auto_sync.is_some());
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
