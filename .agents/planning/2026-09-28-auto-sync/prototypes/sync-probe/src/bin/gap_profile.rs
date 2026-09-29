//! THROWAWAY: gap profile for one legacy chart.
use ddr_chart_tools::sync::TimeMap;
use ddr_chart_tools::{ssq, ssq_legacy};
fn main() {
    let p = std::env::args().nth(1).expect("ssq");
    let mut parsed = ssq::parse(&std::fs::read(&p).unwrap()).unwrap();
    println!("raw pairs (pre-modernize, tps {}): {:?}", parsed.song.tps, parsed.raw_tempo_pairs);
    ssq_legacy::modernize::modernize(&mut parsed);
    println!("modernized pairs: {:?}", parsed.raw_tempo_pairs);
    println!("segments: {:?}", parsed.song.tempo_segments.iter().map(|s| (s.start_beat.as_rational().as_f64(), s.bpm.as_rational().as_f64())).collect::<Vec<_>>());
    println!("stops: {:?}", parsed.song.stops.iter().map(|s| (s.at_beat.as_rational().as_f64(), s.duration_seconds.as_f64())).collect::<Vec<_>>());
    let a = TimeMap::from_tempo_pairs(&parsed.raw_tempo_pairs, 1000).unwrap();
    let b = TimeMap::from_song(&parsed.song).unwrap();
    let first = parsed.song.charts.iter().flat_map(|c| c.notes.iter()).map(|n| n.beat.as_rational().as_f64()).fold(f64::MAX, f64::min);
    let last = parsed.song.charts.iter().flat_map(|c| c.notes.iter()).map(|n| n.beat.as_rational().as_f64()).fold(0.0, f64::max);
    println!("notes from beat {first} to {last}");
    for beat in [0.0, 4.0, 8.0, first, 100.0, 200.0, 280.0, 290.0, last] { println!("  beat {beat:6.1}: pairs {:9.3}s  song {:9.3}s  gap {:+8.2} ms", a.seconds_at(beat), b.seconds_at(beat), (a.seconds_at(beat)-b.seconds_at(beat))*1000.0); }
}
