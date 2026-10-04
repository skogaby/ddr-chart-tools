# Design: 20261003-hudson-ssq-support

**Requirements**: [requirements.md](requirements.md)
**Format reference**: [`docs/hudson_ssq_format.md`](../../../docs/hudson_ssq_format.md)

## Overview

Two independent pieces:

1. `src/ssq/hudson.rs` decodes a type 9 chunk into a format-specific
   `HudsonChart` (rows, chara-note table, freeze table), then neutralizes it
   into a model `Chart` using the game's gimmicks-off rules. `ssq::parse`
   dispatches type 9 there when `param2` is a step difficulty code.
2. `src/wav/` decodes RIFF WAV PCM. The job layer's legacy audio sniffing
   recognizes `RIFF…WAVE`, and `DDR_LEGACY` accepts `.wav` files.

## Decisions

### D1: Neutralize gimmicks in the SSQ parser, not in `ssq_legacy`

The type 9 chunk *is* the chart; there's no type 3 alongside it to fall back
on. The model has no gimmick concept, and adding one would put Hudson-specific
data in `model/`, which the rules forbid. So the parser does what the game
does when gimmicks are off and emits plain `Tap`/`HoldHead` notes. The
intermediate `HudsonChart` keeps the decode testable on its own.

### D2: Distinguish Hudson charts from arcade metadata by `param2`

Arcade type 9 (`ssq_format.md` §8) is song metadata. Hudson type 9 always
carries a step difficulty code in `param2`. A valid code means a Hudson
chart, and decode errors propagate. Anything else is dropped as auxiliary,
exactly as before. No CLI-level switch is needed, which keeps the parser
format-variant agnostic (`ssq/mod.rs` doc).

### D3: Resolve freezes in game note-list order

The game appends echo notes straight after their row and closes freezes on
the most recent note in that list, removed hazards included. The
neutralizer builds the same list, resolves freezes against it, and only then
sorts and merges into model notes. Across all 211 HP1 charts this gives no
collisions, no notes inside freezes, and no freezes on removed notes.

### D4: Hand-rolled WAV reader, no new crate

A RIFF PCM reader is ~150 lines over `LeReader`. `hound` would add a
dependency for something this small. The reader takes 16-bit PCM (tag 1 or
extensible + PCM GUID) at any channel count and rate. Channel and rate
policy stay in the job layer.

### D5: Admit 32 kHz to `DDR_SAMPLE_RATES`

HP1 audio is 32 kHz and the user asked for no resampling. The XWB header
carries the rate and `xactengine2_10` rate-converts per wave
(`DDR_SAMPLE_RATES` doc), so 32 kHz should play. That's unverified on
hardware and gets revisited if the game refuses it.
