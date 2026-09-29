# Onset Detection for Offset Estimation

What the estimator correlates chart event times against (D5, D6), and which
feature to compute from the audio. Prototype comparison results are in
`prototype-results.md`.

## Requirements the feature must meet

- **Time precision over detection accuracy.** We never need to decide
  *whether* an onset exists at a given place; we sum a continuous
  onset-strength envelope at hundreds of chart times. What matters is that
  the envelope's rise is sharp and consistently placed relative to the
  attack, so per-event jitter averages out.
- **Constant bias is fine.** Any fixed lag between the envelope peak and the
  perceived attack is absorbed by the calibrated target `T` (D7).
- **Robust to sustained and vibrato content.** Held synth pads, vocals, and
  vibrato produce spectral change that is not rhythmic.

## Candidates

### Spectral flux (log-magnitude)

Frame the mono signal with a Hann window, take `L(t,k) = log(1 + γ|X(t,k)|)`,
and sum the positive frame-to-frame differences over frequency:

```
flux(t) = Σ_k w_k · max(0, L(t,k) − L(t−1,k))
```

Dixon's comparison of onset detection functions found spectral flux among the
best performers while being the simplest (S. Dixon, "Onset Detection
Revisited", Proc. DAFx-06, Montreal, 2006). Log compression makes quiet
attacks count.

### SuperFlux

Spectral flux on a log-frequency filterbank, with two changes: the reference
frame is `μ > 1` frames back instead of 1, and it is maximum-filtered across
neighboring frequency bins before the difference is taken, so vibrato that
moves energy between adjacent bins produces no positive flux
(S. Böck and G. Widmer, "Maximum Filter Vibrato Suppression for Onset
Detection", Proc. DAFx-13, Maynooth, 2013). Published parameters: 2048-sample
frames at 200 frames/s, 24 bands per octave, maximum filter 3 bands wide.

### +9ms or Null? rising edge

From <https://github.com/telperion/nine-or-null>
(`nine-or-null/nine_or_null/__init__.py`, `check_sync_bias`):

- Hann-windowed spectrogram with a **10 ms window and 0.2 ms step**, in
  `log2` power.
- Frequency emphasis `w(f) = f · e^(−f / 3000 Hz)`, which peaks at 3 kHz and
  suppresses sub-bass rumble and hiss.
- For each beat of the simfile's own timing (skipping beats less than 50 ms
  apart), a ±50 ms window of the spectrogram is cut out, flattened over
  frequency, and stacked (the "beat digest").
- The digest is convolved with a rising-edge kernel `[1, 1, 0, −1, −1]`, the
  response summed across beats, and the argmax taken as the bias.
- Two unexplained fudge terms exist in the code: `spectrogram_offset =
  sqrt(0.5) · window` ("trying to figure out why this isn't half a window")
  and a user-settable `magic_offset_ms` ("might be related to attack
  perception?"). Both are the detector bias D7 calibrates away empirically.
- Confidence: normalize the response to [0, 1], and penalize every point by
  `v⁴ · d^1.5`, where `v` is its height above the median relative to the peak
  and `d` its distance from the peak beyond a nearness margin.

Because the window is only ±50 ms around each beat, the method never sees the
aliasing problem D6 addresses; it is built to decide between 0 and +9 ms.

## What the prototype compares

1. Onset feature: plain log spectral flux vs. a SuperFlux-style max-filtered
   flux vs. a short-window rising-edge feature in the style of +9ms or Null?.
2. Template: chart note pattern (D6) vs. regular beat grid.
3. Confidence signals: peak z-score, strongest rival peak, split-half
   disagreement (D17) — judged by whether they predict large residuals
   against the community offsets.
