# Idea Honing — Decision Register

Project: `2026-09-28-auto-sync`. Ordered by blast radius: user-visible behavior
and public interfaces first, reversible internals last. `★` marks decisions the
maintainer likely has not considered.

| ID | Decision | Why it matters | Recommendation | Status |
|----|----------|----------------|----------------|--------|
| D1 | Which conversions accept auto-sync | User-visible scope; CLI validation rules | All four conversions (DDR and SM5 outputs) | Accepted |
| D2 | CLI surface | Public interface | `--auto-sync [apply\|report]` (bare flag = apply) + `--auto-sync-max-ms N` (renamed from `-window-ms`: it is now a cap on the correction, see D9) | Accepted |
| D3 ★ | Fix the timeline-shift primitive and apply `--sync-offset-ms` on every path | Changes existing `--sync-offset-ms` output for legacy → DDR | One uniform "shift the whole timeline by δ" operation, used by both features on all four paths | Accepted |
| D4 | Composition with `--sync-offset-ms` | Business rule 10; user-visible results | source sync → modernize → auto-sync to target → add bias | Accepted |
| D5 ★ | Estimation method | Determines accuracy and robustness | Whole-song correlation of the onset envelope against chart event times; no single-anchor snap, no tempo estimation | Accepted |
| D6 ★ | What chart times to correlate against | Beat grids alias inside a ±200 ms window | The chart's note pattern (union of all difficulties, weighted by agreement) | Accepted |
| D7 ★ | Definition of "in sync" (target offset) | Detector bias is nonzero and Konami's stock sync is not uniform | One constant `T = median(measured + community offset)` over the stock corpus (sign corrected by research), cross-checked on the 107 songs rated exactly in sync | Accepted |
| D8 | Behavior when the estimate is untrustworthy | Batch safety; user-visible warnings | Leave the source sync untouched, `warn!`, conversion still succeeds. Refuse when: correction exceeds the cap, split-half disagreement > 5 ms, or rival peak ≥ 0.9 | Accepted |
| D9 | Maximum correction | Bounds the correction; removes aliasing | Cap ±60 ms (configurable); search ±(cap + 10 ms) internally and refuse any result beyond the cap | Overridden |
| D10 | Correct the chart or the audio | Audio re-encode cost; legacy XWB passthrough | Chart-side only; audio bytes untouched | Accepted |
| D11 ★ | Where the analysis code lives | Overrides a steering rule | New top-level `src/sync/` module, with `.spec/steering/structure.md` updated in the same change | Accepted |
| D12 | FFT implementation | New dependency needs justification | `rustfft` (pure Rust) | Accepted |
| D13 | Note times come from the timing the output will carry | Legacy `tempo_segments` can differ from written pairs | SSQ output: final tempo pairs; SSC output: model segments/stops/offset | Assumed |
| D14 | Dead-band for tiny corrections | 1–2 ms corrections matter to top players | No dead-band: apply the correction rounded to whole ms (SSQ resolution); a correction that rounds to 0 changes nothing | Assumed |
| D15 | Signal-processing parameters | Accuracy vs. speed | Mono downmix; `rise` feature (10 ms Hann window, ~0.7 ms hop, log2 power, emphasis f·e^(−f/3 kHz), 2 ms signed difference); 1 ms shift grid with parabolic refinement — settled by prototype at the ±60 ms cap | Assumed |
| D16 | Which note kinds count | Mines are not musical hits | Taps, hold heads, shocks count; mines excluded | Assumed |
| D17 | Split-half consistency check | Catches wrong-BPM charts where one offset cannot fit | Estimate each half separately; disagreement > 5 ms makes the estimate untrustworthy (the threshold simulated against the corpus) | Accepted |
| D18 | Negative `tempo_data[0]` | Stock files stay within ±22 ms | Allowed; validated in-game during the plan | Assumed |
| D19 | Performance budget | Batch runs over whole packs | ≤ 2 s analysis per song in a release build | Assumed |
| D20 | Test strategy | No licensed fixtures | Synthetic onset tracks with known offsets in CI; corpus validation documented and run manually | Assumed |
| D21 | Out of scope | Scope control | BPM detection, BPM-drift correction, per-section or per-chart offsets, audio resampling, applying community offsets during conversion; follow-up: DDR → SM5 reading TPS ≠ 1000 anchors the way the game does (integer-ms normalization, verified in Ghidra) | Assumed |
| D22 | Role of the community offset database | Avoids a runtime dependency on another project's file | Calibration and validation input only; the tool never reads it at runtime | Assumed |
| D23 | Errors beyond the cap | Cannot be measured reliably (half-beat aliasing) | Out of scope; the refusal warning says the sync looks off by more than the cap and needs manual correction | Assumed |
| D24 | Where the calibration harness lives | `T` must be re-measured with the production code | An `#[ignore]`d, environment-gated test (`tests/auto_sync_calibration.rs`) that runs the production estimator over `$DDR_WORLD_INSTALL` and a CSV path. Calibrates on TPS 1000 stock songs only, where DDR's integer-ms anchor normalization and raw timing coincide | Assumed |

