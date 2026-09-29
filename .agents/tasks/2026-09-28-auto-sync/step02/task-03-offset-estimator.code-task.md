# Task: Offset estimator and public `sync::estimate` API

## Description

Implement the estimator. It correlates the onset envelope against weighted
chart events and finds the refined peak, then decides whether and how far
to move the chart:

- check the rival-peak and split-half consistency;
- refuse on untrustworthy input;
- round the correction to whole milliseconds.

Expose it as `sync::estimate` with the design's result types.

## Background

- **Measured offset.** `m` = audio onset time − chart event time, in ms. A
  chart is "in sync" when `m` equals the calibrated `TARGET_OFFSET_MS`. The
  correction is `m − T`; a positive value moves the chart later.
- **Why the checks exist.** Research over the stock catalogue showed three
  failure modes:
  - Half-beat aliasing; the ±60 ms cap removes it.
  - Bistable songs, where two alignments 22–37 ms apart score within 7% of
    each other; the rival gate handles these.
  - Drift or cuts; the split-half gate handles these.

  Refusing leaves the source sync untouched. The estimator never returns an
  error.

## Reference Documentation

**Required:**
- Design: `.agents/planning/2026-09-28-auto-sync/design/detailed-design.md`
  (R4–R8; *Estimator algorithm* items 3–8 and the sign conventions; *Data
  Models*: `Params`, `SyncEstimate`, `Outcome`, `Refusal`, constants;
  *Testing Strategy → `sync::estimate`*)

**Additional References (if relevant to this task):**
- `.agents/planning/2026-09-28-auto-sync/research/prototype-results.md`
  §5 and §8: why the thresholds are what they are.

**Note:** Read any document listed above before beginning implementation.

## Technical Requirements

1. **Public types in `src/sync/mod.rs`.** `Params { max_correction_ms: u32
   }` (with a `Default` using `DEFAULT_MAX_CORRECTION_MS`), `SyncEstimate`,
   `Outcome`, `Refusal`, all as specified in *Data Models*.
2. **`pub fn estimate(audio: &AudioBuffer, events: &[SyncEvent], params:
   &Params) -> SyncEstimate`**, implemented in `src/sync/estimate.rs`,
   following *Estimator algorithm* 3–8 exactly:
   - Search half-width `H = cap + SEARCH_MARGIN_MS`. Keep only events at
     least `H/1000 + 0.1` s inside both envelope ends.
   - Score every integer `d` in `[round(T) − H, round(T) + H]`.
   - Take the argmax and refine it parabolically.
   - Edge: the peak index is within `EDGE_MARGIN_SAMPLES` of either end.
   - Rival: over local maxima more than `RIVAL_EXCLUSION_MS` from the peak,
     relative to the median; 1.0 when the peak is ≤ the median.
   - Split halves: sort by time, split at `n/2`, take each half's refined
     argmax on its own curve.
   - Verdict:
     - `correction = m − T`;
     - `|correction| > cap` → `BeyondCap`;
     - otherwise round half away from zero: 0 → `Unchanged`, else
       `Apply { delta_ms }`.
3. **Refusal order.** `NoAudio`, `TooFewEvents` (fewer than `MIN_EVENTS`
   usable), `AtSearchEdge`, `AmbiguousPeak` (rival ≥ 0.9), `HalvesDisagree`
   (> 5 ms), `BeyondCap`. The first applicable one is reported.
4. **Fields.** `measured_ms`, `correction_ms`, `rival` and `split_ms` are
   populated whenever a score curve was computed, including for the
   post-measurement refusals; they are `None` only for `NoAudio` and
   `TooFewEvents`. `events_used` counts the usable events.
5. **Verdict and rounding** are factored into a small pure function so the
   rounding edges can be unit-tested directly.
6. **No panics.**
   - No `unwrap`/`expect` in non-test code.
   - Handle NaN-free `f32`/`f64` comparisons without panicking: use
     `total_cmp` or explicit folds.
7. **Docs.** `///` on every public item. Document the sign convention on
   `SyncEstimate`.

## Dependencies

- Tasks 01 (`SyncEvent`, constants) and 02 (`envelope`) of this step.

## Implementation Approach

1. Write the tests first. Synthetic signals, deterministic (a small
   xorshift PRNG in test code): decaying 20 ms noise bursts at event times
   plus low-level background noise, 44.1 kHz stereo, about 10–12 s, with an
   irregular event pattern.
   - **Relative recovery.** With `m₀` the measured offset on unshifted
     audio, shifting the bursts by X ∈ {−55, −30, −7, +1, +13, +40, +55} ms
     moves `measured_ms` by X ± 0.5 ms.
   - **One constructed case per refusal:**
     - fewer than 32 events → `TooFewEvents`;
     - empty audio → `NoAudio`;
     - true offset just beyond `cap + margin` → `AtSearchEdge`;
     - true correction between cap and `cap + margin` → `BeyondCap`;
     - events duplicated at +20 ms, equal weight → `AmbiguousPeak`;
     - the second half of the bursts shifted +10 ms relative to the first
       → `HalvesDisagree`.
   - **Verdict and rounding:** +0.49 → `Unchanged`; +0.5 → `Apply(1)`;
     −0.5 → `Apply(−1)`; −1.49 → `Apply(−1)`; beyond the cap →
     `BeyondCap`.
   - **An in-sync source:** events equal to the burst times shifted by
     `T − m₀` give `Unchanged`.
2. Implement.
3. Measure the debug-build test runtime; keep the signals short enough that
   `cargo test` stays quick.
4. Run `cargo fmt`, clippy, and the tests.

## Acceptance Criteria

1. **Relative recovery**
   - Given a synthetic song and its baseline measurement `m₀`
   - When the audio is shifted by X ∈ {−55, −30, −7, +1, +13, +40, +55} ms
   - Then `measured_ms − m₀` equals X within 0.5 ms, for each X

2. **Every refusal reason is reachable and ordered**
   - Given each constructed refusal case
   - When `estimate` runs
   - Then the outcome is the specified `Refused(reason)`, measured fields
     are present for post-measurement refusals, and absent for `NoAudio`
     and `TooFewEvents`

3. **Rounding and cap**
   - Given corrections +0.49, +0.5, −0.5, −1.49, and one beyond the cap
   - When the verdict function runs
   - Then the outcomes are `Unchanged`, `Apply(1)`, `Apply(−1)`,
     `Apply(−1)`, and `Refused(BeyondCap)`

4. **Already in sync**
   - Given events placed so the correction is within ±0.5 ms
   - When `estimate` runs
   - Then the outcome is `Unchanged`

5. **Never panics**
   - Given any of the above inputs, including empty events
   - When `estimate` runs
   - Then it returns a `SyncEstimate`

6. **Quality gates**
   - Given the change
   - When `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and
     `cargo test` run
   - Then `fmt` produces no diff and both others pass

## Metadata
- **Complexity**: High
- **Labels**: sync, estimator, dsp
- **Required Skills**: Rust, signal correlation, numerical robustness
- **Generated By**: code-task-generator 2026-09-28
- **Source Plan**: .agents/planning/2026-09-28-auto-sync/implementation/plan.md
- **Plan Step**: Step 2: `src/sync/` estimator on synthetic audio
