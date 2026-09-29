# Task: `--auto-sync` end to end for `SM5 → DDR` and `DDR → SM5`

## Description

Ship the user-facing feature on the two most common conversions:

- the `--auto-sync [apply|report]` and `--auto-sync-max-ms` flags;
- job orchestration that measures with `sync::estimate` and applies the
  correction through `shift_timeline`;
- per-song logging with a stable `key=value` tail;
- integration tests;
- docs.

`DDR_LEGACY` inputs are rejected with a clear "not yet supported" error
until Step 5, so the flag is never silently ignored.

This is one task rather than a CLI task plus an orchestration task.
Splitting would either expose a flag that does nothing for a commit, or
need the synthetic-song builder twice (unit and integration).

## Background

- **Available so far.** Steps 1–3 delivered:
  - `job::sync_offset::shift_timeline`, the uniform shift, already used by
    `--sync-offset-ms` on every path;
  - the `sync::` estimator: `TimeMap`, `chart_events`, `estimate`;
  - the calibrated `TARGET_OFFSET_MS` = −2.11 ms.
- **Ordering per path (R10).** Parse → build output timing → auto-sync
  (measure, then shift) → bias → write.
- **Timing used for events:**
  - `SM5 → DDR`: the final synthesized tempo pairs (TPS 1000).
  - `DDR → SM5`: the model's segments, stops and offset
    (`TimeMap::from_song`).

## Reference Documentation

**Required:**
- Design: `.agents/planning/2026-09-28-auto-sync/design/detailed-design.md`
  (R1–R3, R7–R10, R12; *Components and Interfaces → CLI*, *Job types*, *Job
  orchestration*, *Logging*; *Error Handling*; *Testing Strategy → `cli`*
  and *Integration tests*; *Documentation Changes*)

**Additional References (if relevant to this task):**
- `.spec/steering/product.md` business rules 5, 10 and 11 (audio pairing,
  bias, song code): must remain true.

**Note:** Read any document listed above before beginning implementation.

## Technical Requirements

1. **Job types (`src/cli/job.rs`).**
   - Add `AutoSyncMode { Apply, Report }` (`ValueEnum`, values `apply` and
     `report`) and `AutoSync { mode, max_correction_ms }`.
   - Add `Job::auto_sync: Option<AutoSync>`, with `///` docs.
2. **CLI (`src/cli/mod.rs`).**
   - `--auto-sync [apply|report]`: optional value, a bare flag means
     `apply`.
   - `--auto-sync-max-ms N`.
   - Help text on both.
3. **Validation (`Cli::validate()`).** New typed `CliError` variants:
   - `AutoSyncMaxRequiresAutoSync`;
   - `AutoSyncMaxOutOfRange { ms }`, outside
     1..=`sync::MAX_CORRECTION_LIMIT_MS`;
   - a temporary `AutoSyncLegacyNotYetSupported` for
     `--from-format DDR_LEGACY` with `--auto-sync`.
4. **Planning.** `into_plan` threads `AutoSync`, defaulting the cap to
   `sync::DEFAULT_MAX_CORRECTION_MS`, into every job.
5. **Orchestration (`src/job/sync_offset.rs`).** Add
   `auto_sync(job, cfg, audio, map: Option<&TimeMap>, song, tempo_pairs)
   -> Result<i32, JobError>`, returning the applied delta.
   - Builds events with `chart_events` (none when `map` is `None`).
   - Calls `estimate`.
   - Logs per design *Logging*: `info!` for apply, unchanged and report;
     `warn!` for refusals, with actionable messages; `debug!` detail comes
     from the estimator.
   - In `Apply` mode applies `Outcome::Apply` through `shift_timeline`.
     `Report` mode and refusals change nothing.
   - The tail keys `auto_sync=`, `delta_ms=`, `correction_ms=`,
     `measured_ms=`, `rival=`, `split_ms=`, `events=`, and `reason=` (on
     refusals) are stable. Measured keys are omitted when unmeasured.
   - The decision and message logic is a pure helper that can be
     unit-tested.
6. **Wiring (`src/job/mod.rs`).**
   - `sm5_to_ddr`: `TimeMap::from_tempo_pairs(&tempo_pairs, 1000)` on the
     final pairs, then `auto_sync`, then the bias shift.
   - `ddr_to_sm5`: `TimeMap::from_song(&song)`, then `auto_sync`, then the
     bias shift.
   - Paths without `--auto-sync` are unchanged.
