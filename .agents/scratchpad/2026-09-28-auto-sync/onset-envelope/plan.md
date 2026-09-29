# Plan: onset-envelope

Status: Approved 2026-09-28. This rests on the upstream approvals and the
maintainer's standing approval. Auto mode.

## Test scenarios (`src/sync/onset.rs`)

1. **`click_train_peaks_follow_clicks_with_constant_lag`**
   - Input: 20 clicks at irregular times (base 0.3 s, gaps
     0.21–0.37 s), each 1 ms (44 samples) of amplitude 0.8, in 6.5 s of
     44.1 kHz stereo.
   - For each click, the refined envelope maximum in [click − 15 ms,
     click + 15 ms] minus the click time forms `lag_i`.
   - Assert: `|lag_i − median| ≤ hop` for all clicks, and
     `|median| < 10 ms`.
2. **`sample_rate_does_not_move_onsets`**
   - The same click times at 48 kHz.
   - Assert: `|median_48k − median_44k| ≤ 0.5 ms`.
3. **`silence_is_flat`**
   - Input: 2 s of zeros.
   - Assert: every value is exactly 0.
4. **`degenerate_audio_has_no_envelope`**
   - Inputs: empty buffer; rate 0; channels 0; 100 samples.
   - Assert: `None` for each.
5. **`sample_interpolates_and_is_zero_outside`**
   - An envelope built by hand from `values = [0,1,2]`, `t0 = 1.0`,
     `dt = 0.5`.
   - Assert: `sample(1.25) = 0.5`; `sample(0.9) = 0`; `sample(2.0)` (the
     last frame) `= 0`.

## Implementation

A single pass over frames:

1. Window and zero-pad into a reused `Vec<Complex32>`.
2. FFT in place, with scratch.
3. Compute `E`.
4. Push `E` into an `energies` vec.
5. Output `E[t] − E[t−L]`.

`Envelope::new` (test-only) builds the hand-made envelope for scenario 5.
