//! THROWAWAY PROTOTYPE for the auto-sync feature — not production code and
//! never carried into `src/`. Measures how far each stock DDR World chart is
//! from its audio and compares onset features, templates, and confidence
//! signals against the community offset database.
//!
//! Usage:
//!   sync-probe <ddr_data_dir> <judgement_offsets.csv> <out_dir>
//!              [--stride K] [--limit N] [--threads T] [--codes a,b,c]
//!   Writes <out_dir>/songs.csv and one score-curve file per song under <out_dir>/curves/.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use ddr_chart_tools::model::NoteKind;
use ddr_chart_tools::{ssq, xwb};
use rustfft::num_complex::Complex32;
use rustfft::FftPlanner;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Kind {
    /// Sum of positive log-magnitude differences (optionally vs. a
    /// frequency-max-filtered reference frame: SuperFlux-style).
    Flux,
    /// Signed difference of frequency-weighted log2 energy (nine-or-null style).
    Rise,
}

#[derive(Clone, Copy, Debug)]
enum Weighting {
    Flat,
    /// w(f) = f * exp(-f / 3000)
    Emphasis3k,
    /// w(f) = f * exp(-f / 1000)
    Emphasis1k,
}

#[derive(Clone, Debug)]
struct FeatureCfg {
    name: &'static str,
    win: usize,
    hop: usize,
    lag_ms: f64,
    maxfilt: usize,
    weighting: Weighting,
    kind: Kind,
    lo_hz: f64,
    hi_hz: f64,
}

static FEATURE_FILTER: Mutex<Option<Vec<String>>> = Mutex::new(None);

fn feature_cfgs() -> Vec<FeatureCfg> {
    let all = all_feature_cfgs();
    match FEATURE_FILTER.lock().unwrap().as_ref() {
        Some(names) => all.into_iter().filter(|f| names.iter().any(|n| n == f.name)).collect(),
        None => all,
    }
}

fn all_feature_cfgs() -> Vec<FeatureCfg> {
    vec![
        FeatureCfg {
            name: "flux",
            win: 1024,
            hop: 64,
            lag_ms: 1.45,
            maxfilt: 0,
            weighting: Weighting::Flat,
            kind: Kind::Flux,
            lo_hz: 30.0,
            hi_hz: 12000.0,
        },
        FeatureCfg {
            name: "rise",
            win: 441,
            hop: 32,
            lag_ms: 2.0,
            maxfilt: 0,
            weighting: Weighting::Emphasis3k,
            kind: Kind::Rise,
            lo_hz: 30.0,
            hi_hz: 16000.0,
        },
        FeatureCfg {
            name: "rise1k",
            win: 441,
            hop: 32,
            lag_ms: 2.0,
            maxfilt: 0,
            weighting: Weighting::Emphasis1k,
            kind: Kind::Rise,
            lo_hz: 30.0,
            hi_hz: 16000.0,
        },
        FeatureCfg {
            name: "riselp",
            win: 882,
            hop: 32,
            lag_ms: 3.0,
            maxfilt: 0,
            weighting: Weighting::Flat,
            kind: Kind::Rise,
            lo_hz: 30.0,
            hi_hz: 500.0,
        },
        FeatureCfg {
            name: "low",
            win: 2048,
            hop: 64,
            lag_ms: 5.0,
            maxfilt: 0,
            weighting: Weighting::Flat,
            kind: Kind::Flux,
            lo_hz: 30.0,
            hi_hz: 180.0,
        },
    ]
}

/// Half-width of the stored score curve. Wide enough that every
/// synthetic shift (±200) can be evaluated with a ±260 ms window by
/// slicing: curve_s(d) == curve(d + s).
const CURVE_MS: i32 = 460;


// ---------------------------------------------------------------------------
// Envelope
// ---------------------------------------------------------------------------

struct Envelope {
    data: Vec<f32>,
    /// Time (s) of frame 0's attribution point.
    t0: f64,
    /// Seconds per frame.
    dt: f64,
}

impl Envelope {
    fn sample(&self, t: f64) -> f64 {
        let x = (t - self.t0) / self.dt;
        if x < 0.0 {
            return 0.0;
        }
        let i = x.floor() as usize;
        if i + 1 >= self.data.len() {
            return 0.0;
        }
        let f = x - i as f64;
        self.data[i] as f64 * (1.0 - f) + self.data[i + 1] as f64 * f
    }
    fn end_time(&self) -> f64 {
        self.t0 + self.dt * self.data.len() as f64
    }
}

