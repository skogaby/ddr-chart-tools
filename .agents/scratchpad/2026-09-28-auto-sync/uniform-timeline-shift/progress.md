# Progress: uniform-timeline-shift

## Checklist

- [x] Baseline: `cargo test` before changes (386 passed)
- [x] Unit tests for `shift_timeline` (scenarios 1–6), failing
- [x] Implement `shift_timeline` and `JobError::SyncShiftOverflow`; unit tests pass
- [x] Path tests (scenarios 7–10); 7–9 fail, 10 passes
- [x] Wire four paths, delete `apply_sync_offset`; path tests pass
- [x] CLI test (scenario 11)
- [x] Docs: help text, `Job` doc, `product.md`, README
- [x] Manual old-vs-new binary comparison (A2)
- [x] Validate: fmt, clippy, full test suite; consistency review
- [x] Commit

## Log

1. **Baseline:** `cargo test` gave 386 passed, 0 failed.
2. **Red (unit).** The tests and the `shift_timeline` body were drafted
   together. To keep the red phase honest, the body was replaced with a
   no-op and `cargo test --lib job::sync_offset` run: 5 failed, and 1
   (`zero_shift_is_a_no_op`) passed, as expected against a no-op.
3. **Green (unit):** implementation restored; 6/6 pass.
4. **Red (paths).** 4 `run_one` tests using a synthetic two-segment song
   with a stop:
   - `sm5_to_ddr` failed: the first anchor was unchanged, so the bias was
     ignored.
   - `ddr_to_sm5` failed: the offset was unchanged.
   - `legacy_to_ddr` failed at the *second* anchor `(16384, 6000)`: only
     the first moved. This is the R11 bug, reproduced.
   - `legacy_to_sm5` passed; it is a regression guard.
5. **Green (paths).** Wired `shift_timeline` into all four paths at the
   bias position and deleted `apply_sync_offset`. 396 passed, no warnings.
6. **CLI test:** `sync_offset_reaches_sm5_and_ddr_jobs` passed immediately,
   as planned; the plumbing existed and the test locks it down.
7. **Docs.**
   - `--sync-offset-ms` help text and `Job::sync_offset_ms` doc updated.
   - `product.md`: rule 10, the *Sync offset* glossary entry, and a new
     *Common Mistakes* bullet.
   - README: flag table row and *Sync Offset* section, with the +53 ms
     figure flagged as under re-validation.
8. **Manual comparison (A2)** against a release build of `HEAD` (in a
   temporary git worktree, since removed).
   - **Inputs:** stock DDR World `puty` (TPS 150), an SSC/OGG converted
     from it, and Dancing Stage Unleashed `acow` (legacy, WAVM, non-zero
     origin).
   - **No bias, all four conversions:** SSQ, SSC, XWB and XSB are
     byte-identical.
   - **OGG outputs differ:** the old binary run twice also differs, because
     `vorbis_rs` randomizes the stream serial. With the per-page serial and
     CRC masked, the OGGs are identical.
   - **`DDR_LEGACY → SM5 --sync-offset-ms 53`:** SSC byte-identical (AC4
     "unchanged").
   - **Effect of the fix:**

     | Conversion | Old binary | New binary |
     |---|---|---|
     | `SM5 → DDR +10` | shift per anchor 0/0 | 10/10 |
     | `DDR_LEGACY → DDR +53` (20 anchors) | first/last 53/0 | 53/53, every anchor |
     | `DDR → SM5 +25` | `#OFFSET 0.000000` | `#OFFSET -0.025000` |

9. **Validate.**
   - `cargo fmt -- --check` clean.
   - `clippy -D warnings` flagged `type_complexity` on a test helper's
     return type. Fixed with test-module aliases `TestError`, `TestResult`
     and `TempoPairs` (no `allow`).
   - `cargo test`: 397 passed, 0 failed.
   - `cargo build --release` ok.
10. **Consistency review.**
    - The new module follows the `thiserror` `JobError` pattern with `//!`
      and `///` docs.
    - The old `unwrap_or(zero)`, `saturating_add` and silent-`if let Ok`
      handling are gone.
    - Tests use `?` with `Box<dyn Error>`.
    - `structure.md`'s `job/` row ("sync-offset bias") and `tech.md` remain
      accurate.
    - No issues left open.

## Deviations

- **Red phase for the unit tests.** Verified by temporarily neutering a
  drafted implementation rather than writing the implementation strictly
  after the tests. The failing run is in `logs/red-unit.log`.
- **Placement of the `legacy_to_sm5` bias.** The bias moved from right
  after `modernize` to right before writing: after `.sif` metadata and
  audio decode, which it does not interact with. This matches the R10
  ordering, and the output is byte-identical.

## Notes for later steps

- `vorbis_rs` output is not byte-deterministic (random stream serial).
  Step 4's "report mode is inert" integration test must compare *chart*
  timing, not OGG bytes.
- The path-test fixture helpers (`bias_fixture_song`, `write_bias_fixture`,
  `convert_fixture_with_bias`) live in `src/job/mod.rs`'s test module.
  Step 4's `tests/common/mod.rs` synthetic-song builder serves the
  integration tests and can follow the same shape.

Commit: 65fbee4

Status: Complete 65fbee4
