//! THROWAWAY: compare the ±70 ms estimate with a ±210 ms search on legacy
//! charts, and show each chart's raw tempo_data[0] (zeroed by modernize).
use ddr_chart_tools::sync::{self, Outcome, Params, TimeMap};
use ddr_chart_tools::{ssq, ssq_legacy, wavm};
use std::fs;
fn main() {
    let dir = std::env::args().nth(1).expect("folder");
    let mut names: Vec<_> = fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).collect();
    names.sort();
    for p in names {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let Some(id) = name.strip_suffix("_all.ssq") else { continue };
        let Ok(w) = fs::read(p.with_file_name(format!("{id}.wavm"))) else { continue };
        let mut parsed = ssq::parse(&fs::read(&p).unwrap()).unwrap();
        let (tps, td0) = (parsed.song.tps, parsed.raw_tempo_pairs[0].1);
        ssq_legacy::modernize::modernize(&mut parsed);
        let audio = wavm::parse(&w).unwrap();
        let map = TimeMap::from_tempo_pairs(&parsed.raw_tempo_pairs, 1000).unwrap();
        let ev = sync::chart_events(&parsed.song.charts, &map);
        let n = sync::estimate(&audio, &ev, &Params::default());
        let wd = sync::estimate(&audio, &ev, &Params { max_correction_ms: 200 });
        let o = |o: Outcome| match o { Outcome::Refused(r) => r.key().to_string(), Outcome::Apply{..} => "apply".into(), Outcome::Unchanged => "unchanged".into() };
        let bpm = parsed.song.tempo_segments.iter().map(|s| s.bpm.as_rational().as_f64()).fold(0.0, f64::max);
        println!("{id:6} bpm {bpm:6.1} raw td0 {:+6.1} ms | ±70: {:+7.1} {:16} | ±210: {:+7.1} {:16} rival {:.2}", td0 as f64 * 1000.0 / tps as f64,
            n.correction_ms.unwrap_or(f64::NAN), o(n.outcome), wd.correction_ms.unwrap_or(f64::NAN), o(wd.outcome), wd.rival.unwrap_or(f64::NAN));
    }
}
