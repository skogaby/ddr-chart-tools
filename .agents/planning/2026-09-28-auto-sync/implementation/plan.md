# Auto-Sync: Implementation Plan

Status: Approved 2026-09-28

Implements `design/detailed-design.md`; section references (R1–R13,
"Estimator algorithm", …) point there. Requirements, constants, and
algorithms are specified in the design and are not restated here. The
throwaway prototype under `prototypes/` informed the design but is not
carried into `src/`: implement from the design.

Every step ends with `cargo fmt` (no diff), `cargo clippy --all-targets --
-D warnings`, and `cargo test` passing, and is one reviewable commit.

## Checklist

- [x] Step 1: Uniform timeline shift; `--sync-offset-ms` on every conversion
- [x] Step 2: `src/sync/` estimator on synthetic audio
- [x] Step 3: Calibration against stock DDR World
- [x] Step 4: `--auto-sync` end to end for `SM5 → DDR` and `DDR → SM5`
- [x] Step 5: `--auto-sync` for `DDR_LEGACY` inputs
- [x] Step 6: Cross-build check, manual validation, README guidance

---

## Step 1: Uniform timeline shift; `--sync-offset-ms` on every conversion

**Objective.** Fix the pre-existing bias bug (design R11). The same
primitive later serves auto-sync.

**Guidance.**

- Add `src/job/sync_offset.rs` with `shift_timeline` as specified in
  *Components and Interfaces → Job orchestration*. Add
  `JobError::SyncShiftOverflow` to `JobError`.
- Delete `apply_sync_offset`. Call `shift_timeline(job.sync_offset_ms)` at
  the bias position of R10 on all four paths:
  - `sm5_to_ddr`: on the final `tempo_pairs` after `synthesize_events`;
  - `ddr_to_sm5` and `legacy_to_sm5`: on the model offset, with an empty
    pair slice;
  - `legacy_to_ddr`: on the final pairs.
- Update `Job::sync_offset_ms`'s doc comment and the `--sync-offset-ms` help
  text (design, CLI section).
- Docs:
  - `.spec/steering/product.md` rule 10: the bias applies on every
    conversion and moves every anchor.
  - README *Sync Offset* section: the bias now moves the whole chart on
    every conversion; the +53 ms Ultramix figure was tuned against the old
    first-segment-only behavior and is under re-validation.

**Tests.**

- `shift_timeline` unit tests from *Testing Strategy*:
  - every pair moves, and slopes and stop lengths are unchanged;
  - single-BPM regression: the last pair moves;
  - the model offset moves by δ/1000;
  - overflow returns `SyncShiftOverflow`.
- A `cli` test that `--sync-offset-ms` reaches `SM5 → DDR` and
  `DDR → SM5` jobs.
- `job` unit tests: for each path, the written chart's timing is shifted by
  exactly the bias. Use the existing in-module test helpers; build a `Song`
  directly rather than going through files.

**Integration.** Standalone; touches only `job/`, `cli/`, and docs.

**Demo.** `SM5 → DDR` with `--sync-offset-ms 10`: every `tempo_data` entry
in the output SSQ is 10 greater than without the flag (the tool's own SSQ
dump or parser shows this), where previously nothing changed. On a
single-BPM legacy song the final anchor now moves too.

---

## Step 2: `src/sync/` estimator on synthetic audio

**Objective.** The complete, format-independent estimator: `TimeMap`,
`chart_events`, onset envelope, score and verdict. Verified on synthetic
signals. This is the step most likely to reveal a design problem, so it
comes before any wiring.

**Guidance.**

- Add `rustfft = "6"` to `Cargo.toml`. Add its row, with rationale, to the
  `.spec/steering/tech.md` dependency table.
- Create the module layout in *Components and Interfaces → Sync module*.
  - Give `mod.rs` a `//!` header stating what the module owns and does not
    own.
  - Put the constants table from *Data Models*, each with a doc comment
    stating its origin, in `mod.rs`.
  - Seed `TARGET_OFFSET_MS` with −2.10 and mark it provisional until
    Step 3.
- Implement the *Estimator algorithm* exactly as specified: window, hop,
  band, emphasis, lag, attribution, event margin, search range, parabolic
  refinement, rival, split halves, verdict ordering, and rounding.
- Implement `TimeMap` semantics as specified, including stop-start
  placement and extrapolation. `from_song` mirrors the walk in
  `ssq::writer::synthesize_tempo_entries_until`, in `f64`.
- No `unwrap`/`expect`. The only fallible input, an empty or zero-rate
  buffer, yields `Refused(NoAudio)`.