All `Proposed` decisions accepted 2026-09-28 (D1 and D3 individually, D2 and
D4–D12 as recommended, D7 in its revised form). `Assumed` rows stand unless
overridden.

## Detail

### D1 — Which conversions accept auto-sync

**Question.** The idea names DDR output. Should `--auto-sync` also be accepted
for SM5 output?

**Recommendation.** Accept it on all four conversions. The estimator and the
shift primitive are target-agnostic, so SM5 support is a validation rule and
one call per path. It gives a direct way to check a result by eye (legacy →
SM5 with auto-sync, open in ArrowVortex, look at beat lines against the
waveform), and report mode on DDR → SM5 over stock songs is how D7 is
calibrated.

**Rejected alternative.** DDR outputs only, as literally stated. Smaller
surface, but loses the eyeball check and needs a special case for
calibration.

**Accepted 2026-09-28** — include all conversion types.

### D2 — CLI surface

**Recommendation.**

- `--auto-sync` — measure and correct (equivalent to `--auto-sync apply`).
- `--auto-sync report` — measure and log, write outputs with the source sync
  unchanged. Used for calibration and for auditing a batch before applying.
- `--auto-sync-window-ms N` — half-width of the search window, default 200;
  only valid alongside `--auto-sync`.

Implemented as a clap `value_enum` with an optional value
(`default_missing_value = "apply"`), keeping derive in `src/cli/` and the
"window requires auto-sync" rule in `Cli::validate()`.

**Rejected alternative.** Separate `--auto-sync` and `--auto-sync-dry-run`
booleans — two flags for one tri-state.

**Revised 2026-09-28 (follows the D9 override).** `--auto-sync-window-ms`
becomes `--auto-sync-max-ms N`, default 60: a cap on the applied correction,
not a search width. Only valid alongside `--auto-sync`.

### D3 — Fix the timeline-shift primitive ★

**Finding.** `apply_sync_offset` (`src/job/mod.rs:259-271`) adds the bias to
`raw_tempo_pairs[0].1` only. SSQ `tempo_data` values are absolute audio times
and BPM is the slope between pairs, so for DDR output this bends the first
tempo segment rather than moving the chart. On a single-BPM song the applied
correction tapers from N ms at beat 0 to 0 ms at END; on multi-segment songs
only notes in the first segment move. Separately, `--sync-offset-ms` is
silently ignored on SM5 → DDR and DDR → SM5.

**Recommendation.** Replace it with one operation that shifts every
`tempo_data` entry and `audio_sync_offset_seconds` by δ, and use it for both
the bias and auto-sync on all four paths.

**Consequence.** Legacy → DDR output with `--sync-offset-ms` changes. The
documented `+53 ms` for Ultramix → DDR World was tuned against the old
behavior and may need re-deriving — which D7's calibration run does anyway.

**Accepted 2026-09-28.** The `+53 ms` figure was never checked on long
single-BPM songs (Q2), so it is treated as unvalidated. The README guidance is
re-derived by measuring Ultramix material with report mode after the fix.

### D4 — Composition with `--sync-offset-ms`

**Recommendation.** Order: parse source sync → modernize (legacy) → auto-sync
moves the chart to the target → add `--sync-offset-ms`. The bias stays a
per-target engine correction layered on top (rule 10 holds). If calibration
(D7) shows the Ultramix +53 ms was a content offset rather than engine
latency, auto-sync absorbs it and the README says to drop the bias when
auto-sync is on.

### D5 — Estimation method ★

**Recommendation.** Compute an onset-strength envelope for the audio. For
each candidate shift δ in the window, score = weighted sum of the envelope at
every chart event time shifted by δ. Pick the maximum and refine it to
sub-millisecond precision.

This uses the chart's exact tempo map (BPMs and stops), so no beat tracking
or BPM estimation is needed — there is one unknown scalar. It uses hundreds of
events, so per-onset jitter averages out.