7. **Integration tests.** `tests/common/mod.rs` provides a synthetic song
   builder:
   - irregular notes on two charts at 150 BPM;
   - an SSC written by `ssc::write`;
   - OGG audio of decaying noise bursts at the note times plus a chosen
     shift, placed with `sync::TimeMap::from_song`.

   Tests run the real binary (`env!("CARGO_BIN_EXE_ddr-chart-tools")`),
   parse the log tail from stderr, and compare re-parsed charts. Named
   `tests/sm5_to_ddr.rs` and `tests/ddr_to_sm5.rs`; the `DDR` input for the
   latter is produced by converting the synthetic SSC with the tool. The
   design's cases:
   - convergence (a second report run gives |δ| ≤ 1 ms);
   - linearity (a 25 ms audio shift changes δ by 25 ± 1);
   - report mode is inert (chart bytes identical to a run without the
     flag);
   - the bias composes with auto-sync (exactly +10 on every anchor or on
     `#OFFSET`).
8. **Docs.**
   - `.spec/steering/product.md`: glossary *Auto-sync* and *Sync target*;
     business rule 13 (opt-in, chart-side only, cap, ordering, refusal
     never fails a job).
   - `.spec/steering/structure.md`: `job/` row gains auto-sync
     orchestration.
   - README: flags table, and an *Auto-sync* section (what it does, the
     cap, refusals, report mode, the temporary `DDR_LEGACY` limitation).

## Dependencies

- Steps 1–3 (commits `65fbee4`, `64c520d`, `96209ae`, `7eff865`,
  `749f165`).

## Implementation Approach

1. Write the tests first:
   - CLI unit tests: bare flag → `Apply`; `report`; max-ms without the
     flag; 0 and 201; the legacy rejection; the default cap in jobs.
   - Unit tests of the pure decision/message helper: each outcome × mode,
     including tail keys.
   - Integration tests.
2. Implement the types, CLI, orchestration and wiring.
3. Docs.
4. Run `cargo fmt`, clippy and the full test suite.

## Acceptance Criteria

1. **Bare flag applies, `report` only reports**
   - Given `--auto-sync` or `--auto-sync report`
   - When parsed
   - Then jobs carry `Apply` or `Report` with the default 60 ms cap (or
     `--auto-sync-max-ms`)

2. **Invalid combinations are rejected**
   - Given `--auto-sync-max-ms` without `--auto-sync`, a value of 0 or 201,
     or `--auto-sync` with `--from-format DDR_LEGACY`
   - When validated
   - Then the matching `CliError` is returned (exit code 2)

3. **Convergence**
   - Given a synthetic song whose audio is offset from its chart
   - When converted `SM5 → DDR --auto-sync` and the output then converted
     `DDR → SM5 --auto-sync report`
   - Then the second run reports |δ| ≤ 1 ms
   - And likewise for `DDR → SM5 --auto-sync` followed by a report run on
     its output

4. **Linearity**
   - Given two sources differing only by a 25 ms audio shift
   - When each is converted with `--auto-sync report`
   - Then their reported corrections differ by 25 ± 1 ms

5. **Report mode is inert**
   - Given `--auto-sync report`
   - When converting
   - Then the written chart file is byte-identical to a run without
     `--auto-sync`

6. **Bias composes**
   - Given `--auto-sync --sync-offset-ms 10`
   - When converting
   - Then every anchor (SSQ) is exactly 10 ms later, or `#OFFSET` (SSC)
     exactly 0.010 s lower, than with `--auto-sync` alone

7. **Logs are actionable and greppable**
   - Given any auto-sync run
   - When it finishes
   - Then one `info!` or `warn!` line per song carries the stable
     `auto_sync=… delta_ms=…` tail, and refusals say what the user can do

8. **Docs updated**
   - Given README, `product.md` and `structure.md`
   - When read
   - Then they describe the flags, rule 13, and the temporary `DDR_LEGACY`
     limitation

9. **Quality gates**
   - Given the change
   - When `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and
     `cargo test` run
   - Then `fmt` produces no diff and both others pass

## Metadata
- **Complexity**: High
- **Labels**: sync, cli, job, integration-tests, docs
- **Required Skills**: Rust, clap, integration testing
- **Generated By**: code-task-generator 2026-09-28
- **Source Plan**: .agents/planning/2026-09-28-auto-sync/implementation/plan.md
- **Plan Step**: Step 4: `--auto-sync` end to end for `SM5 → DDR` and `DDR → SM5`
