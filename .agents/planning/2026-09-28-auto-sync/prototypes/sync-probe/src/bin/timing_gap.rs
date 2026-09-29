//! THROWAWAY: max gap between the SSQ-pair timing and the model (SSC) timing
//! of each modernized legacy chart, over its note range.
use ddr_chart_tools::sync::TimeMap;
use ddr_chart_tools::{ssq, ssq_legacy};
use std::fs;

fn main() {
    let dir = std::env::args().nth(1).expect("folder");
    let mut rows = Vec::new();
    for entry in fs::read_dir(&dir).unwrap() {
        let p = entry.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let Some(id) = name.strip_suffix("_all.ssq") else { continue };
        let mut parsed = ssq::parse(&fs::read(&p).unwrap()).unwrap();
        let tps = parsed.song.tps;
        ssq_legacy::modernize::modernize(&mut parsed);
        let a = TimeMap::from_tempo_pairs(&parsed.raw_tempo_pairs, 1000).unwrap();
        let b = TimeMap::from_song(&parsed.song).unwrap();
        let last = parsed.song.charts.iter().flat_map(|c| c.notes.iter()).map(|n| n.beat.as_rational().as_f64()).fold(0.0, f64::max);
        let mut worst = (0.0f64, 0.0);
        let mut beat = 0.0;
        while beat <= last { let g = (a.seconds_at(beat) - b.seconds_at(beat)) * 1000.0; if g.abs() > worst.0.abs() { worst = (g, beat); } beat += 0.25; }
        let zero_dt = parsed.raw_tempo_pairs.windows(2).filter(|w| w[1].0 > w[0].0 && w[1].1 == w[0].1).count();
        rows.push((id.to_string(), tps, worst.0, worst.1, zero_dt, parsed.raw_tempo_pairs.len()));
    }
    rows.sort_by(|x, y| y.2.abs().total_cmp(&x.2.abs()));
    for r in rows.iter().take(12) { println!("{:6} tps={:4} max gap {:+8.2} ms at beat {:6.1}  instant-advance pairs {}  pairs {}", r.0, r.1, r.2, r.3, r.4, r.5); }
    println!("songs with gap > 1 ms: {}/{}", rows.iter().filter(|r| r.2.abs() > 1.0).count(), rows.len());
}
