//! Binary entry point.
//!
//! Parses arguments, initializes logging, runs the conversion, and
//! translates the outcome into a process exit code.

use std::process::ExitCode;

use clap::Parser;

use ddr_chart_tools::cli::Cli;
use ddr_chart_tools::job;
use ddr_chart_tools::job::batch;
use ddr_chart_tools::util::logging;

/// Exit codes per US-5: 0 = all ok, 1 = per-file failure, 2 = CLI error.
const EXIT_OK: u8 = 0;
const EXIT_FILE_ERROR: u8 = 1;
const EXIT_CLI_ERROR: u8 = 2;

fn main() -> ExitCode {
    let cli = Cli::parse();

    let verbosity = cli.verbose;
    let quiet = cli.quiet;
    init_logging(verbosity, quiet);

    if let Err(e) = cli.validate() {
        eprintln!("error: {e}");
        return ExitCode::from(EXIT_CLI_ERROR);
    }

    let plan = match cli.into_plan() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(EXIT_CLI_ERROR);
        }
    };

    match plan.pairing {
        None => {
            // Single-file mode: fail immediately on error.
            match plan.jobs.first().map(job::run_one) {
                Some(Ok(())) => ExitCode::from(EXIT_OK),
                Some(Err(e)) => {
                    eprintln!("error: {e}");
                    ExitCode::from(EXIT_FILE_ERROR)
                }
                None => {
                    eprintln!("error: nothing to convert");
                    ExitCode::from(EXIT_CLI_ERROR)
                }
            }
        }
        Some(pairing) => {
            // Batch mode: continue past per-file errors and report what
            // the directory scan could not pair.
            let summary = batch::run_batch(&plan.jobs, Some(&pairing));
            if summary.failed > 0 {
                ExitCode::from(EXIT_FILE_ERROR)
            } else {
                ExitCode::from(EXIT_OK)
            }
        }
    }
}

fn init_logging(verbosity: u8, quiet: bool) {
    if quiet {
        // Suppress info, keep warn + error.
        logging::init(0);
        log::set_max_level(log::LevelFilter::Warn);
    } else {
        logging::init(verbosity);
    }
}
