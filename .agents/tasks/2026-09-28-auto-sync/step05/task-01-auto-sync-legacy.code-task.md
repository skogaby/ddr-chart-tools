# Task: `--auto-sync` for `DDR_LEGACY` inputs

## Description

Complete R1 by wiring `--auto-sync` into `DDR_LEGACY → DDR` and
`DDR_LEGACY → SM5`. For `DDR_LEGACY → DDR`, compliant XWB audio is
byte-copied without decoding, so decode it for analysis only. Remove Step
4's temporary rejection.

## Background

- **Timing measured against:**
  - `legacy_to_ddr` writes the final tempo pairs, modernized and extended
    by `synthesize_events` (TPS 1000), and is measured against them.
  - `legacy_to_sm5` writes the model's segments, stops and offset, and is
    measured with `TimeMap::from_song`.
- **Passthrough.** `try_audio_passthrough` byte-copies a DDR-profile XWB
  plus its sibling XSB. Auto-sync must not change that: the correction is
  chart-side.
- **Decode failures.** A decode failure *for analysis only* is a `warn!`
  and a skipped auto-sync, never a job failure (design *Error Handling*). If
  the audio also has to be re-encoded, the existing decode error still
  fails the job, as today.

## Reference Documentation

**Required:**
- Design: `.agents/planning/2026-09-28-auto-sync/design/detailed-design.md`
  (R1, R9, R10; *Job orchestration* `legacy_to_ddr` / `legacy_to_sm5`;
  *Error Handling*; *Integration tests* 1–3 and 5)

**Additional References (if relevant to this task):**
- `docs/ultramix_archive_formats.md`: the WAVM layout the test encoder
  targets.

**Note:** Read any document listed above before beginning implementation.

## Technical Requirements

1. **`legacy_to_ddr`.**
   - After `synthesize_events`, when `job.auto_sync` is set, decode the
     audio once with `decode_legacy_audio`:
     - on success, build `TimeMap::from_tempo_pairs(&tempo_pairs,
       song.tps)`, then `auto_sync_delta`, then `shift_timeline`;
     - on failure, `warn!` "auto-sync skipped: cannot decode audio for
       analysis: {error}" and continue.
   - Then apply the bias shift and write the SSQ.
   - Audio: passthrough as before. Otherwise reuse the analysis decode when
     present, or decode now.
2. **`legacy_to_sm5`.** After decode: `TimeMap::from_song(&result.song)`,
   then `auto_sync_delta`, then `shift_timeline(&mut result.song, &mut
   result.raw_tempo_pairs, delta)`, then the bias.
3. **CLI.** Remove `CliError::AutoSyncLegacyNotYetSupported`, its check and
   its test. Add a test that `--auto-sync` is accepted with
   `--from-format DDR_LEGACY`.
4. **Test helpers.**
   - `tests/common/mod.rs` gains a small XBOX-IMA (WAVM) encoder: the
     block layout of `src/wavm/xbox_ima.rs`, with the standard IMA
     quantizer tracking the decoder's state.
   - Plus helpers that build legacy inputs (SSQ from the tool, with WAVM or
     XWB + XSB audio).
5. **`tests/ddr_legacy_to_ddr.rs`.**
   - Convergence (WAVM).
   - Linearity (WAVM).
   - Report mode is inert.
   - Passthrough preserved: output XWB and XSB byte-identical to the input,
     SSQ moved.
   - An undecodable passthrough bank: the conversion succeeds, the warning
     is logged, the chart is unchanged, and the XWB is byte-identical.
6. **`tests/ddr_legacy_to_sm5.rs`.** Convergence (WAVM), report mode is
   inert, and the bias composes.
7. **README.** Drop the *Not yet for `DDR_LEGACY` inputs* note.

## Dependencies

- Step 4 (commit `e0c126b`): `auto_sync_delta`, the CLI flags, and
  `tests/common`.

## Implementation Approach

1. Write the tests first:
   - the CLI acceptance test;
   - the legacy integration tests, which fail today because of the CLI
     rejection.
2. Implement the wiring and remove the rejection.
3. Update the README.
4. Run `cargo fmt`, clippy and the full test suite.

## Acceptance Criteria

1. **Legacy inputs are accepted**
   - Given `--from-format DDR_LEGACY --auto-sync`
   - When validated
   - Then no error is returned

2. **Convergence on WAVM audio**
   - Given a legacy SSQ whose WAVM audio is offset
   - When converted with `--auto-sync` to DDR (or SM5), and the output then
     re-measured with `--auto-sync report`
   - Then |δ| ≤ 1 ms

3. **Linearity**
   - Given two legacy sources differing only by a 25 ms audio shift
   - When measured with `--auto-sync report`
   - Then their corrections differ by 25 ± 1 ms

4. **Report mode is inert**
   - Given `--auto-sync report`
   - When converting a legacy source
   - Then the chart is byte-identical to a run without `--auto-sync`

5. **Passthrough preserved**
   - Given a compliant XWB + XSB legacy source
   - When converted to DDR with `--auto-sync`
   - Then the output XWB and XSB are byte-identical to the input while the
     SSQ anchors move by the applied delta

6. **Undecodable analysis audio is not fatal**
   - Given a passthrough-eligible XWB whose audio cannot be decoded
   - When converted with `--auto-sync`
   - Then the job succeeds with a warning, the SSQ equals a run without
     auto-sync, and the XWB is copied unchanged

7. **Bias composes on legacy → SM5**
   - Given `--auto-sync --sync-offset-ms 10`
   - When converting to SM5
   - Then `#OFFSET` is exactly 0.010 s lower than with `--auto-sync` alone

8. **Quality gates**
   - Given the change
   - When `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and
     `cargo test` run
   - Then `fmt` produces no diff and both others pass

## Metadata
- **Complexity**: Medium
- **Labels**: sync, job, legacy, integration-tests
- **Required Skills**: Rust, ADPCM basics
- **Generated By**: code-task-generator 2026-09-28
- **Source Plan**: .agents/planning/2026-09-28-auto-sync/implementation/plan.md
- **Plan Step**: Step 5: `--auto-sync` for `DDR_LEGACY` inputs
