# Task: Calibrate `TARGET_OFFSET_MS` against stock DDR World

## Description

Add an ignored, environment-gated calibration test. It runs the production
estimator over the stock DDR World catalogue and the community offset
database, derives `TARGET_OFFSET_MS`, and checks agreement. Run it, set the
constant from its result, and document the calibration gotchas.

## Background

- **What the constant is.** `TARGET_OFFSET_MS` is the measured offset of an
  in-sync chart for this exact detector. It is currently −2.10 ms, the
  throwaway prototype's value. The production estimator follows the same
  specification, but small implementation differences (event margins,
  exact-beat vs tick-rounded note times) can move it by a fraction of a
  millisecond.
- **The reference.** The community database gives, for each song, the
  play-validated amount to add to `#OFFSET`: `c`. So a well-calibrated
  target satisfies `T = median(m + c)`.
- **The corpus.** Calibrate on TPS 1000 songs only. DDR World rounds tempo
  anchors to whole milliseconds (verified in the executable), so for TPS
  1000 exact timing and game timing coincide.
- **Prototype reference numbers** on the 711 TPS 1000 songs:
  - `T = −2.10`;
  - 94.0% within 1 ms and 97.6% within 2 ms of the community value;
  - 95.6% of all songs accepted.

## Reference Documentation

**Required:**
- Design: `.agents/planning/2026-09-28-auto-sync/design/detailed-design.md`
  (R13; *Testing Strategy → Calibration test*; *Documentation Changes* for
  `tech.md` and `structure.md`; Appendix A.1, A.2, A.5)

**Additional References (if relevant to this task):**
- `.agents/planning/2026-09-28-auto-sync/research/calibration-corpus.md`:
  corpus layout, CSV semantics and sign.
- `.agents/planning/2026-09-28-auto-sync/research/prototype-results.md`
  §8–9: the prototype's numbers to compare against.

**Note:** Read any document listed above before beginning implementation.

## Technical Requirements

1. **`tests/auto_sync_calibration.rs`.** A single `#[test] #[ignore]`
   function.
   - Reads `DDR_WORLD_INSTALL` and `DDR_SYNC_OFFSETS_CSV`. If either is
     unset, prints why and returns.
   - Corpus: every CSV row with a value whose
     `$DDR_WORLD_INSTALL/data/mdb_apx/ssq/<code>.ssq` parses with TPS 1000
     and whose `$DDR_WORLD_INSTALL/data/sound/win/dance/<code>.xwb`
     decodes.
   - Missing, non-1000-TPS, unparsable or undecodable songs are counted and
     skipped.
2. **Public API only:**
   - `ssq::parse`;
   - `xwb::parse_audio`;
   - `sync::TimeMap::from_tempo_pairs(&raw_tempo_pairs, tps)`;
   - `sync::chart_events(&song.charts, &map)`;
   - `sync::estimate(.., &Params::default())`.

   Parse the CSV by hand: header `code,p1_offset,p2_offset`, value in
   column 2, blank means unset.
3. **Statistics.**
   - `T_measured = median(m + c)` over songs with `measured_ms`;
     `e = m + c − T_measured`.
   - The percentage within 0.5, 1, 2 and 3 ms.
   - The accepted fraction (`Apply` or `Unchanged`) and the refusal
     counts.
   - The worst 10 songs by `|e|`.
4. **Asserts.**
   - `|T_measured − TARGET_OFFSET_MS| ≤ 0.1`; on failure the message names
     the value to set.
   - ≥ 90% within 1 ms and ≥ 96% within 2 ms.
   - Acceptance ≥ 93%.
   - At least 500 songs measured, so a broken path cannot pass vacuously.
5. **Run and set the constant.** Run it in a release build. Set
   `TARGET_OFFSET_MS` to `T_measured` rounded to 0.01 ms, and rewrite its
   doc comment: drop "provisional" and record the corpus size, `T`, the
   within-1/2 ms agreement, the acceptance, and the date. Re-run to confirm
   it passes.
6. **Parity gate.** If agreement is materially below the prototype's (for
   example below 92% within 1 ms), stop and investigate before continuing
   to Step 4.
7. **`.spec/steering/tech.md` gotchas.**
   - DDR World's integer-ms anchor normalization (`step::SsqReader`
     prepare: `floor(f32(int32(v·1000)) / f32(tps) + 0.5f)`, identical in
     the 2025-08-05, 2026-08-25 and 2026-09-15 builds).
   - Half-beat aliasing is the reason for the ±60 ms cap.
   - `TARGET_OFFSET_MS` is detector-specific: recalibrate after any onset
     change.
   - The community CSV's `c` is the amount to add to `#OFFSET`, so
     `T = median(m + c)`.
8. **`.spec/steering/structure.md`.** Note that
   `tests/auto_sync_calibration.rs` is the one test named for what it
   validates rather than a conversion direction.

## Dependencies

- Step 2 (`src/sync/` public API).
- A local DDR World install and the community CSV, present on the
  maintainer's machine. Neither is committed; the test skips without them.

## Implementation Approach

1. Write the test and run it against the provisional constant; it passes
   or fails on the T check.
2. Update the constant from the measurement and re-run until it passes.
3. Add the docs.
4. Run `cargo fmt`, clippy, and the default (non-ignored) tests.

## Acceptance Criteria

1. **Skips cleanly without data**
   - Given `DDR_WORLD_INSTALL` or `DDR_SYNC_OFFSETS_CSV` unset
   - When `cargo test --test auto_sync_calibration -- --ignored` runs
   - Then it prints why and passes

2. **Calibrates on the real catalogue**
   - Given both variables pointing at the stock install and the CSV
   - When run in release with `--ignored --nocapture`
   - Then it prints the corpus size, `T`, the agreement and acceptance, and
     passes with the updated constant

3. **Parity with the prototype**
   - Given the run above
   - When agreement is compared with the prototype
   - Then within-1 ms is ≥ 92% and within-2 ms is ≥ 96%, or the step stops
     for investigation

4. **Constant documented**
   - Given the updated `TARGET_OFFSET_MS`
   - When its doc comment is read
   - Then it states the corpus, the value, the agreement and the date, and
     is no longer marked provisional

5. **Steering updated**
   - Given `tech.md` and `structure.md`
   - When read
   - Then the four gotchas and the test-naming exception are present

6. **Quality gates**
   - Given the change
   - When `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and
     `cargo test` run
   - Then `fmt` produces no diff and both others pass (the calibration test
     is ignored by default)

## Metadata
- **Complexity**: Medium
- **Labels**: sync, calibration, testing, docs
- **Required Skills**: Rust, statistics
- **Generated By**: code-task-generator 2026-09-28
- **Source Plan**: .agents/planning/2026-09-28-auto-sync/implementation/plan.md
- **Plan Step**: Step 3: Calibration against stock DDR World
