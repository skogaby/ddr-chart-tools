# Requirements: 20261003-hudson-ssq-support

**Parent SIM**: none (hobby project)

## Problem

DDR Hottest Party 1 (Wii) songs are extracted as SSQ + WAV pairs. Their
charts live entirely in Hudson-format **type 9** step chunks
(`docs/hudson_ssq_format.md`) and carry gimmick "chara note" items. The SSQ
parser currently drops type 9 as an auxiliary chunk, so converted SSQs come
out with no steps. The audio is RIFF WAV (16-bit PCM, stereo, 32 kHz), which
`DDR_LEGACY` audio decoding doesn't accept (it tries WAVM and fails).

## User stories

- **US-1** As a modder, I can convert a Hottest Party SSQ + WAV pair with
  `--from-format DDR_LEGACY --to-format DDR` (or `SM5`) and get playable
  charts for every difficulty in the source.
- **US-2** The converted charts are the game's own "gimmicks off, hand
  markers off" charts: gimmick items become normal arrows, hazard items are
  removed, "Koopa" items gain their one-beat echo arrow, and freezes survive
  (`docs/hudson_ssq_format.md` §4).
- **US-3** WAV audio is accepted as `DDR_LEGACY` input in single-file and
  batch mode (`foo.ssq` ↔ `foo.wav`) and is encoded at its source rate. No
  resampling is done; 32 kHz is admitted to the set of DDR output rates.
- **US-4** The arcade type 9 "song metadata" chunk (`ssq_format.md` §8) is
  still dropped as before.

## Acceptance criteria

1. A type 9 chunk whose `param2` is a valid step difficulty code parses into a
   `Chart`. Any other type 9 chunk is recorded in `aux_chunks_dropped` as
   before.
2. Malformed Hudson chunks (truncated rows, extra block or sub-block out of
   bounds, item index past the `CHAR` table, a `FREZ` table too short for
   its freeze-end rows) fail with `SsqError::MalformedChunk` carrying the
   chunk's byte offset. Freeze-end rows with no `FREZ` table at all are
   ignored with a `warn!`, as the game ignores them.
3. Gimmick neutralization matches §4 of the format doc exactly, including
   echo placement, the 2-echo-per-row cap, and freeze resolution in game
   list order.
4. Removed hazard notes and dropped freezes are logged at `warn!`; the
   per-chart conversion summary is logged at `info!`.
5. Output SSQs contain only chunk types 1/2/3 at TPS=1000 (business rule 3).
6. RIFF WAV with 16-bit PCM (format tag 1, or `WAVE_FORMAT_EXTENSIBLE` with
   a PCM subformat) decodes into `AudioBuffer`. Other encodings are rejected
   with a typed error.
7. `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`
   are clean.

## Out of scope

- Hand-markers-on mode, gimmicks-on mode, and Mario Mix item semantics.
- Resampling audio.
- Banner/jacket conversion and song-code assignment for batch runs.
