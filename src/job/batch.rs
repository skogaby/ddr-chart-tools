//! Batch runner with per-file error recovery.

use log::{error, info, warn};

use crate::cli::job::Job;
use crate::util::pair::PairResult;

/// Outcome counts for a batch run.
#[derive(Debug, Default)]
pub struct BatchSummary {
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
}

/// Run a list of jobs, continuing past per-file errors.
///
/// Unpaired/ambiguous files from `pair_result` (if provided) are
/// counted as skipped and logged.
pub fn run_batch(jobs: &[Job], pair_result: Option<&PairResult>) -> BatchSummary {
    let mut summary = BatchSummary::default();

    // Log skipped files from pairing.
    if let Some(pr) = pair_result {
        for path in &pr.unpaired_charts {
            warn!("skipped (no matching audio): {}", path.display());
            summary.skipped += 1;
        }
        for path in &pr.unpaired_audio {
            warn!("skipped (no matching chart): {}", path.display());
            summary.skipped += 1;
        }
    }

    for job in jobs {
        summary.attempted += 1;
        match super::run_one(job) {
            Ok(()) => {
                summary.succeeded += 1;
            }
            Err(e) => {
                error!("{}: {e}", job.chart_in.display());
                summary.failed += 1;
            }
        }
    }

    info!(
        "batch complete: {} attempted, {} succeeded, {} failed, {} skipped",
        summary.attempted, summary.succeeded, summary.failed, summary.skipped
    );

    summary
}
