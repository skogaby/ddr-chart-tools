//! THROWAWAY: relate each legacy chart's raw tempo origin to its auto-sync correction.
use ddr_chart_tools::sync::{self, Outcome, Params, TimeMap};
use ddr_chart_tools::{ssq, ssq_legacy, wavm};
use std::fs;
fn main() {
    let dir = std::env::args().nth(1).expect("folder");
    let mut names: Vec<_> = fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).collect();
    names.sort();
    let (mut small, mut big) = (Vec::new(), Vec::new());
    for p in names {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let Some(id) = name.strip_suffix("_all.ssq") else { continue };
        let Ok(w) = fs::read(p.with_file_name(format!("{id}.wavm"))) else { continue };
        let mut parsed = ssq::parse(&fs::read(&p).unwrap()).unwrap();
        let (tps, (to0, td0)) = (parsed.song.tps, parsed.raw_tempo_pairs[0]);
        let second = parsed.raw_tempo_pairs.get(1).copied();
        let td0_ms = td0 as f64 * 1000.0 / tps as f64;
        ssq_legacy::modernize::modernize(&mut parsed);
        let audio = wavm::parse(&w).unwrap();
        let map = TimeMap::from_tempo_pairs(&parsed.raw_tempo_pairs, 1000).unwrap();
        let est = sync::estimate(&audio, &sync::chart_events(&parsed.song.charts, &map), &Params::default());
        // Same chart with tempo_data[0] preserved instead of zeroed.
        let kept: Vec<(i32, i32)> = parsed.raw_tempo_pairs.iter().map(|&(t, d)| (t, d + (td0_ms.round() as i32))).collect();
        let kmap = TimeMap::from_tempo_pairs(&kept, 1000).unwrap();
        let kest = sync::estimate(&audio, &sync::chart_events(&parsed.song.charts, &kmap), &Params::default());
        let o = |e: &sync::SyncEstimate| match e.outcome { Outcome::Refused(r) => r.key(), Outcome::Apply{..} => "apply", Outcome::Unchanged => "unchanged" };
        println!("{id:6} tps {tps:4} time_offset[0] {to0:7} td0 {td0_ms:+8.1} ms second {second:?} | zeroed: {:+7.1} {:15} | kept: {:+7.1} {:15}",
            est.correction_ms.unwrap_or(f64::NAN), o(&est), kest.correction_ms.unwrap_or(f64::NAN), o(&kest));
        if let (Some(a), Some(b)) = (est.correction_ms, kest.correction_ms) {
            let accepted = |e: &sync::SyncEstimate| !matches!(e.outcome, Outcome::Refused(_));
            if td0_ms.abs() < 200.0 { small.push((a, b, accepted(&est), accepted(&kest))); } else { big.push((a, b, accepted(&est), accepted(&kest))); }
        }
    }
    for (label, v) in [("|td0|<200ms", &small), ("|td0|>=200ms", &big)] {
        let med = |mut x: Vec<f64>| { x.sort_by(f64::total_cmp); if x.is_empty() { f64::NAN } else { x[x.len()/2] } };
        let mad = |x: &Vec<f64>| { let m = med(x.clone()); 1.4826 * med(x.iter().map(|y| (y-m).abs()).collect()) };
        let z: Vec<f64> = v.iter().map(|r| r.0).collect(); let k: Vec<f64> = v.iter().map(|r| r.1).collect();
        println!("{label}: n={} | zeroed: median {:+.1} robust-std {:.1} accepted {} | kept: median {:+.1} robust-std {:.1} accepted {}",
            v.len(), med(z.clone()), mad(&z), v.iter().filter(|r| r.2).count(), med(k.clone()), mad(&k), v.iter().filter(|r| r.3).count());
    }
}
