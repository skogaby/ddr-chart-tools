//! Logging subscriber setup.

use log::LevelFilter;

/// Initialize the process-wide logger.
///
/// Verbosity count maps to level:
///
/// | `verbosity` | level |
/// |-------------|-------|
/// | `0`         | `Info` |
/// | `1`         | `Debug` |
/// | `>= 2`      | `Trace` |
///
/// `warn!` and `error!` are always enabled. The `RUST_LOG` environment
/// variable, when set, overrides the verbosity-derived level.
pub fn init(verbosity: u8) {
    env_logger::Builder::from_default_env()
        .filter_level(level_for(verbosity))
        .parse_default_env()
        .init();
}

fn level_for(verbosity: u8) -> LevelFilter {
    match verbosity {
        0 => LevelFilter::Info,
        1 => LevelFilter::Debug,
        _ => LevelFilter::Trace,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbosity_0_is_info() {
        assert_eq!(level_for(0), LevelFilter::Info);
    }

    #[test]
    fn verbosity_1_is_debug() {
        assert_eq!(level_for(1), LevelFilter::Debug);
    }

    #[test]
    fn verbosity_2_is_trace() {
        assert_eq!(level_for(2), LevelFilter::Trace);
    }

    #[test]
    fn verbosity_above_2_saturates_at_trace() {
        assert_eq!(level_for(100), LevelFilter::Trace);
    }
}