- Register `pub mod sync;` in `src/lib.rs`.
- Update `.spec/steering/structure.md`: add `sync/` to the layout and the
  responsibilities table, as described in *Documentation Changes*.

**Tests.** All unit tests listed under *Testing Strategy* for
`sync::onset`, `sync::timemap`, `sync::events`, and `sync::estimate`.

- They include relative-recovery sweeps, one constructed case per refusal
  reason, and rounding edges.
- Synthetic helpers live in `#[cfg(test)]` code inside `src/sync/`.
- Keep signals to 10–15 s so debug-build tests stay fast; measure and note
  the runtime.

**Integration.** Library-only. Nothing in `job/` calls it yet; Steps 3 and
4 do.

**Demo.** `cargo test sync::` passes. On a synthetic 44.1 kHz click track
shifted by −55 … +55 ms, the recovered shifts match within 0.5 ms. A
drift-constructed case, a two-alignment case, and an over-cap case are each
refused with the right reason.

---

## Step 3: Calibration against stock DDR World

**Objective.** Confirm that the production estimator reproduces the
prototype's accuracy on the real catalogue, and fix `TARGET_OFFSET_MS` from
production measurements (design R13).

**Guidance.**

- Add `tests/auto_sync_calibration.rs`, `#[ignore]`d. Its behavior is
  specified in *Testing Strategy → Calibration test*:
  - env vars `DDR_WORLD_INSTALL` and `DDR_SYNC_OFFSETS_CSV`, skipping with
    a message if either is unset;
  - TPS 1000 songs only;
  - `T = median(m + c)`;
  - the thresholds;
  - a printed summary table.
- Use only the public library API: `ssq::parse`, `xwb::parse_audio`,
  `TimeMap::from_tempo_pairs`, `chart_events`, `sync::estimate`. Parse the
  CSV by hand; it is three columns. A song that fails to parse or decode is
  counted and skipped, not fatal.
- Run it in a release build. Set `TARGET_OFFSET_MS` to the measured `T`,
  rounded to 0.01 ms, and record the corpus size, `T`, and the agreement
  figures in the constant's doc comment.
- If agreement is materially below the prototype's (94.0% within 1 ms and
  97.6% within 2 ms on TPS 1000 songs), stop and investigate before
  Step 4. The first suspects are divergence from the specified parameters
  and attribution-time mistakes.
- Add the `.spec/steering/tech.md` gotchas listed in *Documentation
  Changes*:
  - DDR's integer-ms anchor normalization;
  - half-beat aliasing as the reason for the cap;
  - `TARGET_OFFSET_MS` is detector-specific;
  - the community CSV sign.
- Add the test's exception to the tests layout in `structure.md`.

**Tests.** The calibration test itself. It must also pass when re-run
against the updated constant.

**Integration.** Exercises Step 2's public API end to end against the
crate's own SSQ parser and XWB decoder.

**Demo.**
`DDR_WORLD_INSTALL=… DDR_SYNC_OFFSETS_CSV=… cargo test --release --test auto_sync_calibration -- --ignored --nocapture`
prints corpus size, `T`, agreement, and acceptance, and passes.

---

## Step 4: `--auto-sync` end to end for `SM5 → DDR` and `DDR → SM5`

**Objective.** The user-facing feature on the two most common conversions:
flags, orchestration, logging, and apply and report modes (R1–R3, R8–R10,
R12).

**Guidance.**

- `src/cli/job.rs`: add `AutoSyncMode` and `AutoSync`, and add
  `Job::auto_sync`. Update the existing test `Job` constructors, e.g.
  `job_for` in `src/job/mod.rs`.
- `src/cli/mod.rs`:
  - add the two arguments and the `validate()` rules from the design's CLI
    section;
  - thread `AutoSync` into every job in `into_plan`, with a default cap of
    `DEFAULT_MAX_CORRECTION_MS`;
  - until Step 5, `validate()` rejects `--auto-sync` with
    `--from-format DDR_LEGACY`, using a clear "not yet supported" error, so
    the flag is never silently ignored.
- `src/job/sync_offset.rs`: add `auto_sync`, with the log lines and refusal
  messages from *Logging*. The `key=value` tail must be stable.
- Wire both paths in the positions given in *Job orchestration*:
  - `sm5_to_ddr`: `TimeMap::from_tempo_pairs` on the final pairs, before the
    bias;
  - `ddr_to_sm5`: `TimeMap::from_song`, before the bias.
- Add `tests/common/mod.rs`: a synthetic-song builder that writes a small
  chart plus click-track audio, offset by a known X, using the crate's own
  writers and encoders.