**Rejected alternative.** Snap the chart start to the nearest detected beat
(the rough idea). The first beat is often in silence, a fade-in, or a pickup;
one onset carries 10–20 ms of detector jitter; and a wrong pick gives no
signal that it is wrong.

### D6 — Correlate against the note pattern ★

**Finding.** A regular beat grid makes the score periodic with the beat
period. At 180 BPM (333 ms beat) a true +150 ms error and an alias at −183 ms
score the same, both inside ±200 ms. At 300 BPM the window holds two whole
periods.

**Recommendation.** Use the union of note times across every difficulty, each
time weighted by how many charts place a note there. Rests, breaks, and
syncopation make the pattern aperiodic, so the score has one clear peak; the
weighting emphasizes the hits every difficulty agrees on, which are the
strongest musical events.

**Rejected alternative.** Beat grid plus a "prefer the peak nearest 0 ms"
prior (what nine-or-null effectively does with its narrow window). Fine for
±9 ms decisions; picks the wrong alias when the true error exceeds half a
beat.

**Research (2026-09-28).** Decision holds; rationale corrected. The note
template beats the beat grid at every error size (94% vs 90% stock, 90% vs
72–77% at ±150 ms), but it *reduces* aliasing rather than removing it: DDR
music has strong off-beat onsets, so the half-beat alias still scores
0.94–1.00 of the true peak on about 2.5% of songs.
`research/prototype-results.md` §3–4.

### D7 — Target offset: calibrate against stock DDR World, corrected by community offsets ★

**Finding (from Q1).** Konami did not sync every stock song well. The
maintainer's modpack ships a community-sourced per-song offset database
(`judgement_offsets.csv`, 1,440 songs matched to local SSQ + XWB). Values
are the compensating JUDGEMENT OFFSET in ms, positive = judged later, so a
song with value `c` needs its chart moved `c` ms later to be in sync. Median
−2 ms, std. dev. 6.8 ms, range −34 … +23 ms; 107 songs are exactly 0. Full
analysis: `research/calibration-corpus.md`.

**Recommendation (revised).** Measure every stock song with the estimator
(`m` = audio onset − chart event time). Then:

- `T = median(m − c)` over all 1,440 songs is the shipped target constant.
- `median(m)` over the 107 songs with `c = 0` must agree with `T` — the
  cross-check that the sign convention is right.
- The spread of `m − c − T` is the estimator's accuracy against community
  ground truth, and songs with large residuals set the D8 thresholds.
- To exercise the large-error range the stock corpus lacks, stock charts are
  also shifted by known amounts (±50 … ±200 ms) and the estimator must
  recover `c + shift`. This is the direct test of D6's aliasing argument on
  real music.

The constant ships in `src/sync/` with a doc comment recording the corpus,
`T`, and the residual spread. SM5 output uses the same target (assumes a
well-synced DDR World chart ≈ StepMania null; checked by eye in ArrowVortex).

**Rejected alternative.** Only the 107 exactly-zero songs. Sidesteps the
sign question, but uses 7% of the data and gives no accuracy measurement
across the range of real errors.

**Research (2026-09-28).** The sign reading above was inverted: empirically
`m = T − c`, so `T = median(m + c)`, and a song with `c > 0` has early music
(its chart would move earlier). `T = −1.74 ms` for the prototype's feature;
the `c = 0` subset gives −1.54 ms. Residual robust std. dev. 0.72 ms. At the
±60 ms cap (D9 override) the best feature is `rise`, with `T = −2.10 ms` and
residual robust std. dev. 0.47 ms. On the 711 TPS 1000 songs, where the game's
integer-ms anchor normalization cannot differ from raw timing, agreement is
0.44 ms and 94% within 1 ms; calibration uses that subset (D24,
`research/prototype-results.md` §9). The
community values come from an automated tool validated in play (maintainer,
2026-09-28), so this is agreement with that tool. `T` is feature-specific
and must be re-measured with the production implementation (D24).

### D8 — Untrustworthy estimates

**Recommendation.** When confidence is low (weak or broad peak, a competing
peak, peak on the window edge, split-half disagreement per D17), leave the
source sync untouched, log `warn!` with the measurement and the reason, and
let the conversion succeed. Thresholds are set from corpus results.

**Rejected alternatives.** Failing the job (one ambient song would abort a
pack in single mode and clutter batch errors), or applying anyway (silently
makes a correct chart wrong).

