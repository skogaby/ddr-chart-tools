# Task: Onset-strength envelope (`sync::onset`)

## Description

Implement the audio side of auto-sync: an onset-strength envelope computed
from an `AudioBuffer` with a short-window "rising edge" feature. Add the
`rustfft` dependency it needs.

## Background

The approved design selected the envelope by scoring several candidate
features against a community-validated per-song offset database over the
stock DDR World catalogue. The chosen feature:

- 10 ms Hann window;
- hop of about 0.73 ms;
- log2 power, weighted by `f·e^(−f/3 kHz)` over 30 Hz–16 kHz;
- signed difference against the frame 2 ms earlier.

It agreed within 1 ms for 91% of songs. Its parameters are part of the
calibration: the target offset constant is valid only for exactly these
parameters. They are therefore specified precisely and must be implemented
exactly.

## Reference Documentation

**Required:**
- Design: `.agents/planning/2026-09-28-auto-sync/design/detailed-design.md`
  (*Estimator algorithm* items 1–2; *Data Models* `Envelope` and the
  constants table; *Testing Strategy → `sync::onset`*; *Appendix C* for
  `rustfft`)

**Additional References (if relevant to this task):**
- `.agents/planning/2026-09-28-auto-sync/research/onset-detection.md`: the
  rising-edge feature's origin (+9ms or Null?).
- `.agents/planning/2026-09-28-auto-sync/research/dependencies.md`:
  `rustfft`'s dependency footprint.

**Note:** Read any document listed above before beginning implementation.

## Technical Requirements

1. **Dependency.** Add `rustfft = "6"` to `Cargo.toml`. Add a row to the
   dependency table in `.spec/steering/tech.md`: onset-envelope FFT, pure
   Rust, no C code.
2. **`src/sync/onset.rs`.**
   - `Envelope { values: Vec<f32>, t0_s: f64, dt_s: f64 }`, with private
     fields and crate-visible accessors as needed.
   - `pub(crate) fn envelope(audio: &AudioBuffer) -> Option<Envelope>`.
3. **Algorithm** (design *Estimator algorithm* 1–2), exactly:
   - **Mono:** average the channels, scaled by 1/32768 to `f32`.
   - **Window:** `win = round(0.010·sr)`, periodic Hann `0.5 − 0.5·cos(2πn/win)`,
     zero-padded to `nfft = win.next_power_of_two()`.
   - **Hop:** `round(sr·32/44100)`. Frame count is `(len − win)/hop + 1` when
     `len > win`, else 0.
   - **Band:** bins `k` with `max(1, ceil(30/bin_hz)) ≤ k ≤ min(nfft/2,
     floor(16000/bin_hz))`, where `bin_hz = sr/nfft`.
   - **Frame energy:** `E = Σ w(f_k)·log2(|X_k|² + 1e-9)`, with
     `w(f) = f·e^(−f/3000)/1103.6`.
   - **Envelope value:** `L = max(1, round(0.002·sr/hop))`;
     `env[t] = E[t] − E[t−L]` for `t ≥ L`, and 0 before.
   - **Timing:** `t0_s = (win/2)/sr − L·hop/(2·sr)`, `dt_s = hop/sr`.
4. **`Envelope::sample(t_s)`.** Linear interpolation. Returns 0 before `t0`
   and at or past the last frame pair.
5. **`Envelope::start_s()` / `end_s()`** for the estimator's event-margin
   filter.
6. **Degenerate input.** Return `None` for empty audio, `sample_rate == 0`,
   `channels == 0`, or audio shorter than one window.
7. **Constants.** Parameters are named constants, with origin comments, in
   `src/sync/mod.rs` (design constants table).
8. **Implementation constraints.**
   - Plan the FFT once per call. Reuse the scratch and frame buffers across
     frames.
   - No `unwrap`/`expect` in non-test code.
   - `///` on items.

## Dependencies

- Task 01 of this step (`src/sync/` skeleton and constants table).
- `crate::model::AudioBuffer`.

## Implementation Approach

1. Write the tests first:
   - **Click train.** 20 one-millisecond clicks at irregular times over
     about 6 s at 44.1 kHz. For each click, the parabolically refined
     envelope peak lies within one hop of `click + c` for a single constant
     `c` (the median offset). `c` is small, under 10 ms.
   - **Silence.** The envelope is flat (every value exactly 0).
   - **Sample rate.** The same clicks at 48 kHz give a median offset within
     0.5 ms of the 44.1 kHz median.
   - **Degenerate input.** Empty, zero-rate, zero-channel and too-short
     buffers return `None`.
   - **Sampling.** `sample` interpolates and returns 0 outside the range.
2. Implement.
3. Measure the debug-build test runtime and record it.
4. Run `cargo fmt`, clippy, and the tests.

## Acceptance Criteria

1. **Onsets are localized consistently**
   - Given a click train at 44.1 kHz
   - When the envelope is computed
   - Then every click's envelope peak is within one hop of `click +
     median_offset`

2. **Rate-independent timing**
   - Given the same click train at 44.1 kHz and at 48 kHz
   - When both envelopes are computed
   - Then their median peak offsets differ by at most 0.5 ms

3. **Silence produces no onsets**
   - Given all-zero audio
   - When the envelope is computed
   - Then every value is 0

4. **Degenerate audio yields no envelope**
   - Given empty, zero-rate, zero-channel, or shorter-than-one-window audio
   - When `envelope` is called
   - Then it returns `None` without panicking

5. **Dependency justified**
   - Given the change
   - When `Cargo.toml` and `tech.md` are inspected
   - Then `rustfft = "6"` is present and documented

6. **Quality gates**
   - Given the change
   - When `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and
     `cargo test` run
   - Then `fmt` produces no diff and both others pass

## Metadata
- **Complexity**: Medium
- **Labels**: sync, dsp, audio, dependency
- **Required Skills**: Rust, FFT/STFT basics
- **Generated By**: code-task-generator 2026-09-28
- **Source Plan**: .agents/planning/2026-09-28-auto-sync/implementation/plan.md
- **Plan Step**: Step 2: `src/sync/` estimator on synthetic audio
