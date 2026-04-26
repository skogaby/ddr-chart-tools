//! Basename pairing for batch-mode file discovery.
//!
//! Scans a flat directory for chart+audio pairs matching a source
//! format's expected extensions. Handles ambiguity (multiple audio
//! files for one basename) and unpaired files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::cli::job::Format;

/// Result of pairing files in a directory.
#[derive(Debug)]
pub struct PairResult {
    /// Successfully paired (chart_path, audio_path).
    pub pairs: Vec<(PathBuf, PathBuf)>,
    /// Basenames skipped due to ambiguous audio (basename, conflicting paths).
    pub ambiguous: Vec<(String, Vec<PathBuf>)>,
    /// Unpaired chart files (no matching audio).
    pub unpaired_charts: Vec<PathBuf>,
    /// Unpaired audio files (no matching chart).
    pub unpaired_audio: Vec<PathBuf>,
}

/// Scan `dir` for chart+audio pairs matching `format`.
///
/// Only examines the top level of `dir` (no recursion).
pub fn find_pairs(dir: &Path, format: Format) -> std::io::Result<PairResult> {
    let chart_exts = format.chart_extensions();
    let audio_exts = format.audio_extensions();

    let mut charts: HashMap<String, PathBuf> = HashMap::new();
    let mut audio: HashMap<String, Vec<PathBuf>> = HashMap::new();

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());
        let ext = match ext {
            Some(e) => e,
            None => continue,
        };

        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };

        if chart_exts.iter().any(|&ce| ce == ext) {
            // Ultramix chart filenames use `{id}_all.ssq` while their
            // audio siblings are `{id}.wavm`. Strip the `_all` suffix
            // from chart stems before pairing so they match.
            let stem = if matches!(format, Format::DdrLegacy) {
                stem.strip_suffix("_all").map(str::to_string).unwrap_or(stem)
            } else {
                stem
            };
            charts.entry(stem).or_insert(path);
        } else if audio_exts.iter().any(|&ae| ae == ext) {
            audio.entry(stem).or_default().push(path);
        }
    }

    let mut result = PairResult {
        pairs: Vec::new(),
        ambiguous: Vec::new(),
        unpaired_charts: Vec::new(),
        unpaired_audio: Vec::new(),
    };

    // Match charts to audio.
    for (stem, chart_path) in &charts {
        match audio.remove(stem) {
            Some(audio_paths) if audio_paths.len() == 1 => {
                result
                    .pairs
                    .push((chart_path.clone(), audio_paths.into_iter().next().unwrap()));
            }
            Some(audio_paths) => {
                result.ambiguous.push((stem.clone(), audio_paths));
            }
            None => {
                result.unpaired_charts.push(chart_path.clone());
            }
        }
    }

    // Remaining audio with no matching chart.
    for (_stem, paths) in audio {
        result.unpaired_audio.extend(paths);
    }

    result.pairs.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_dir(files: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for name in files {
            fs::write(dir.path().join(name), b"").unwrap();
        }
        dir
    }

    #[test]
    fn pairs_ddr_ssq_xwb() {
        let dir = setup_dir(&["foo.ssq", "foo.xwb", "bar.ssq", "bar.xwb"]);
        let result = find_pairs(dir.path(), Format::Ddr).unwrap();
        assert_eq!(result.pairs.len(), 2);
        assert!(result.unpaired_charts.is_empty());
        assert!(result.unpaired_audio.is_empty());
    }

    #[test]
    fn unpaired_chart_reported() {
        let dir = setup_dir(&["foo.ssq"]);
        let result = find_pairs(dir.path(), Format::Ddr).unwrap();
        assert!(result.pairs.is_empty());
        assert_eq!(result.unpaired_charts.len(), 1);
    }

    #[test]
    fn unpaired_audio_reported() {
        let dir = setup_dir(&["foo.xwb"]);
        let result = find_pairs(dir.path(), Format::Ddr).unwrap();
        assert!(result.pairs.is_empty());
        assert_eq!(result.unpaired_audio.len(), 1);
    }

    #[test]
    fn ambiguous_audio_in_legacy_mode() {
        let dir = setup_dir(&["foo.ssq", "foo.xwb", "foo.wavm"]);
        let result = find_pairs(dir.path(), Format::DdrLegacy).unwrap();
        assert!(result.pairs.is_empty());
        assert_eq!(result.ambiguous.len(), 1);
        assert_eq!(result.ambiguous[0].1.len(), 2);
    }

    #[test]
    fn ignores_wrong_extensions() {
        let dir = setup_dir(&["foo.ssq", "foo.xwb", "readme.txt", "notes.md"]);
        let result = find_pairs(dir.path(), Format::Ddr).unwrap();
        assert_eq!(result.pairs.len(), 1);
    }

    #[test]
    fn sm5_pairs_ssc_and_sm_with_ogg() {
        let dir = setup_dir(&["a.ssc", "a.ogg", "b.sm", "b.ogg"]);
        let result = find_pairs(dir.path(), Format::Sm5).unwrap();
        assert_eq!(result.pairs.len(), 2);
    }

    #[test]
    fn ignores_subdirectories() {
        let dir = setup_dir(&["foo.ssq", "foo.xwb"]);
        fs::create_dir(dir.path().join("subdir")).unwrap();
        let result = find_pairs(dir.path(), Format::Ddr).unwrap();
        assert_eq!(result.pairs.len(), 1);
    }

    #[test]
    fn legacy_pairs_ultramix_all_suffix_with_wavm() {
        // Ultramix: abs2_all.ssq pairs with abs2.wavm.
        let dir = setup_dir(&["abs2_all.ssq", "abs2.wavm"]);
        let result = find_pairs(dir.path(), Format::DdrLegacy).unwrap();
        assert_eq!(result.pairs.len(), 1);
        assert!(result.unpaired_charts.is_empty());
        assert!(result.unpaired_audio.is_empty());
    }

    #[test]
    fn legacy_pairs_plain_stems_unchanged() {
        // Non-Ultramix legacy pair still works with matching stems.
        let dir = setup_dir(&["foo.ssq", "foo.wavm"]);
        let result = find_pairs(dir.path(), Format::DdrLegacy).unwrap();
        assert_eq!(result.pairs.len(), 1);
    }
}
