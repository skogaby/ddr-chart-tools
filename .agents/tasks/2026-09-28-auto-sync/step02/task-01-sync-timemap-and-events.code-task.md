# Task: `src/sync/` skeleton, `TimeMap`, and chart events

## Description

Create the new top-level `src/sync/` module. Implement its two
timing-side building blocks:

- `TimeMap`: piecewise-linear beat → audio-seconds, built either from SSQ
  tempo pairs or from the model's semantic timing.
- `chart_events`: the weighted union of chart note positions the estimator
  correlates against.

Also create the module's constants table and register the module in the
steering documents.

## Background

- **Why a new module.** Auto-sync measures where a chart's notes fall
  against the audio's rhythmic onsets. That concern belongs to no existing
  module:
  - `model/` holds types and invariants;
  - `util/` excludes domain logic;
  - `job/` is orchestration;
  - the format modules own byte layouts.

  The approved design overrides the "no new top-level module" steering rule
  for `src/sync/`, provided `.spec/steering/structure.md` is updated in the
  same change.
- **Which timing to measure against.** Each output must be measured against
  the timing it will actually carry:
  - SSQ outputs: the final `(measure_tick, tempo_data)` pairs, at TPS 1000
    (so ms).
  - SSC outputs: `Song::tempo_segments`, `Song::stops` and
    `Song::audio_sync_offset_seconds`, which uses the DDR sign convention
    (audio time of beat 0).
- **How `from_song` must walk.** The same way the SSQ writer does, in
  `ssq::writer::synthesize_tempo_entries_until` (`src/ssq/writer.rs`):
  - the earliest `#BPMS` entry applies from beat 0;
  - segment boundaries and stops merge in beat order, segments first on a
    tie;
  - a stop is two anchors at the same beat.

## Reference Documentation

**Required:**
- Design: `.agents/planning/2026-09-28-auto-sync/design/detailed-design.md`
  (*Components and Interfaces → Sync module*, *`TimeMap` semantics*; R5;
  *Data Models* constants table; *Testing Strategy → `sync::timemap`,
  `sync::events`*; *Documentation Changes* for `structure.md`)

**Additional References (if relevant to this task):**
- `docs/ssq_format.md` §3: tempo pairs and stops.
- `.spec/steering/structure.md`: the layout and responsibilities table to
  update.

**Note:** Read any document listed above before beginning implementation.

## Technical Requirements

1. **Module files.**
   - `src/sync/mod.rs`: a `//!` header saying what the module owns (onset
     analysis and chart-vs-audio offset estimation, as pure computation)
     and what it does not (I/O, format parsing, applying corrections).
   - Submodules `timemap` and `events`.
   - `pub mod sync;` in `src/lib.rs`.
2. **Constants.** Add the design's *Data Models* constants to `mod.rs`,
   each with a `///` comment stating its origin. Mark `TARGET_OFFSET_MS` =
   −2.10 as provisional until the calibration step. The onset DSP constants
   may be added now or in the envelope task, whichever keeps them in one
   place.
3. **`SyncEvent { time_s: f64, weight: f64 }`.** Public; derives `Debug,
   Clone, Copy, PartialEq`.
4. **`TimeMap`.** Public, anchors `(beat: f64, seconds: f64)`, with:
   - `from_tempo_pairs(pairs: &[(i32, i32)], tps: u32) -> Option<TimeMap>`:
     beat = tick / 1024, seconds = tempo_data / tps. `None` if `tps == 0`,
     there are fewer than two anchors, ticks decrease anywhere, or no pair
     of anchors has a nonzero beat span.
   - `from_song(song: &Song) -> Option<TimeMap>`: mirrors the writer walk
     in `f64`, then appends one trailing anchor one measure (4 beats) past
     the last anchor at the tempo in force, so extrapolation past the final
     BPM change uses that tempo. `None` if there are no tempo segments or
     any BPM ≤ 0.
   - `seconds_at(&self, beat: f64) -> f64`:
     - a beat equal to a stop's beat returns the stop's *first* anchor;
     - between anchors, linear interpolation;
     - before the first anchor or past the last, extrapolate from the
       nearest segment with a nonzero beat span.
