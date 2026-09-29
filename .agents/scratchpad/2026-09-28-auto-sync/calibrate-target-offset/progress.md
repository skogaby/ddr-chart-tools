# Progress: calibrate-target-offset

## Checklist

- [ ] Calibration test
- [ ] Release run + constant
- [ ] Docs
- [ ] Validate
- [ ] Commit

## Log
- [x] all

1. The skip path prints its message and passes (logs/skip.log).
2. Release run against the provisional −2.10 (logs/run1.log, 151 s):
   - 711 measured.
   - Skipped: 723 not TPS 1000, 1 missing, and 6 unreadable (the known
     parser/XWB limits: ifly, lich, hart, mixn, itiz, souv).
   - T = −2.107.
   - Within 0.5 / 1 / 2 / 3 ms: 73.0% / 94.1% / 97.6% / 98.3%.
   - Accepted 99.3% (5 HalvesDisagree).
   - **Parity with the prototype:** 94.0% / 97.6%. Passed.
3. Set TARGET_OFFSET_MS = −2.11 with a documented calibration record;
   re-ran and passed (logs/run2.log).
4. `tech.md` gains the 4 gotchas; `structure.md` gains the naming
   exception.
5. fmt and clippy clean; `cargo test` passes with the calibration test
   ignored.

## Deviations

- None. Acceptance of 99.3% is higher than the prototype's 95.6%, which was
  measured over all songs including TPS 150 and used a wider event margin.

Status: Complete 749f165