**Research (2026-09-28).** Thresholds from data: refuse when the strongest
rival peak more than 25 ms away scores ≥ 0.9 of the best, when the two halves
disagree by more than 10 ms, or when the peak is within 5 ms of the window
edge. With the ±200 ms window this is wrong about 3% of the time at every
error size, and refuses 13% (stock) to 33% (±150 ms).

**Revised for the ±60 ms cap (2026-09-28).** The rival-peak gate never fires
inside ±60 ms and stays only as a safety net for very high BPM. The useful
signal is split-half disagreement: > 5 ms refuses 4.4% of stock songs and
trims confident-looking outliers (29 → 21 songs more than 3 ms from the
community value). A result beyond the cap replaces the old window-edge rule.
`research/prototype-results.md` §8.

### D9 — Search window

**Recommendation.** Default ±200 ms (the top of your estimate), configurable
via `--auto-sync-window-ms`. A peak within a few ms of the window edge means
the true answer may lie outside, so it is treated as untrustworthy and the
warning suggests widening the window.

**Research (2026-09-28).** Holds, *with* D8's gating. Half a beat at DDR's
common 150–200 BPM is 150–200 ms, so the window always contains the
half-beat alias. A tempo-scaled window (±0.45 beat) is better on small
errors but gives confidently wrong answers when the real error is large
(26–28% wrong at ±150 ms); a hybrid is in between (16% wrong at ±150 ms).
Fixed ±200 ms with gating is the only option that keeps wrong answers near
3% across the whole range. `research/prototype-results.md` §5.

**Overridden 2026-09-28 by the maintainer:** ±200 ms is too aggressive, and
1–2 ms agreement matters because top players feel it. The cap is ±60 ms,
enough for the legacy console offset (~53 ms). At that cap no half-beat
alias fits, and agreement with the community values tightens to a robust
std. dev. of 0.47 ms: 91% within 1 ms, 97% within 2 ms
(`research/prototype-results.md` §8). The search runs to ±70 ms so that
corrections near the cap are not truncated by the window edge; results
beyond ±60 ms are refused. Flag renamed `--auto-sync-max-ms` (D2).

### D10 — Chart-side correction only

**Recommendation.** Move the chart (tempo pairs / `#OFFSET`), never the
audio. Lossless, keeps the legacy XWB passthrough, and leaves the preview
slice untouched. The passthrough path decodes the XWB for analysis only.

**Rejected alternative.** Pad or trim audio — forces a lossy re-encode and
shifts preview timing.

### D11 — New top-level `src/sync/` module ★

**Finding.** `.spec/steering/structure.md` forbids new top-level modules, but
onset detection and offset estimation are a concern no existing module owns:
`model/` holds types and rules, `util/` excludes domain logic, `job/` is
orchestration, and format modules own byte layouts.

**Recommendation.** Add `src/sync/`, pure computation over an `AudioBuffer`
plus a list of weighted event times (no I/O, no format knowledge). `job/`
builds the event list from the output's timing and applies the result.
Update `structure.md` (layout and responsibilities table), `product.md`
(glossary: auto-sync, sync target; a new business rule for D4's ordering),
and `tech.md` (dependency table) in the same change.

**Rejected alternative.** `src/job/auto_sync/` — honors the rule's letter but
puts DSP inside the orchestration layer.

### D12 — FFT implementation

**Recommendation.** `rustfft` — pure Rust, no C toolchain (matters for the
`x86_64-pc-windows-gnu` cross-compile), widely used, SIMD-accelerated.
Dependency footprint is confirmed in research before the design commits.

**Rejected alternatives.** Hand-rolled radix-2 FFT (about 100 lines, but more
numeric code to own and test); time-domain band-energy onsets with no FFT
(weaker on dense mixes).

### D13–D24 — Assumed

Settled without escalation; listed in the table so the design can be audited
against them. Override any by ID.

## Questions

- **Q1 — answered 2026-09-28.** Stock songs live under
  `$DDR_WORLD_INSTALL/data/`; per-song ground truth comes from the modpack's
  community `judgement_offsets.csv`. Folded into D7; details in
  `research/calibration-corpus.md`.
- **Q2 — answered 2026-09-28.** The `+53 ms` Ultramix bias has not been
  checked on long single-BPM songs. Treated as unvalidated (see D3).

## Outliers against the community values

2026-09-28, maintainer: the modpack's `judgement_offsets.csv` may be out of
date. The current community value for `tiho` now matches this tool's
estimate. The remaining confident disagreements (`hane`, `reci`, `osak2`,
`wwto`) are not investigated further. The estimator agrees with the
community values for the large majority of songs, which is the acceptance
bar.

Readiness Confirmed 2026-09-28

