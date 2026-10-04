# Tasks: 20261003-hudson-ssq-support

- [x] 1. Reverse-engineer type 9 parsing and gimmick handling in the HP1 DOL; write `docs/hudson_ssq_format.md`.
- [x] 2. `src/ssq/hudson.rs`: bounds-checked decoder for header, rows, `EXDT`/`CHAR`/`FREZ` (unit tests on synthetic bytes).
- [x] 3. Gimmicks-off neutralizer `HudsonChart` → `Chart` (unit tests per item rule, echo cap, freeze resolution).
- [x] 4. Wire type 9 dispatch in `ssq::parse` (difficulty-code gate; arcade metadata still dropped).
- [x] 5. `src/wav/`: RIFF PCM decoder; `Error::Wav`; legacy audio sniffing; `.wav` in `DDR_LEGACY` audio extensions; 32 kHz in `DDR_SAMPLE_RATES`.
- [x] 6. Integration test `tests/hudson_legacy_to_ddr.rs` (synthetic SSQ + WAV → DDR, re-parse output).
- [x] 7. Update `structure.md`, `product.md`, `README.md`, and the `auxiliary.rs`/`ssq/mod.rs` docs.
- [x] 8. Manual end-to-end batch over `~/Desktop/hottest_party_songs`.