5. **`chart_events(charts: &[Chart], map: &TimeMap) -> Vec<SyncEvent>`.**
   - Union of note beats over all charts; weight = number of charts with a
     counted note at that exact beat (`Beat` equality).
   - Taps, hold heads and shocks count; mines do not.
   - A chart contributes at most once per beat.
   - Output sorted by beat.
6. **No panics.** No `unwrap`/`expect` in non-test code. `///` on every
   public item.
7. **`structure.md`.** Add `sync/` to the layout block and the
   responsibilities table. Owns: onset analysis, offset estimation,
   beat→time mapping for analysis. Does not own: I/O, format parsing,
   applying corrections.

## Dependencies

- None new; no FFT yet.
- Uses `crate::model::{Song, Chart, NoteKind, Beat, Rational}`.
- Tests may use `ssq::writer::synthesize_tempo_entries_until` to build
  comparison pairs.

## Implementation Approach

1. Write the tests first:
   - `timemap`: single BPM; BPM change; a stop (a note on the stop maps to
     the stop start, a note after it includes the stop); extrapolation
     before and after; decreasing ticks → `None`; `from_song` vs
     `from_tempo_pairs` agreement within 1 ms on the same song.
   - `events`: charts-not-notes weighting; mines excluded; tap + hold head
     on one beat in one chart counts once; output ordered.
2. Implement `TimeMap` and `chart_events`.
3. Add the constants and docs, and update `structure.md`.
4. Run `cargo fmt`, clippy, and the tests.

## Acceptance Criteria

1. **Pair-built map places beats correctly**
   - Given pairs `(0, 0), (4096, 2000)` at TPS 1000
   - When `seconds_at(2.0)` is called
   - Then it returns 1.0

2. **Stops**
   - Given pairs with a stop `(8192, 4000), (8192, 4500)`
   - When `seconds_at` is called at the stop's beat and just after it
   - Then the stop's beat returns 4.0 s, and a beat one beat later includes
     the 0.5 s stop

3. **Extrapolation uses the right tempo**
   - Given a song whose last BPM change is at beat 16
   - When `from_song(..).seconds_at(40.0)` is evaluated
   - Then it matches the SSQ writer's synthesized pairs at beat 40 within
     1 ms, using the final BPM, not the previous one

4. **Model and pair maps agree**
   - Given a song with two segments and a stop, and its pairs synthesized
     by `synthesize_tempo_entries_until`
   - When both maps are evaluated at every beat 0…40
   - Then they agree within 1 ms

5. **Event weights count charts**
   - Given two charts both with a note at beat 4, one of them with two
     notes (tap + hold head) at beat 4, and a mine at beat 5
   - When `chart_events` runs
   - Then beat 4 has weight 2, and beat 5 is absent

6. **Malformed timing is rejected**
   - Given decreasing ticks, a single pair, TPS 0, or a song with no
     segments
   - When a map is built
   - Then `None` is returned

7. **Module registered and documented**
   - Given the change
   - When `structure.md` and `src/lib.rs` are inspected
   - Then `sync/` is listed with its responsibilities and exported

8. **Quality gates**
   - Given the change
   - When `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and
     `cargo test` run
   - Then `fmt` produces no diff and both others pass

## Metadata
- **Complexity**: Medium
- **Labels**: sync, timing, model
- **Required Skills**: Rust, SSQ tempo semantics
- **Generated By**: code-task-generator 2026-09-28
- **Source Plan**: .agents/planning/2026-09-28-auto-sync/implementation/plan.md
- **Plan Step**: Step 2: `src/sync/` estimator on synthetic audio
