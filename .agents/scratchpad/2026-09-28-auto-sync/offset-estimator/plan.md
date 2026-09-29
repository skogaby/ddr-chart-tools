# Plan: offset-estimator

Status: Approved 2026-09-28. This rests on the upstream approvals and the
maintainer's standing approval. Auto mode.

## Test scenarios (`src/sync/estimate.rs`)

Helper `song(shift_ms, split_shift_ms)` returns the audio and the burst
event list, with bursts placed at `event + shift` (and an extra
`split_shift` for the second half of the events). `m₀ =
estimate(song(0)).measured_ms`.

1. **`recovers_audio_shifts`**
   - For each X ∈ {−55, −30, −7, 1, 13, 40, 55}:
     `|measured(X) − m₀ − X| ≤ 0.5`.
2. **`refuses_too_few_events`**
   - 20 events → `TooFewEvents`, with measured fields `None`.
3. **`refuses_without_audio`**
   - Empty buffer → `NoAudio`.
4. **`refuses_peak_at_search_edge`**
   - cap 30; bursts shifted so the measured value would be about 45 →
     `AtSearchEdge`, with `measured_ms` `Some`.
5. **`refuses_beyond_cap`**
   - cap 30; bursts shifted so the correction is about 35 → `BeyondCap`,
     with `correction_ms ≈ 35 ± 0.5`.
6. **`refuses_two_equal_alignments`**
   - Events duplicated at +20 ms, same weight → `AmbiguousPeak`, rival
     ≥ 0.9.
7. **`refuses_when_halves_disagree`**
   - Second half of the bursts +10 ms → `HalvesDisagree`, split ≈ 10.
8. **`in_sync_chart_is_unchanged`**
   - Events moved by `m₀ − T` → `Unchanged`.
9. **`offset_chart_gets_rounded_correction`**
   - Events moved by `m₀ − T − 7` → `Apply { delta_ms: 7 }`.
10. **`verdict_rounds_half_away_and_caps`**
    - +0.49 → `Unchanged`; +0.5 → 1; −0.5 → −1; −1.49 → −1; 60.0 @cap 60
      → `Apply(60)`; 60.2 @cap 60 → `BeyondCap`; NaN → `BeyondCap`.

## Implementation

Free functions in `estimate.rs`:

- `score_curve(env, events, lo_ms, len)`
- `peak(curve) -> (usize, f64)` (argmax plus parabolic refinement)
- `median`
- `rival_ratio(curve, index)`
- `verdict(correction_ms, cap_ms)`

`estimate` composes them. Every comparison uses `total_cmp`.
