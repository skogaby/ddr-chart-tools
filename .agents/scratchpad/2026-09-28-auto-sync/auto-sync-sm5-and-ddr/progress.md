# Progress: auto-sync-sm5-and-ddr

## Checklist

- [ ] Unit tests (red)
- [ ] Types/CLI/orchestration/wiring (green)
- [ ] Integration tests
- [ ] Docs
- [ ] Validate
- [ ] Commit

## Log
- [x] all

1. **Red (unit):** compile errors for the missing `AutoSync`,
   `AutoSyncMode`, CLI variants, `decide` and `Refusal::key`.
2. **Green (unit):** 443 passed.
3. **Wiring:** `auto_sync_delta` plus `shift_timeline` on `sm5_to_ddr`
   (from the final pairs) and `ddr_to_sm5` (`from_song`).
4. **Integration tests:** 8, all passing.
   - They were written after the wiring. Red was verified by making
     `auto_sync_delta` return 0: 5 of 8 failed.
   - The 3 that passed are invariant checks (report inert on DDR→SM5, and
     bias composition), which hold for a no-op too.
5. **Stock spot check in report mode:**
   - puty: +11 ms (community +11).
   - hane: +12 ms; the community lists 0 (a known disagreement).
   - rint: refused, `ambiguous_peak`, rival 0.96.
6. **Docs:** `product.md` (glossary and rule 13), `structure.md` (`job/`
   row), README (flags and an Auto-sync section).
7. **Gates:** fmt and clippy clean; `cargo test` passes in about 21 s.

## Deviations

- **Internal orchestration signature.** It is `auto_sync_delta(...) -> i32`
  followed by `shift_timeline`, rather than a single function taking the
  song and the audio. This avoids a borrow conflict, and behavior is as
  designed.

Status: Complete e0c126b
