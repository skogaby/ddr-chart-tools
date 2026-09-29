# Task: Uniform timeline shift; `--sync-offset-ms` on every conversion

## Description

Replace `apply_sync_offset` with a single `shift_timeline` primitive that
moves the whole chart relative to unchanged audio. Apply `--sync-offset-ms`
through it on all four conversions. This fixes two existing defects:

1. **Legacy → DDR bends the first tempo segment.** `apply_sync_offset` adds
   the bias to `raw_tempo_pairs[0].1` only. SSQ `tempo_data` values are
   absolute audio times and BPM is the slope between consecutive pairs, so
   on legacy → DDR the offset changes the first segment's tempo instead of
   moving the chart. On a single-BPM song the applied correction tapers
   from N ms at beat 0 to 0 ms at the end.
2. **Two paths ignore the flag.** `--sync-offset-ms` does nothing on
   `SM5 → DDR` and `DDR → SM5`.

The same primitive is what the upcoming auto-sync feature applies its
correction through, so it must be exact and uniform.

## Background

- **The model's sync scalar.** `Song::audio_sync_offset_seconds`
  (`src/model/mod.rs`) is the audio time at which beat 0 occurs (DDR sign
  convention). The SSC writer negates it into `#OFFSET`
  (`src/ssc/mod.rs`, `model_offset_to_sm`).
- **What SSQ output writes.** SSQ output emits `(measure_tick,
  tempo_data)` pairs. Every `tempo_data[i]` is an absolute audio time in
  seconds-ticks, and BPM is `Δticks / Δtempo_data` (`docs/ssq_format.md`
  §3, `src/ssq/tempo.rs`). Moving the chart N ms later therefore means
  adding N to *every* `tempo_data` entry. By the time any path writes, the
  tick rate is 1000, so seconds-ticks equal ms.
- **Current per-path behavior** (`src/job/mod.rs`):
  - **`sm5_to_ddr`** synthesizes pairs with
    `ssq::writer::synthesize_tempo_entries_until`, whose accumulator starts
    at `audio_sync_offset_seconds`. `synthesize_events` then returns the
    final `tempo_pairs`. Bias: not applied.
  - **`legacy_to_ddr`** runs `ssq_legacy::modernize::modernize`, then
    `apply_sync_offset` (first pair only). `synthesize_events` may then
    append a trailing pair extrapolated from the last segment's slope, and
    the resulting pairs are written verbatim.
  - **`legacy_to_sm5`** runs modernize, then `apply_sync_offset`. The SSC
    writer uses `audio_sync_offset_seconds` + `tempo_segments`, so the
    shift is uniform there today. Behavior must not change.
  - **`ddr_to_sm5`** passes the source offset through. Bias: not applied.
- **Ordering.** The design fixes the order for every path as: parse →
  modernize (legacy only) → build output timing → [auto-sync, a later
  step] → bias → write. For SSQ outputs the bias is applied to the *final*
  pairs, after `synthesize_events`. Because the shift is uniform, this is
  equivalent to shifting before extrapolation, and it is where auto-sync
  will slot in.
- **Business rule 10** (`.spec/steering/product.md`): the bias is
  additive, not replacing. It stays additive.

## Reference Documentation

**Required:**
- Design: `.agents/planning/2026-09-28-auto-sync/design/detailed-design.md`
  (R10, R11; *Components and Interfaces → Job orchestration*; *Error
  Handling*; *Testing Strategy → `job::sync_offset::shift_timeline`* and
  *`cli`*; *Documentation Changes*)

**Additional References (if relevant to this task):**
- `.agents/planning/2026-09-28-auto-sync/research/orientation.md`, section
  *Sync is one scalar in the model, but per-path handling is uneven*: the
  per-path analysis that found both defects.
- `docs/ssq_format.md` §3: tempo chunk semantics.
- `.spec/steering/rust-cli-standards.md`: error-handling and test
  conventions.

**Note:** Read any document listed above before beginning implementation.

## Technical Requirements

1. **New file.** Create `src/job/sync_offset.rs`, declared from
   `src/job/mod.rs`, with a `//!` header stating what it owns: the timeline
   shift. The next plan steps add auto-sync orchestration to it.
2. **The primitive.** Implement

   ```rust
   pub(super) fn shift_timeline(
       song: &mut Song,
       tempo_pairs: &mut [(i32, i32)],
       delta_ms: i32,
   ) -> Result<(), JobError>
   ```

   - Add `delta_ms` to every `tempo_pairs[i].1`, using checked arithmetic.
   - Add `delta_ms / 1000` s, as an exact `Rational`, to
     `song.audio_sync_offset_seconds`.
   - `delta_ms == 0` is a no-op that returns `Ok`.
   - Any overflow returns `JobError::SyncShiftOverflow { delta_ms }`. Never
     saturate, never silently skip. No partial mutation should be
     observable: check all entries before writing any.
3. **Error variant.** Add `JobError::SyncShiftOverflow { delta_ms: i32 }`
   with an `#[error(...)]` message naming the shift. It reaches the
   top-level `Error` through the existing `Job` variant.
4. **Remove the old function.** Delete `apply_sync_offset` and its call
   sites.
5. **Apply the bias on all four paths**, at the bias position:
   - `sm5_to_ddr`: on the final `tempo_pairs` returned by
     `synthesize_events`, and on `song`, before `ssq::writer::write`.
   - `legacy_to_ddr`: on the final `tempo_pairs` returned by
     `synthesize_events`, and on `result.song`, before `ssq::writer::write`.
   - `ddr_to_sm5`: on `result.song`, with an empty pair slice, before
     `ssc::write`.
   - `legacy_to_sm5`: on `result.song` and `result.raw_tempo_pairs`, the
     latter for consistency (it is not written), before `ssc::write`.
