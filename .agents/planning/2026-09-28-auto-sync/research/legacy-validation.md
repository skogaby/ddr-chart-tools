# Legacy Validation: Dancing Stage Unleashed

Plan Step 6, run 2026-09-28 against the maintainer's full Dancing Stage
Unleashed rip. That is an Xbox title on the Ultramix engine: a flat
folder of `<id>_all.ssq` + `<id>.wavm` pairs, not in this repository.

Commands:

- `--from-format DDR_LEGACY --to-format {DDR,SM5} --input-folder <rip> --auto-sync report`
- Throwaway diagnostics under `prototypes/sync-probe/src/bin/`: `drift`,
  `timing_gap`, `wide`, `origin`.

## Results

| Measure | Value |
|---|---|
| Paired songs | 44 (68 unpaired files skipped: `_org` / `_kengo` variant charts, `_loop` previews) |
| Accepted (DDR target) | 16 — `report` 16, of which 1 is already in sync |
| Refused | 28 — `halves_disagree` 23, `at_search_edge` 2, `ambiguous_peak` 2, `beyond_cap` 1 |
| Accepted corrections | median +2.3 ms, std. dev. 28 ms, range −57 … +54 ms |
| DDR vs SM5 target | 16 songs accepted by both; max difference 0.80 ms, median 0.21 ms |
| Runtime | 27 s (DDR) and 36 s (SM5) for 44 songs, release build |

**No common bias.** The old `--sync-offset-ms 53` constant has no support
here. The README now recommends `--auto-sync` for Ultramix-era charts and
cites these figures.

## Why so many refusals

- **Genuine tempo drift.** Per-quarter estimates (`drift`) show steady
  linear drift in several songs, for example:
  - `bt4u`: +0.52 ms/s, which is 6 → 41 ms over the song;
  - `sprs`: +0.30 ms/s;
  - `san2`: −0.47 ms/s;
  - `kind`: −0.18 ms/s.

  The chart BPM differs from the audio's by roughly 0.02–0.05%. A single
  offset cannot fix that, so the split-half refusal is correct. BPM
  correction is out of scope (design); it is a possible follow-up.
- **Aliasing noise at wide windows.** Many other per-quarter series jump by
  exactly one beat or half a beat (`swee` 180 → −153 at 180 BPM; `secr`,
  `virt`, `heal`). This is the half-beat alias the ±60 ms cap exists to
  avoid, seen here with a ±200 ms per-quarter search. It is not evidence
  about the ±60 ms estimate.
- **Search edge.** `blas` and `grad` sit just past +70 ms with the default
  cap.

## Open question for the maintainer: legacy `tempo_data[0]`

`ssq_legacy::modernize` shifts every anchor so `tempo_data[0]` becomes 0.
Its comment calls the value "a small authoring-tool quirk". Comparing
corrections with the value zeroed (today) and kept (`origin`):

- **Small values** (|`tempo_data[0]`| < 200 ms, 12 songs accepted either
  way).
  - Zeroed: corrections spread from −54 to +54 ms, with outliers such as:
    - `lcan` (`tempo_data[0]` = +53 ms): +54 ms correction;
    - `rebi` (−27 ms): −45 ms correction.
  - Kept: these land with the rest (`lcan` +1.4, `rebi` −17.9). Without
    `secr`, which is quarter-beat ambiguous, the kept corrections sit in
    −28 … +6 ms.
  - This suggests that small values are real audio pre-roll that
    modernization discards.
- **Large values** (0.3–7.7 s, e.g. `soin` 827 ms, `infi` 3.4 s): zeroing
  is clearly right. `soin` measures exactly in sync (+0.1 ms) with the value
  zeroed, and keeping multi-second values would move charts by seconds.

This is plausibly where the old "+53 ms" folklore came from: many TPS 75
charts carry `tempo_data[0]` = 4 ticks = 53.3 ms.

Deciding the semantics changes existing, business-rule-backed behavior
(rule 4), so this was **not changed**. Recorded as a follow-up for the
maintainer.

## Other findings (pre-existing, not changed)

- **`maxx` (`DDR_LEGACY → SM5`) is 4 beats late in the SSC.** Its tempo
  chunk opens with an "instant advance" pair `(0, 4) → (4096, 4)` (beats
  0–4 in zero time). The SSQ output keeps it. The tempo parser drops it
  from `tempo_segments`, so the SSC output places every note 800 ms (4
  beats at 300 BPM) later than the SSQ output.
  - Auto-sync cannot see this, because the chart is periodic at 300 BPM.
  - This is the only such song in the rip (`timing_gap`: 1 of 51 charts
    with an SSQ-vs-SSC gap > 1 ms).
- **Chart-vs-model timing otherwise agrees within 0.67 ms** on every other
  chart. That is the TPS 75 rescale rounding.
