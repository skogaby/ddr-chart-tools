# Orientation

Blind-spot pass over the codebase and prior art before settling requirements.

## The idea as understood

Add an opt-in step to the conversion pipeline that measures how far the
chart's timing is from the audio's actual rhythmic onsets and corrects the
chart's sync offset by that amount, within a bounded window (about ±200 ms).
Tempo (BPMs, stops) is trusted as-is; only the single sync scalar moves.

## Codebase findings

### Sync is one scalar in the model, but per-path handling is uneven

- The model carries sync as `Song::audio_sync_offset_seconds`, in the DDR sign
  convention (audio time at which beat 0 occurs). `src/model/mod.rs:228-235`.
- In an SSQ, every `tempo_data[i]` is an **absolute** audio time in
  seconds-ticks; BPM is the slope between consecutive pairs
  (`docs/ssq_format.md` §3, `src/ssq/tempo.rs:98-145`). Moving the whole chart
  relative to the audio therefore means adding δ to *every* `tempo_data`
  entry, not just `tempo_data[0]`.

How each conversion path handles sync today (`src/job/mod.rs`):

| Path | How the offset reaches the output | `--sync-offset-ms` |
|------|-----------------------------------|--------------------|
| SM5 → DDR (`sm5_to_ddr`) | `synthesize_tempo_entries_until` seeds its accumulator from `audio_sync_offset_seconds` (`src/ssq/writer.rs:159`), so every pair shifts uniformly | **Not applied** — silently ignored |
| DDR_LEGACY → DDR (`legacy_to_ddr`) | Modernize zeroes `tempo_data[0]` (`src/ssq_legacy/modernize.rs:113-132`); writer emits `raw_tempo_pairs` verbatim | `apply_sync_offset` adds N to `raw_tempo_pairs[0].1` **only** (`src/job/mod.rs:259-271`) |
| DDR_LEGACY → SM5 (`legacy_to_sm5`) | SSC writer uses `audio_sync_offset_seconds` + `tempo_segments` | Applied correctly (uniform) via the model offset |
| DDR → SM5 (`ddr_to_sm5`) | Source offset passed through | **Not applied** — silently ignored |

**Pre-existing defect (legacy → DDR):** because only `tempo_data[0]` moves,
the bias bends the first tempo segment instead of shifting the chart. For a
single-BPM song whose pairs are `(0, td0) … (END, tdN)`, the correction tapers
linearly from N ms at beat 0 to 0 ms at END. For multi-segment songs, only
notes inside the first segment move at all. Auto-sync needs a correct
"shift the whole timeline by δ" primitive, so this has to be fixed as part of
the feature, and it changes existing `--sync-offset-ms` output.

### Audio is uniformly available, except on one path

- Every decoder produces `AudioBuffer` (interleaved `i16`, 44.1/48 kHz)
  `src/model/audio.rs`: `ogg::decode::decode`, `xwb::parse_audio`,
  `wavm::parse`.
- Exception: legacy → DDR tries `try_audio_passthrough` first, byte-copying a
  compliant XWB + XSB without ever decoding (`src/job/mod.rs:545-587`).
  Auto-sync would need to decode for analysis only; the passthrough can
  survive because the correction is chart-side.

### The authoritative timing differs by output format

- SSQ output is driven by tempo *pairs*: synthesized from the model for SM5
  sources, raw (post-modernize) for legacy sources.
- The legacy tempo parser skips "instant advance" pairs when building
  `tempo_segments` (`src/ssq/tempo.rs:112-121`), so for legacy → DDR the
  semantic `tempo_segments` can differ from the pairs actually written. Note
  times used for analysis must come from the timing the output will carry.

### Constraints from steering

- No new top-level module without updating `.spec/steering/structure.md`
  ("categories above cover every concern") — DSP / onset analysis is a
  concern none of the existing modules own.
- New crates need a justification in the feature design
  (`.spec/steering/tech.md`, `CLAUDE.md`). No FFT or DSP crate is present
  today (`Cargo.toml`).
- Business rule 10: `--sync-offset-ms` is additive, a per-target engine bias —
  auto-sync must compose with it, not replace it.
- `tempo_data[0]` in stock DDR World SSQs is observed within ±22 ms
  (`.spec/steering/tech.md`); auto-sync could push it further negative (beat 0
  before audio start). SM5 → DDR already emits large positive values that play
  correctly in-game.
- There is no `tests/` directory yet; all current tests are unit tests,
  despite `CLAUDE.md` describing `tests/{from}_to_{to}.rs`.

## Prior art

- **+9ms or Null?** (<https://github.com/telperion/nine-or-null>, MIT). Uses the
  simfile's own timing data to locate every beat, cuts a spectrogram window
  around each, stacks them ("beat digest"), convolves with a rising-edge
  kernel, and takes the peak as the file's sync bias. Adds a confidence
  metric that penalizes strong response far from the chosen peak. The author
  cautions it is not millisecond-perfect because instrument attacks vary; it
  is built to decide between 0 ms and +9 ms, i.e. a narrow window.
- **ITG +9 ms bias** (<https://wiki.clubfantastic.dance/Sync#itg-offset-and-the-9ms-bias>).
  Community StepMania packs are commonly synced 9 ms early relative to null.
  SM5 → DDR conversions of those packs inherit the bias today.
- **ArrowVortex** — the maintainer's current manual workflow: align beat
  lines to the waveform by eye.

## What changes the idea

1. **Single-anchor alignment is fragile.** Snapping the first chart beat to
   the nearest detected waveform beat depends on one onset — often in
   silence, a fade-in, or a pickup note — with 10–20 ms detector jitter.
   Correlating the whole song's timing against the onset envelope uses
   hundreds of events and needs no tempo estimation: the chart already gives
   exact beat times, so the only unknown is one scalar.
2. **A beat grid is periodic; ±200 ms is wider than half a beat.** At 180 BPM
   a beat is 333 ms, so a true +150 ms error has an equally good alias at
   −183 ms inside the window. At 300 BPM the window spans two whole periods.
   Correlating against the chart's *note pattern* (rests, syncopation, breaks)
   is aperiodic and yields one peak — which is also the user's "analysis on
   the output steps" instinct.
3. **"In sync" needs a definition.** Onset detectors peak a few ms after the
   perceptual attack, and Konami's house sync may not be null. Measuring stock
   DDR World songs and using their median as the target absorbs both biases in
   one calibrated constant.
4. **The shift primitive is broken for legacy → DDR** (above), and the bias is
   ignored on two paths.
5. **Trust needs a report mode.** Measuring without changing anything is what
   calibrates the constant and lets the user audit a batch before applying.

## Unknowns

- Accuracy achievable on real DDR / Ultramix / SM-pack material.
- The value of the calibration constant, and whether stock songs cluster
  tightly enough to define one.
- Confidence thresholds that separate trustworthy from untrustworthy
  estimates.
- Whether DDR World tolerates `tempo_data[0]` well below −22 ms.
- Dependency footprint and Windows cross-compile behavior of an FFT crate.
