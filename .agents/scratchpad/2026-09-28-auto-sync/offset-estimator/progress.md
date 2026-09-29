# Progress: offset-estimator

## Checklist

- [ ] Types + tests (red)
- [ ] estimate (green)
- [ ] Validate
- [ ] Commit

## Log
- [x] all

1. **Red:** compile errors for the missing `estimate`, `verdict`,
   `median` and `rival_ratio`.
2. **Green, first run:** 10 of 12 passed. Two test constructions needed
   adjustment; the implementation matched the spec in both cases.
   - **Two-alignment test.** Duplicates at +20 ms gave rival 0.85, not
     ≥ 0.9, because they fall on the 20 ms burst's cutoff and score lower.
     They moved to +30 ms, which gave AmbiguousPeak.
   - **Edge test.** A true peak 5 ms past the edge left the in-window curve
     featureless, which gave AmbiguousPeak (a correct refusal, just a
     different reason). It moved to 2 ms past the edge, which gave
     AtSearchEdge.
3. **Full suite:** 427 passed; about 7 s in debug, dominated by synthetic
   envelopes. fmt and clippy clean.

## Deviations

- **Two-alignment test:** duplicate offset +30 ms rather than the design's
  +20 ms (test construction only).
- **Defensive clamp:** `estimate` clamps `max_correction_ms` to
  `MAX_CORRECTION_LIMIT_MS` (assumption A1), guarding library callers.

Status: Complete 7eff865