fn compute_envelope(mono: &[f32], sr: u32, cfg: &FeatureCfg, planner: &mut FftPlanner<f32>) -> Envelope {
    let nfft = cfg.win.next_power_of_two();
    let fft = planner.plan_fft_forward(nfft);
    let window: Vec<f32> = (0..cfg.win)
        .map(|n| {
            let x = std::f32::consts::PI * 2.0 * n as f32 / cfg.win as f32;
            0.5 - 0.5 * x.cos()
        })
        .collect();
    let bin_hz = sr as f64 / nfft as f64;
    let k_lo = ((cfg.lo_hz / bin_hz).ceil() as usize).max(1);
    let k_hi = ((cfg.hi_hz / bin_hz).floor() as usize).min(nfft / 2);
    let nb = k_hi - k_lo + 1;
    let weights: Vec<f32> = (k_lo..=k_hi)
        .map(|k| match cfg.weighting {
            Weighting::Flat => 1.0,
            Weighting::Emphasis3k => {
                let f = k as f64 * bin_hz;
                (f * (-f / 3000.0).exp() / 1103.6) as f32 // normalized so peak ~1
            }
            Weighting::Emphasis1k => {
                let f = k as f64 * bin_hz;
                (f * (-f / 1000.0).exp() / 367.9) as f32
            }
        })
        .collect();
    let lag = ((cfg.lag_ms * 1e-3 * sr as f64 / cfg.hop as f64).round() as usize).max(1);
    let n_frames = if mono.len() > cfg.win { (mono.len() - cfg.win) / cfg.hop + 1 } else { 0 };

    let mut buf = vec![Complex32::new(0.0, 0.0); nfft];
    let mut scratch = vec![Complex32::new(0.0, 0.0); fft.get_inplace_scratch_len()];
    // Ring of the last (lag+1) log spectra (Flux) or energies (Rise).
    let mut ring: Vec<Vec<f32>> = vec![vec![0.0; nb]; lag + 1];
    let mut energies: Vec<f32> = Vec::with_capacity(n_frames);
    let mut out = Vec::with_capacity(n_frames);
    let mut maxf = vec![0.0f32; nb];

    for f in 0..n_frames {
        let start = f * cfg.hop;
        for i in 0..nfft {
            buf[i] = if i < cfg.win {
                Complex32::new(mono[start + i] * window[i], 0.0)
            } else {
                Complex32::new(0.0, 0.0)
            };
        }
        fft.process_with_scratch(&mut buf, &mut scratch);
        match cfg.kind {
            Kind::Flux => {
                let slot = f % (lag + 1);
                for (j, k) in (k_lo..=k_hi).enumerate() {
                    let mag = buf[k].norm();
                    ring[slot][j] = (1.0 + mag).ln();
                }
                if f < lag {
                    out.push(0.0);
                    continue;
                }
                let refslot = (f - lag) % (lag + 1);
                let reference = &ring[refslot];
                let cur = &ring[slot];
                let r: &[f32] = if cfg.maxfilt > 0 {
                    for j in 0..nb {
                        let a = j.saturating_sub(cfg.maxfilt);
                        let b = (j + cfg.maxfilt).min(nb - 1);
                        let mut m = f32::MIN;
                        for v in &reference[a..=b] {
                            if *v > m {
                                m = *v;
                            }
                        }
                        maxf[j] = m;
                    }
                    &maxf
                } else {
                    reference
                };
                let mut s = 0.0f32;
                for j in 0..nb {
                    let d = cur[j] - r[j];
                    if d > 0.0 {
                        s += weights[j] * d;
                    }
                }
                out.push(s);
            }
            Kind::Rise => {
                let mut e = 0.0f32;
                for (j, k) in (k_lo..=k_hi).enumerate() {
                    let p = buf[k].norm_sqr();
                    e += weights[j] * (p + 1e-9).log2();
                }
                energies.push(e);
                if f < lag {
                    out.push(0.0);
                } else {
                    out.push(e - energies[f - lag]);
                }
            }
        }
    }
    // Attribute each frame to its window center, minus half the lag (the
    // difference straddles frames f-lag .. f).
    let dt = cfg.hop as f64 / sr as f64;
    let t0 = (cfg.win as f64 / 2.0) / sr as f64 - (lag as f64 * dt) / 2.0;
    Envelope { data: out, t0, dt }
}