6. **Doc comments.** Update the comment on `Job::sync_offset_ms`
   (`src/cli/job.rs`) to say the bias applies on every conversion and moves
   every tempo anchor.
7. **Help text.** Update the `--sync-offset-ms` help text
   (`src/cli/mod.rs`): N ms moves every tempo anchor in SSQ output later
   (and `#OFFSET` by −N/1000 s in SSC output). Drop the
   "`tempo_data[0]`" wording.
8. **`.spec/steering/product.md`.** Business rule 10 and the *Sync offset*
   glossary entry now say the bias applies on every conversion and moves
   the whole chart (every anchor). Keep "additive, not replacing".
9. **README *Sync Offset* section.** State that the bias moves the whole
   chart on every conversion. State that the documented +53 ms Ultramix
   figure was tuned against the earlier first-segment-only behavior and is
   under re-validation. Keep the example commands.
10. **Standards.** Follow `CLAUDE.md` and
    `.spec/steering/rust-cli-standards.md`:
    - no `unwrap`/`expect` in non-test code;
    - `///` docs on new items;
    - `cargo fmt` gives no diff;
    - `cargo clippy --all-targets -- -D warnings` is clean.

## Dependencies

- None. This is the first step of the plan and is independent of the
  auto-sync estimator.
- Uses existing `crate::model::{Song, Rational}` arithmetic
  (`Rational::new`, `Rational::add`) and `crate::job::JobError`.

## Implementation Approach

1. **Write the unit tests first,** in `src/job/sync_offset.rs`:
   - uniform shift across a multi-segment pair list with a stop;
   - single-BPM regression, where the last pair moves;
   - model offset delta;
   - zero no-op;
   - overflow in the positive and negative directions leaves inputs
     untouched.
2. **Implement `shift_timeline`** and the error variant.
3. **Rewire the four paths** in `src/job/mod.rs` and delete
   `apply_sync_offset`. `legacy_to_ddr` already has its final pairs after
   `synthesize_events`. `sm5_to_ddr` has them too, so only an insertion is
   needed.
4. **Add path-level tests** in `src/job/mod.rs`'s test module. For each
   path, compare the timing the writer would receive with and without a
   bias; building `Song` / `SsqParseResult` values directly is fine.
   - SSQ paths: every pair differs by exactly N, and the pair ticks and
     consecutive differences are identical.
   - SSC paths: the model offset differs by exactly N/1000.
   - Add at least one test that writes SSQ bytes and re-parses them with
     `ssq::parse` to confirm the emitted `tempo_data` values.
5. **Add a `cli` test** that `--sync-offset-ms` is carried into jobs for
   `SM5 → DDR` and `DDR → SM5`. Note: `into_plan` already threads it; the
   test locks that down now that it is honored.
6. **Update docs:** help text, `Job` doc comment, `product.md`, README.
7. **Run the checks:** `cargo fmt`, `cargo clippy --all-targets -- -D
   warnings`, `cargo test`.

## Acceptance Criteria

1. **Legacy → DDR moves the whole chart**
   - Given a modernized legacy song with several tempo segments and a stop,
     and `--sync-offset-ms N`
   - When the SSQ is written
   - Then every `tempo_data` entry is exactly N greater than with no bias,
     every measure-tick is unchanged, and every consecutive difference
     (BPMs, stop durations) is unchanged

2. **Single-BPM regression**
   - Given a single-BPM legacy song whose pairs are `(0, a) … (END, b)`
   - When converted to DDR with `--sync-offset-ms 53`
   - Then the final pair's `tempo_data` is `b + 53`, not `b`

3. **SM5 → DDR honors the bias**
   - Given an SSC/SM song and `--sync-offset-ms N`
   - When converted to DDR
   - Then every emitted `tempo_data` entry is exactly N greater than without
     the flag (previously the flag had no effect)

4. **SSC outputs shift `#OFFSET`**
   - Given `--sync-offset-ms N` on `DDR → SM5` or `DDR_LEGACY → SM5`
   - When the SSC is written
   - Then `#OFFSET` is exactly N/1000 s lower than without the flag, and
     `#BPMS` / `#STOPS` are unchanged
   - This is new behavior for `DDR → SM5` and unchanged behavior for
     `DDR_LEGACY → SM5`

5. **No bias, no change**
   - Given no `--sync-offset-ms` (or 0)
   - When any of the four conversions runs
   - Then the written chart is identical to the output before this change

6. **Overflow is an error, not a wrong chart**
   - Given a tempo pair list where adding `delta_ms` would overflow `i32`
   - When `shift_timeline` is called
   - Then it returns `JobError::SyncShiftOverflow { delta_ms }`, and neither
     the pairs nor the model offset is modified

7. **Old helper removed and docs updated**
   - Given the finished change
   - When the codebase and docs are inspected
   - Then:
     - `apply_sync_offset` no longer exists;
     - the `--sync-offset-ms` help text, `Job::sync_offset_ms` doc,
       `product.md` rule 10 and glossary, and the README *Sync Offset*
       section all describe a whole-chart shift on every conversion;
     - the README flags the +53 ms figure as under re-validation

8. **Quality gates**
   - Given the finished change
   - When `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and
     `cargo test` are run
   - Then `fmt` produces no diff and both other commands pass

## Metadata
- **Complexity**: Medium
- **Labels**: bugfix, sync, job, cli, docs
- **Required Skills**: Rust, SSQ tempo-chunk semantics, exact rational arithmetic
- **Generated By**: code-task-generator 2026-09-28
- **Source Plan**: .agents/planning/2026-09-28-auto-sync/implementation/plan.md
- **Plan Step**: Step 1: Uniform timeline shift; `--sync-offset-ms` on every conversion
