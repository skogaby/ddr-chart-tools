# Progress: validation-and-guidance

## Checklist

- [x] Cross-builds: `x86_64-pc-windows-gnu` and
  `x86_64-unknown-linux-musl` release builds succeed
  (logs/cross-*.log).
- [x] DSU batch report for both targets (logs/dsu-DDR.log,
  logs/dsu-SM5.log).
  - 44 paired songs: 16 accepted, 28 refused (23 `halves_disagree`).
  - DDR vs SM5 agree within 0.80 ms on every song both accept.
  - Findings: `.agents/planning/2026-09-28-auto-sync/research/legacy-validation.md`.
- [x] README examples and guidance use `--auto-sync`; the +53 ms is
  retired with measured figures. `product.md` glossary updated.
- [x] Gates: fmt, clippy and test clean.
- [x] Commit.

## Maintainer checks (need hardware or eyes)

1. **In-game, DDR World:** a chart whose first tempo point ends up below
   about −22 ms.
   - Candidate: `exot` from the DSU rip, `DDR_LEGACY → DDR --auto-sync`
     (correction −57 ms, so `tempo_data[0]` = −57).
   - Confirm it loads, starts, and feels in sync.
2. **In-game feel check:** two or three auto-synced conversions.
3. **ArrowVortex:** two or three `DDR_LEGACY → SM5 --auto-sync` outputs.
   Confirm the beat lines sit on the waveform attacks (the SM5 target
   assumption).

## Follow-ups found (not changed)

- **Legacy `tempo_data[0]`.** Zeroing it during modernization may discard
  real pre-roll for small values (see the research note).
- **`maxx`:** `DDR_LEGACY → SM5` places the chart 4 beats late, because
  the "instant advance" pair is dropped from `tempo_segments`.
- **Tempo drift** in some legacy charts (BPM off by about 0.02–0.05%),
  which a single offset cannot fix.

## Deviations

- None.

Status: Complete 869a854