// ---------------------------------------------------------------------------
// Timing
// ---------------------------------------------------------------------------

/// `(measure_tick, ms)` pairs.
fn tick_to_ms(pairs: &[(i64, f64)], t: f64) -> f64 {
    let n = pairs.len();
    if n == 1 {
        return pairs[0].1;
    }
    let k = pairs.partition_point(|p| (p.0 as f64) < t);
    if k < n && (pairs[k].0 as f64) == t {
        return pairs[k].1;
    }
    let (a, b) = if k == 0 {
        let b = pairs.iter().find(|p| p.0 > pairs[0].0).copied().unwrap_or(pairs[1]);
        (pairs[0], b)
    } else if k == n {
        let last = pairs[n - 1];
        let a = pairs.iter().rev().find(|p| p.0 < last.0).copied().unwrap_or(pairs[n - 2]);
        (a, last)
    } else {
        (pairs[k - 1], pairs[k])
    };
    if b.0 == a.0 {
        return a.1;
    }
    a.1 + (b.1 - a.1) * (t - a.0 as f64) / (b.0 - a.0) as f64
}

/// DDR World's anchor normalization as documented in ssq2sm.py
/// (`ddr_timing_anchor_ms`): int32(v*1000) -> f32 -> / f32(tps) -> + 0.5f -> floor.
fn ddr_anchor_ms(v: i32, tps: u32) -> f64 {
    let numerator = (v as i64 * 1000) as i32; // wraps like the game's int32
    let x = numerator as f32;
    let x = x / tps as f32;
    let x = x + 0.5f32;
    x.floor() as f64
}

static DDR_TIMING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct Templates {
    /// (time_s, weight, measure_tick)
    notes: Vec<(f64, f64, i64)>,
    beats: Vec<(f64, f64)>,
    bpm_min: f64,
    bpm_max: f64,
    n_stops: usize,
}

fn build_templates(parsed: &ssq::SsqParseResult) -> Option<Templates> {
    let tps = parsed.song.tps as f64;
    let ddr = DDR_TIMING.load(Ordering::Relaxed);
    let mut pairs: Vec<(i64, f64)> = parsed
        .raw_tempo_pairs
        .iter()
        .map(|&(t, d)| {
            let ms = if ddr { ddr_anchor_ms(d, parsed.song.tps) } else { d as f64 * 1000.0 / tps };
            (t as i64, ms)
        })
        .collect();
    // A wrapped terminal anchor (int32 overflow) would run time backwards;
    // truncate at the first anchor whose time decreases.
    if let Some(bad) = pairs.windows(2).position(|w| w[1].1 < w[0].1) {
        pairs.truncate(bad + 1);
    }
    if pairs.len() < 2 {
        return None;
    }
    let mut counts: BTreeMap<i64, u32> = BTreeMap::new();
    for chart in &parsed.song.charts {
        let mut seen = HashSet::new();
        for note in &chart.notes {
            if matches!(note.kind, NoteKind::Mine) {
                continue;
            }
            let r = note.beat.as_rational();
            let tick = (r.num() as i128 * 1024 / r.den() as i128) as i64;
            if seen.insert(tick) {
                *counts.entry(tick).or_insert(0) += 1;
            }
        }
    }
    if counts.is_empty() {
        return None;
    }
    let first = *counts.keys().next()?;
    let last = *counts.keys().next_back()?;
    let notes = counts
        .iter()
        .map(|(&t, &c)| (tick_to_ms(&pairs, t as f64) / 1000.0, c as f64, t))
        .collect();
    let mut beats = Vec::new();
    let mut t = first.div_euclid(1024) * 1024;
    while t <= last {
        beats.push((tick_to_ms(&pairs, t as f64) / 1000.0, 1.0));
        t += 1024;
    }
    let mut bpm_min = f64::MAX;
    let mut bpm_max = 0.0f64;
    let mut n_stops = 0;
    for w in pairs.windows(2) {
        let dt = (w[1].0 - w[0].0) as f64;
        let ds = w[1].1 - w[0].1;
        if dt == 0.0 {
            n_stops += 1;
        } else if ds > 0.0 {
            let bpm = 60000.0 * (dt / 1024.0) / ds;
            bpm_min = bpm_min.min(bpm);
            bpm_max = bpm_max.max(bpm);
        }
    }
    Some(Templates { notes, beats, bpm_min, bpm_max, n_stops })
}