- Docs:
  - `.spec/steering/product.md`: the *Auto-sync* and *Sync target* glossary
    entries, and new business rule 13 (see *Documentation Changes*).
  - README: both flags, and an *Auto-sync* section covering what it does,
    the cap, refusals, report mode, and the `DDR_LEGACY` limitation until
    Step 5.

**Tests.**

- CLI unit tests from *Testing Strategy*: bare flag → `Apply`; `report`;
  max-ms requires the flag; 0 and 201 rejected; temporary `DDR_LEGACY`
  rejection.
- `tests/sm5_to_ddr.rs` and `tests/ddr_to_sm5.rs`: the convergence,
  linearity, report-mode-inert, and bias-composes cases from *Integration
  tests*.

**Integration.** Consumes Step 1's `shift_timeline` and Steps 2–3's
calibrated estimator.

**Demo.**
- `ddr-chart-tools --from-format SM5 --to-format DDR --input-folder <pack> --auto-sync report`
  logs one line per song with the correction it would make.
- Re-running with `--auto-sync` applies it.
- Converting the output back with `--from-format DDR --to-format SM5 --auto-sync report`
  reports |δ| ≤ 1 ms.

---

## Step 5: `--auto-sync` for `DDR_LEGACY` inputs

**Objective.** Complete R1 by adding `DDR_LEGACY → DDR` and
`DDR_LEGACY → SM5`, including analysis of passthrough audio.

**Guidance.**

- `legacy_to_ddr`: after `synthesize_events`, and only when auto-sync is on,
  decode the audio with `decode_legacy_audio` for analysis even when
  `try_audio_passthrough` will byte-copy it.
  - Build `TimeMap::from_tempo_pairs` on the final pairs, then call
    `auto_sync`.
  - A decode failure here is a `warn!` and a skipped auto-sync (*Error
    Handling*).
- `legacy_to_sm5`: after modernize and decode, build `TimeMap::from_song`
  and call `auto_sync`. Shift the raw pairs as well, for consistency.
- Remove Step 4's temporary `DDR_LEGACY` rejection and its test, and drop
  the limitation note from the README.

**Tests.** `tests/ddr_legacy_to_ddr.rs` and `tests/ddr_legacy_to_sm5.rs`:

- convergence, linearity, and report-mode-inert cases;
- legacy passthrough preserved: output XWB and XSB byte-identical, SSQ
  shifted;
- a WAVM-audio case;
- the analysis-decode-failure fallback: the conversion succeeds and the
  chart is unchanged.

**Integration.** Same orchestration as Step 4. All four conversions now
honor `--auto-sync`.

**Demo.** An Ultramix batch
(`--from-format DDR_LEGACY --to-format DDR --auto-sync report`) logs a
measured correction per song while copying compliant XWBs untouched.

---

## Step 6: Cross-build check, manual validation, README guidance

**Objective.** Close the validation items the design leaves to manual
checks, and replace the unvalidated +53 ms guidance with measured
guidance.

**Guidance.**

- Cross-build once for `x86_64-pc-windows-gnu` (per the README's
  instructions) to confirm `rustfft` builds there.
- Run `--auto-sync report` over the maintainer's legacy console material for
  both targets. The corpus is the maintainer's full Dancing Stage Unleashed
  rip (Xbox, Ultramix-family engine; not in the repository): a flat folder
  of `<id>_all.ssq` + `<id>.wavm` pairs with `.sif` metadata, about 44
  songs.
  - Variant charts (`<id>_org_all.ssq`, `<id>_all_<name>.ssq`) and preview
    loops (`<id>_loop.wavm`) are expected to be reported unpaired in batch
    mode.
  - If corrections cluster (e.g. around +53 ms), rewrite the README's
    *Sync Offset* guidance: with `--auto-sync`, no Ultramix bias is needed,
    or a residual engine bias remains.
  - If the spread is wide or mostly refused, document that instead.
- In-game checks on DDR World:
  - a song whose correction makes `tempo_data[0]` more negative than about
    −22 ms (design assumption);
  - two or three auto-synced conversions for feel.
- ArrowVortex check of two or three `DDR_LEGACY → SM5 --auto-sync` outputs,
  confirming the SM5 target assumption.
- Record findings in `.spec/steering/tech.md` if they change a gotcha, and
  in the README.

**Tests.** None new. This step changes documentation only, unless a
finding exposes a defect, which gets a failing test first.

**Integration.** Validates Steps 1–5 on real content.

**Demo.** README guidance backed by a report-mode run. The in-game negative
`tempo_data[0]` check is recorded as passed, or the defect is filed.
