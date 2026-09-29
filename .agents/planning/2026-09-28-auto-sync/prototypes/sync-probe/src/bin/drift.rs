//! THROWAWAY: per-segment offset of legacy charts vs audio, to test for drift.
//! Usage: drift <folder of <id>_all.ssq + <id>.wavm>
use ddr_chart_tools::sync::{self, Params, SyncEvent, TimeMap};
use ddr_chart_tools::{ssq, ssq_legacy, wavm};
use std::fs;

fn main() {
    let dir = std::env::args().nth(1).expect("folder");
    let mut rows = Vec::new();
    for entry in fs::read_dir(&dir).unwrap() {
        let p = entry.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let Some(id) = name.strip_suffix("_all.ssq") else { continue };
        let wav = p.with_file_name(format!("{id}.wavm"));
        let Ok(wbytes) = fs::read(&wav) else { continue };
        let mut parsed = ssq::parse(&fs::read(&p).unwrap()).unwrap();
        let src_tps = parsed.song.tps;
        ssq_legacy::modernize::modernize(&mut parsed);
        let audio = wavm::parse(&wbytes).unwrap();
        let map = TimeMap::from_tempo_pairs(&parsed.raw_tempo_pairs, parsed.song.tps).unwrap();
        let events = sync::chart_events(&parsed.song.charts, &map);
        let params = Params { max_correction_ms: 200 };
        let n = 4;
        let mut pts = Vec::new();
        for q in 0..n {
            let part: Vec<SyncEvent> = events[q * events.len() / n..(q + 1) * events.len() / n].to_vec();
            let mid = part[part.len() / 2].time_s;
            if let Some(m) = sync::estimate(&audio, &part, &params).measured_ms { pts.push((mid, m)); }
        }
        if pts.len() < 3 { continue; }
        let (sx, sy) = pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
        let (mx, my) = (sx / pts.len() as f64, sy / pts.len() as f64);
        let sxy: f64 = pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
        let sxx: f64 = pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
        let slope = sxy / sxx; // ms per second
        let resid: f64 = pts.iter().map(|p| (p.1 - (my + slope * (p.0 - mx))).abs()).fold(0.0, f64::max);
        let bpm = parsed.song.tempo_segments.first().map(|s| s.bpm.as_rational().as_f64()).unwrap_or(0.0);
        rows.push((id.to_string(), src_tps, bpm, slope, resid, pts.iter().map(|p| format!("{:.0}@{:.0}s", p.1, p.0)).collect::<Vec<_>>().join(" ")));
    }
    rows.sort_by(|a, b| a.3.total_cmp(&b.3));
    for r in &rows { println!("{:6} tps={:4} bpm={:7.2} drift={:+7.3} ms/s ({:+.3}%) maxresid={:5.1}  {}", r.0, r.1, r.2, r.3, r.3 / 10.0, r.4, r.5); }
    let mut s: Vec<f64> = rows.iter().map(|r| r.3).collect(); s.sort_by(f64::total_cmp);
    println!("n={} median drift {:+.3} ms/s", s.len(), s[s.len() / 2]);
}