// ---------------------------------------------------------------------------
// Score curves (all selection logic lives in analyze.py)
// ---------------------------------------------------------------------------

/// Score for every integer shift d in [-CURVE_MS, CURVE_MS]:
/// sum_i w_i * env(t_i + d). Peak at d = audio onset − chart time.
fn curve(env: &Envelope, events: &[(f64, f64)]) -> Vec<f32> {
    (-CURVE_MS..=CURVE_MS)
        .map(|d| {
            let dd = d as f64 / 1000.0;
            events.iter().map(|&(t, wt)| wt * env.sample(t + dd)).sum::<f64>() as f32
        })
        .collect()
}

fn usable(events: &[(f64, f64)], env: &Envelope, margin_s: f64) -> Vec<(f64, f64)> {
    let end = env.end_time();
    events
        .iter()
        .copied()
        .filter(|&(t, _)| t - margin_s >= env.t0 && t + margin_s <= end)
        .collect()
}

/// Metrical weight: quarter-note positions 1.0, eighth off-beats 0.5,
/// anything finer 0.25. Ticks are measure ticks (1024 per beat).
fn metric_weight(tick: i64) -> f64 {
    if tick.rem_euclid(1024) == 0 {
        1.0
    } else if tick.rem_euclid(512) == 0 {
        0.5
    } else {
        0.25
    }
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

struct Job {
    code: String,
    c: i32,
    ssq: PathBuf,
    xwb: PathBuf,
}

const TEMPLATE_NAMES: [&str; 3] = ["notes", "metric", "beats"];
const PARTS: [&str; 3] = ["all", "h1", "h2"];

fn header() -> String {
    let mut h: Vec<String> = [
        "code", "c", "sr", "dur_s", "n_notes", "n_beats", "bpm_min", "bpm_max", "n_stops", "t_decode_ms",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for f in feature_cfgs() {
        h.push(format!("t_{}_ms", f.name));
    }
    h.push("t_curves_ms".into());
    h.join(",")
}

/// Curve order in each .bin file: feature-major, then template, then part.
fn curve_order() -> Vec<String> {
    let mut v = Vec::new();
    for f in feature_cfgs() {
        for t in TEMPLATE_NAMES {
            for p in PARTS {
                v.push(format!("{}_{}_{}", f.name, t, p));
            }
        }
    }
    v
}

fn run_job(job: &Job, planner: &mut FftPlanner<f32>, curve_dir: &Path) -> Result<String, String> {
    let ssq_bytes = fs::read(&job.ssq).map_err(|e| format!("read ssq: {e}"))?;
    let parsed = ssq::parse(&ssq_bytes).map_err(|e| format!("parse ssq: {e}"))?;
    let tpl = build_templates(&parsed).ok_or("no usable timing/notes")?;

    let t = Instant::now();
    let xwb_bytes = fs::read(&job.xwb).map_err(|e| format!("read xwb: {e}"))?;
    let audio = xwb::parse_audio(&xwb_bytes).map_err(|e| format!("decode xwb: {e}"))?;
    let ch = audio.channels.max(1) as usize;
    let mono: Vec<f32> = audio
        .samples
        .chunks_exact(ch)
        .map(|fr| fr.iter().map(|&s| s as f32).sum::<f32>() / (ch as f32 * 32768.0))
        .collect();
    let t_decode = t.elapsed().as_secs_f64() * 1000.0;
    let dur = mono.len() as f64 / audio.sample_rate as f64;

    let mut row = vec![
        job.code.clone(),
        job.c.to_string(),
        audio.sample_rate.to_string(),
        format!("{dur:.2}"),
        tpl.notes.len().to_string(),
        tpl.beats.len().to_string(),
        format!("{:.3}", tpl.bpm_min),
        format!("{:.3}", tpl.bpm_max),
        tpl.n_stops.to_string(),
        format!("{t_decode:.1}"),
    ];

    let mut blob: Vec<u8> = Vec::new();
    let mut t_curves = 0.0;
    let margin = CURVE_MS as f64 / 1000.0 + 0.1;
    for cfg in feature_cfgs() {
        let t = Instant::now();
        let env = compute_envelope(&mono, audio.sample_rate, &cfg, planner);
        row.push(format!("{:.1}", t.elapsed().as_secs_f64() * 1000.0));
        let t = Instant::now();
        let templates: [Vec<(f64, f64)>; 3] = [
            tpl.notes.iter().map(|&(t, w, _)| (t, w)).collect(),
            tpl.notes.iter().map(|&(t, w, tick)| (t, w * metric_weight(tick))).collect(),
            tpl.beats.clone(),
        ];
        for events in &templates {
            let ev = usable(events, &env, margin);
            let mid = ev.len() / 2;
            for part in [&ev[..], &ev[..mid], &ev[mid..]] {
                for v in curve(&env, part) {
                    blob.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        t_curves += t.elapsed().as_secs_f64() * 1000.0;
    }
    row.push(format!("{t_curves:.1}"));
    fs::write(curve_dir.join(format!("{}.bin", job.code)), blob).map_err(|e| format!("write curves: {e}"))?;
    Ok(row.join(","))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: sync-probe <ddr_data_dir> <judgement_offsets.csv> <out_dir> [--stride K] [--limit N] [--threads T] [--codes a,b]");
        std::process::exit(2);
    }
    let data = PathBuf::from(&args[1]);
    let csv_text = fs::read_to_string(&args[2]).expect("read offsets csv");
    let out_dir = PathBuf::from(&args[3]);
    let curve_dir = out_dir.join("curves");
    fs::create_dir_all(&curve_dir).expect("create out dir");
    let mut stride = 1usize;
    let mut limit = usize::MAX;
    let mut threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let mut codes_filter: Option<HashSet<String>> = None;
    let mut i = 4;
    while i < args.len() {
        match args[i].as_str() {
            "--stride" => stride = args[i + 1].parse().unwrap(),
            "--limit" => limit = args[i + 1].parse().unwrap(),
            "--threads" => threads = args[i + 1].parse().unwrap(),
            "--codes" => codes_filter = Some(args[i + 1].split(',').map(String::from).collect()),
            "--timing" => DDR_TIMING.store(args[i + 1] == "ddr", Ordering::Relaxed),
            "--features" => {
                let names: Vec<String> = args[i + 1].split(',').map(String::from).collect();
                *FEATURE_FILTER.lock().unwrap() = Some(names);
            }
            other => panic!("unknown arg {other}"),
        }
        i += 2;
    }

    let mut jobs = Vec::new();
    for line in csv_text.lines().skip(1) {
        let cols: Vec<&str> = line.trim().split(',').map(str::trim).collect();
        if cols.len() < 2 || cols[1].is_empty() {
            continue;
        }
        let code = cols[0].to_string();
        if let Some(f) = &codes_filter {
            if !f.contains(&code) {
                continue;
            }
        }
        let ssq = data.join("mdb_apx/ssq").join(format!("{code}.ssq"));
        let xwb = data.join("sound/win/dance").join(format!("{code}.xwb"));
        if !ssq.is_file() || !xwb.is_file() {
            continue;
        }
        jobs.push(Job { code, c: cols[1].parse().unwrap(), ssq, xwb });
    }
    let jobs: Vec<Job> = jobs.into_iter().step_by(stride).take(limit).collect();
    eprintln!("{} songs, {threads} threads", jobs.len());

    let next = AtomicUsize::new(0);
    let rows: Mutex<Vec<(usize, String)>> = Mutex::new(Vec::new());
    let started = Instant::now();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                let mut planner = FftPlanner::<f32>::new();
                loop {
                    let idx = next.fetch_add(1, Ordering::Relaxed);
                    if idx >= jobs.len() {
                        break;
                    }
                    let job = &jobs[idx];
                    match run_job(job, &mut planner, &curve_dir) {
                        Ok(r) => rows.lock().unwrap().push((idx, r)),
                        Err(e) => eprintln!("{}: {e}", job.code),
                    }
                    let done = rows.lock().unwrap().len();
                    if done % 100 == 0 {
                        eprintln!("{done} done ({:.0}s)", started.elapsed().as_secs_f64());
                    }
                }
            });
        }
    });
    let mut rows = rows.into_inner().unwrap();
    rows.sort_by_key(|r| r.0);
    let mut out = fs::File::create(out_dir.join("songs.csv")).expect("create songs.csv");
    writeln!(out, "{}", header()).unwrap();
    for (_, r) in rows {
        writeln!(out, "{r}").unwrap();
    }
    fs::write(out_dir.join("curve_order.txt"), curve_order().join("\n")).unwrap();
    fs::write(out_dir.join("curve_ms.txt"), CURVE_MS.to_string()).unwrap();
    eprintln!("wrote {} in {:.0}s", out_dir.display(), started.elapsed().as_secs_f64());
}
