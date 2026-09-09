//! `ddr-se-bank` — generate and inspect XACT sound-effect bank pairs.
//!
//! A second, standalone binary alongside the main `ddr-chart-tools` converter.
//! It exists so that a build script can produce the wave-bank + sound-bank pair
//! a mod registers with DDR World's XACT 2 engine, and then assert the result
//! against the engine's container rules without reimplementing a parser.
//!
//! ```text
//! ddr-se-bank generate --input clap.ogg --name asti --out-dir banks/
//! ddr-se-bank dump banks/asti.xwb
//! ```
//!
//! `dump`'s output format is a stable `key=value` interface — see
//! [`ddr_chart_tools::xwb::dump`].

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use ddr_chart_tools::job::se_bank;
use ddr_chart_tools::util::logging;
use ddr_chart_tools::xwb;

/// Exit codes, matching the main binary: 0 = ok, 1 = failure, 2 = CLI error.
const EXIT_OK: u8 = 0;
const EXIT_FILE_ERROR: u8 = 1;

/// Generate and inspect XACT sound-effect bank pairs for DDR World.
#[derive(Debug, Parser)]
#[command(name = "ddr-se-bank", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Increase log verbosity (-v = debug, -vv = trace).
    #[arg(short, action = clap::ArgAction::Count, global = true)]
    verbose: u8,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Generate a wave-bank + sound-bank pair from a mono 44100 Hz Ogg Vorbis file.
    Generate {
        /// Input audio file. Must decode to mono 44100 Hz; it is neither
        /// resampled nor downmixed.
        #[arg(long)]
        input: PathBuf,

        /// Bank name, and the name of the single cue. 1-16 ASCII alphanumeric
        /// characters. Case is significant to the engine.
        #[arg(long)]
        name: String,

        /// Directory to write `<name>.xwb` and `<name>.xsb` into. Created if absent.
        #[arg(long)]
        out_dir: PathBuf,
    },

    /// Print a wave bank's metadata as stable, greppable `key=value` text.
    Dump {
        /// Path to an `.xwb` wave bank. Works on the game's own banks too.
        bank: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    logging::init(cli.verbose);

    let result = match cli.command {
        Command::Generate {
            input,
            name,
            out_dir,
        } => se_bank::generate(&input, &name, &out_dir).map(|out| {
            println!("{}", out.xwb_path.display());
            println!("{}", out.xsb_path.display());
        }),
        Command::Dump { bank } => match std::fs::read(&bank) {
            Ok(bytes) => xwb::dump::describe(&bytes)
                .map(|text| print!("{text}"))
                .map_err(Into::into),
            Err(e) => Err(e.into()),
        },
    };

    match result {
        Ok(()) => ExitCode::from(EXIT_OK),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(EXIT_FILE_ERROR)
        }
    }
}
