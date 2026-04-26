# SSQ File Format Reference

Reference specification for the SSQ step-file format used by DanceDanceRevolution.

---

## 1. Quick reference

- **Endianness**: Little-endian throughout.
- **Alignment**: All chunks are dword-aligned (chunk length always a multiple of 4). Within a chunk, step bytes are 1-byte packed and the freeze-info block is 2-byte aligned.
- **Magic**: None. The file is identified by extension (`.ssq`).
- **Terminator**: A final `00 00 00 00` sentinel (read as "chunk length = 0"). Some files have additional trailing zero-byte padding; the game ignores it.
- **Chunk types**: 1 (tempo), 2 (events), 3 (steps), 4 (effect data A), 5 (effect data B), 9 (metadata, rare), 17 (section markers).
- **Ticks per second (TPS)**: **Not fixed.** Stored per-file in the tempo chunk's `param2`. `1000` is the dominant value in newly-authored charts; older charts use lower rates. Values of `1000`, `150`, and `75` have all been observed in the wild.
- **Measure length**: 4096 ticks per measure (whole note), used by all tick-valued fields.

File layout at the top level:

```
+------------------+
| Chunk: TEMPO     |   type = 1                   (required, exactly one)
+------------------+
| Chunk: EVENTS    |   type = 2                   (required, exactly one)
+------------------+
| Chunk: STEPS #1  |   type = 3, one difficulty
+------------------+   ...
| Chunk: STEPS #N  |
+------------------+
| Chunk: SECTIONS  |   type = 17                  (optional, rare)
+------------------+
| Chunk: EFFECTS-A |   type = 4                   (optional; paired with type 5)
+------------------+
| Chunk: EFFECTS-B |   type = 5                   (optional; paired with type 4)
+------------------+
| Chunk: METADATA  |   type = 9                   (optional, rare)
+------------------+
| 00 00 00 00      |   terminator
+------------------+
```

Ordering rules:
- Tempo chunk (type 1) is always first.
- Events chunk (type 2) always immediately follows tempo.
- Step chunks (type 3) follow the events chunk.
- Auxiliary chunks (types 4, 5, 9, 17) come after step chunks.

The only **required** chunks for the step engine are tempo (1), events (2), and at least one step chunk (3). Everything else is optional.

### 1.1 Format variants

Individual SSQ files vary along two axes: the tempo chunk's TPS value, and
whether auxiliary chunks are present. These axes are independent — a file's
TPS does not determine which chunks it contains.

| Axis | Values seen | Notes |
|------|-------------|-------|
| TPS  | `1000`, `150`, `75` | `1000` is dominant in newly-authored charts; `150` and `75` appear in older charts. Files with TPS ≠ 1000 are referred to as **legacy** in this document. |
| Chunks | Always types 1, 2, 3; optionally 4, 5, 9, 17 | Authoring tools targeting current DDR should emit only types 1, 2, 3. |

Authoring tools should use TPS=1000 and emit only the required chunk types.
Parsers are expected to accept any positive TPS and any subset of chunk types
without rejecting the file.

---

## 2. Chunk header

Every chunk begins with a 12-byte header:

| Offset | Type | Name     | Description                                                 |
|--------|------|----------|-------------------------------------------------------------|
| +0x00  | u32  | length   | Total chunk size in bytes, **including this header**. Always a multiple of 4. |
| +0x04  | u16  | type     | Chunk type (1, 2, 3, 4, 5, 9, or 17)                        |
| +0x06  | u16  | param2   | Type-specific metadata                                      |
| +0x08  | u16  | param3   | Type-specific metadata (usually an entry count)             |
| +0x0A  | u16  | param4   | Type-specific metadata. Always 0 in observed files.         |
| +0x0C  | ...  | body     | Chunk body, `length − 12` bytes                             |

### 2.1 Chunk lookup

The game locates chunks in two ways:

- **Linear walk for tempo/events** — walks from the start of the file, picking the first chunk whose `type` field matches.
- **Scan by (type, param2) for steps** — walks until it finds a chunk matching both `type` and `param2` (used to pick a specific difficulty).

Both loops terminate when `length == 0` (terminator reached) OR when `param2 == 0xFFFF` (see §2.2).

### 2.2 `param2 = 0xFFFF` sentinel

If any chunk has `param2 == 0xFFFF`, the chunk-lookup loops treat it as an "end-of-useful-data" marker and abort the search. This is a forward-compatibility mechanism. No known SSQ file uses this value. Authoring tools should avoid it.

### 2.3 File terminator

After the last real chunk, a single `00 00 00 00` dword ends the file. The parser reads this as "chunk length = 0" and stops. Trailing zero bytes beyond the terminator are silently ignored.

---

## 3. Chunk type 1 — tempo / BPM changes

Exactly one per file. This is the authoritative source of both tempo changes and the file's tick rate.

| Header field | Value / meaning                                        |
|--------------|--------------------------------------------------------|
| type         | `1`                                                    |
| param2       | **Ticks per second (TPS)** — any positive u16; `1000` is dominant, `150` and `75` also observed |
| param3       | Number of tempo entries (N)                            |
| param4       | `0`                                                    |

### 3.1 Body layout

```
+------------------------+
| i32 time_offset[0]     |   N × 4 bytes
| i32 time_offset[1]     |
|        ...             |
| i32 time_offset[N−1]   |
+------------------------+
| i32 tempo_data[0]      |   N × 4 bytes
| i32 tempo_data[1]      |
|        ...             |
| i32 tempo_data[N−1]    |
+------------------------+
```

- `time_offset[i]` — position on the song timeline, in **measure ticks** (4096 per whole note).
- `tempo_data[i]` — cumulative position in **seconds-ticks** (elapsed time × TPS) measured from the song's logical start.

**Invariants**:
- `time_offset[0]` is the origin-shift between the chart's measure timeline
  and the audio-sync timeline. Sign and magnitude vary:
  - In TPS=1000 files, it is always `0` — chart timeline and audio-sync
    timeline share the same origin.
  - In legacy files (TPS < 1000), it may be any i32. Negative values
    (commonly `-4096`, `-8192`, `-12288`) indicate the chart timeline's
    origin is shifted *later* than the audio-sync origin — i.e., the audio
    has been playing for several beats before the chart's tick 0. Positive
    values indicate the chart timeline starts *earlier*; the audio catches
    up a few beats in. Values are typically whole-beat multiples of 1024,
    but sub-beat values have been observed.
- Chart content (step chunk `time_offset[i]`) is always `≥ 0`, even in
  files whose tempo `time_offset[0]` is negative.
- Event-chunk `time_offset[i]` mirrors tempo in sign: if tempo starts
  negative, the event chunk's first timestamps may also be negative.
- `tempo_data[0]` is an audio-sync offset in seconds-ticks (same unit as
  other `tempo_data[i]` values: `seconds × TPS`). In TPS=1000 files it is
  tightly bounded to ±22 ms — a fine-tune audio-sync adjustment. In legacy
  files, values can reach several seconds, consistent with an audio pre-roll
  duration.

  A positive `tempo_data[0]` means the tempo-time axis starts partway through
  an audio pre-roll — by the time the chart reaches tick 0, `tempo_data[0] / TPS`
  seconds of audio time have already elapsed.

Total body size: `8N` bytes.
Total chunk size: `12 + 8N` bytes (always a multiple of 4).

### 3.2 Converting to BPM

For each pair of consecutive entries `i−1`, `i` (with `i ≥ 1`):

```
delta_measure = time_offset[i] − time_offset[i−1]     (measure ticks)
delta_seconds = tempo_data[i]  − tempo_data[i−1]      (seconds-ticks at TPS)

BPM = 240 × TPS × delta_measure / (4096 × delta_seconds)
```

`delta_measure == 0` signals a stop (see §3.3) — treat it as a special case.

### 3.3 Stops

A stop is encoded as two consecutive entries with the **same** `time_offset[i]` but different `tempo_data[i]`:

```
stop_seconds = (tempo_data[i] − tempo_data[i−1]) / TPS
```

The BPM formula in §3.2 would divide by zero (`delta_measure == 0`); parsers must special-case this.

### 3.4 Runtime pre-computation

At chunk-load time the game computes a normalized per-entry value:

```
normalized[i] = round(tempo_data[i] × 1000 / TPS + 0.5)
```

This converts to a TPS-invariant millisecond-scale representation. The game tolerates any TPS value because everything gets rescaled here.

### 3.5 Worked example — tempo chunk with stops (TPS=150)

```
offset   bytes                      meaning
------   -----                      -------
0x0000   54 00 00 00                chunk length = 84
0x0004   01 00                      type = 1
0x0006   96 00                      param2 = 150 (TPS)
0x0008   09 00                      param3 = 9 (entries)
0x000A   00 00                      param4 = 0
0x000C   00 00 00 00                time_offset[0] = 0
0x0010   00 10 00 00                time_offset[1] = 4096
0x0014   00 20 01 00                time_offset[2] = 73728
0x0018   00 20 01 00                time_offset[3] = 73728    ← stop: same as [2]
0x001C   00 28 04 00                time_offset[4] = 272384
0x0020   00 a8 05 00                time_offset[5] = 370688
0x0024   00 a8 05 00                time_offset[6] = 370688   ← stop: same as [5]
0x0028   00 b0 05 00                time_offset[7] = 372736
0x002C   00 70 0a 00                time_offset[8] = 684032
0x0030   01 00 00 00                tempo_data[0]  = 1
0x0034   5e 00 00 00                tempo_data[1]  = 94
0x0038   99 06 00 00                tempo_data[2]  = 1689
0x003C   25 07 00 00                tempo_data[3]  = 1829     ← stop duration = (1829-1689)/150 = 0.933s
0x0040   e8 18 00 00                tempo_data[4]  = 6376
0x0044   7c 2a 00 00                tempo_data[5]  = 10876
0x0048   da 2a 00 00                tempo_data[6]  = 10970    ← stop duration = (10970-10876)/150 = 0.627s
0x004C   38 2b 00 00                tempo_data[7]  = 11064
0x0050   0d 47 00 00                tempo_data[8]  = 18189
```

BPM between entries 1 and 2: `240 × 150 × (73728 − 4096) / (4096 × (1689 − 94)) ≈ 383.7 BPM`.

---

## 4. Chunk type 2 — event stream

Exactly one per file. Contains song-structural events consumed alongside steps (song start/stop markers, section-change flags, etc.).

| Header field | Value / meaning                                          |
|--------------|----------------------------------------------------------|
| type         | `2`                                                      |
| param2       | Always `1`                                               |
| param3       | Number of event entries (N)                              |
| param4       | `0`                                                      |

### 4.1 Body layout

```
+-----------------------+
| i32 time_offset[0]    |   N × 4 bytes
|        ...            |
| i32 time_offset[N−1]  |
+-----------------------+
| u8  event[0].code     |   N × 2 bytes
| u8  event[0].arg      |
|        ...            |
| u8  event[N−1].code   |
| u8  event[N−1].arg    |
+-----------------------+
```

Each event is 2 bytes: `(code, arg)` in file order.
Total body size: `6N` bytes.
Total chunk size: `12 + 6N` bytes.

### 4.2 Event dispatch

| code | Action                                                               |
|------|----------------------------------------------------------------------|
| 1    | Silently consumed. No gameplay effect.                               |
| 2    | **Flow-control marker**. Emits a step-stream marker note (see §4.3). |
| 4    | Emits `{time, 4, arg}` into a separate events vector.                |
| 5    | Emits `{time, (arg & 0x3F) + 1, random()}` into the events vector.   |
| other | Silently consumed.                                                  |

### 4.3 Code-2 sub-events

When `code == 2`, `arg` selects a sub-type:

| arg | Marker byte | Typical meaning                          |
|-----|-------------|------------------------------------------|
| 1   | 0xFB        | Song start / music-on                    |
| 2   | 0xFA        | Chart start / "ready go" off             |
| 3   | 0xF9        | Pre-end cue                              |
| 4   | 0xFE        | Song end / results trigger               |
| 5   | 0xF8        | Alternate start-region cue               |
| other | (skipped) |                                          |

### 4.4 Canonical event sequence

The standard 6-entry pattern:

```
event[0] = (0x01, 0x04)   at tick 0              -- code-1, no effect
event[1] = (0x02, 0x01)   at tick 0              -- song start (0xFB)
event[2] = (0x02, 0x02)   at tick 4096           -- chart start (0xFA)
event[3] = (0x02, 0x05)   at tick 4096           -- (0xF8)
event[4] = (0x02, 0x03)   at SONG_END−4096       -- pre-end cue (0xF9)
event[5] = (0x02, 0x04)   at SONG_END            -- song end (0xFE)
```

Some files have additional code-1 events at mid-song ticks. The step parser silently consumes them. Code-1 is likely a reserved opcode that authoring tools emit but the current step engine does not act on.

---

## 5. Chunk type 3 — step chart

One per difficulty/style combination. Contains the actual arrows for one chart.

| Header field | Value / meaning                                  |
|--------------|--------------------------------------------------|
| type         | `3`                                              |
| param2       | **Difficulty code** (see §5.1)                   |
| param3       | Number of step entries (N)                       |
| param4       | `0`                                              |

### 5.1 Difficulty codes (param2)

The difficulty code is a 16-bit value composed of two bytes:

```
  +----------+----------+
  |  slot    |  style   |
  +----------+----------+
   high byte   low byte
```

**Play style** (low byte):

| Byte | Style  | Active panels |
|------|--------|---------------|
| 0x14 | Single | 4 (bits 0–3)  |
| 0x18 | Double | 8 (bits 0–7)  |

**Slot** (high byte):

| Byte | Slot name  | Alt names                        |
|------|------------|----------------------------------|
| 0x01 | Basic      | Light                            |
| 0x02 | Difficult  | Standard, Another, Trick         |
| 0x03 | Expert     | Heavy, Maniac, SSR               |
| 0x04 | Beginner   |                                  |
| 0x06 | Challenge  | Oni, CHAOS                       |

**Valid difficulty codes**:

| Value   | Chart             |
|---------|-------------------|
| 0x0114  | Single Basic      |
| 0x0214  | Single Difficult  |
| 0x0314  | Single Expert     |
| 0x0414  | Single Beginner   |
| 0x0614  | Single Challenge  |
| 0x0118  | Double Basic      |
| 0x0218  | Double Difficult  |
| 0x0318  | Double Expert     |
| 0x0418  | Double Beginner   |
| 0x0618  | Double Challenge  |

The game maps difficulty indices `{0, 1, 2, 3, 4}` to slots `{4, 1, 2, 3, 6}`. Slot value `0x05` is not accepted; charts using it won't be found.

### 5.2 Body layout

```
+--------------------------+
| i32 time_offset[0]       |   N × 4 bytes
|        ...               |
| i32 time_offset[N−1]     |
+--------------------------+
| u8  step[0]              |   N × 1 byte
|        ...               |
| u8  step[N−1]            |
+--------------------------+
| (up to 1 byte of 2-byte  |   pad so freeze block starts at a 2-byte-aligned offset
|  alignment padding)      |
+--------------------------+
| u8 freeze[0].panels      |   F × 2 bytes  (F = count of zero-valued step bytes)
| u8 freeze[0].kind        |
|        ...               |
| u8 freeze[F−1].panels    |
| u8 freeze[F−1].kind      |
+--------------------------+
| (up to 2 bytes of dword  |   pad so chunk total length is dword-aligned
|  alignment padding)      |
+--------------------------+
```

The freeze block starts at `step_block_start + round_up_to_even(N)`. If `N` is odd there is one padding byte between the last step byte and the freeze block. If `N` is even there is no padding.

**Size**: `chunk_length = 12 + 4N + round_up_to_even(N) + 2F + trailing_pad` where `trailing_pad ∈ {0, 2}` to make `chunk_length` a multiple of 4.

### 5.3 Step byte encoding

Each step byte represents one "row" — a set of panels struck simultaneously at `time_offset[i]`.

| Value      | Meaning                                                     |
|------------|-------------------------------------------------------------|
| `0x00`     | **Freeze-end marker** — consume one `freeze` entry (§5.4)   |
| `0xFF`     | **Both-side shock arrow** (all 8 panels)                    |
| `0x0F`     | **P1-side shock arrow** (in Double mode)                    |
| `0xF0`     | **P2-side shock arrow** (in Double mode only)               |
| any other  | **Normal step** — bitmask of panels pressed                 |

Bit layout for normal steps:

| Bit | Mask | Single mode       | Double mode       |
|-----|------|-------------------|-------------------|
| 0   | 0x01 | P1 Left           | P1 Left           |
| 1   | 0x02 | P1 Down           | P1 Down           |
| 2   | 0x04 | P1 Up             | P1 Up             |
| 3   | 0x08 | P1 Right          | P1 Right          |
| 4   | 0x10 | (unused — 0)      | P2 Left           |
| 5   | 0x20 | (unused — 0)      | P2 Down           |
| 6   | 0x40 | (unused — 0)      | P2 Up             |
| 7   | 0x80 | (unused — 0)      | P2 Right          |

In Single mode the high nibble of a normal-step byte is always zero.

### Shock arrows

A note is classified as a shock arrow when **all 4 panels of a player's side** are hit simultaneously:

| Byte | Single-mode chart | Double-mode chart         |
|------|-------------------|---------------------------|
| 0x0F | Shock (P1 side)   | Shock (P1 side only)      |
| 0xF0 | (never occurs)    | Shock (P2 side only)      |
| 0xFF | Shock (P1 side)   | Shock (both sides)        |

All three encodings are legitimate and used by the game.

### 5.4 Freeze block

A **freeze arrow** is encoded in two parts:

1. An earlier normal-step byte that hits the panels which will be held (the freeze HEAD).
2. A later `0x00` step byte whose time offset = the freeze's end time (the freeze TAIL marker).
3. One `(panels, kind)` pair in the freeze block that identifies which panels the freeze-end applies to.

The freeze block contains **one entry per `0x00` step byte**, in file order.

| Offset | Type | Name   | Description                                                    |
|--------|------|--------|----------------------------------------------------------------|
| +0x00  | u8   | panels | Bitmask of panels whose freeze ends here (same layout as §5.3) |
| +0x01  | u8   | kind   | `0x01` = normal freeze. Other values are silently ignored.     |

### How the parser resolves a freeze

When the parser encounters `step[i] == 0`:

1. Reads the next `(panels, kind)` pair from the freeze block.
2. If `kind != 0x01`, skips it (no freeze emitted).
3. Otherwise, walks the already-built note vector **backward** from the most recent note.
4. For each panel bit in `panels`, finds the most recent earlier note where that panel was hit.
5. Stores the freeze duration (`freeze_end_time − head_time`) on that earlier note.
6. Continues walking backward until all panel bits are matched.

### 5.5 Authoring freezes

To write a freeze note programmatically:

1. At the freeze **start time**, emit a normal step byte whose bits include the freeze-head panels.
2. At the freeze **end time**, emit a `0x00` step byte.
3. Append a `(panels, 0x01)` entry to the freeze block.

**Multiple freeze heads can share one freeze-end** by listing multiple bits in the `panels` byte. The parser walks backward per-bit, so each bit can match a different earlier note.

**Trailing dword padding**: After emitting F freeze entries (2F bytes), if the total chunk length is not a multiple of 4, append `00 00` to pad. The parser treats it as a freeze entry with `kind = 0` which it ignores.

---

## 6. Chunk type 4 — effect data stream A

An auxiliary chunk that appears only in legacy (TPS=150) files. Always paired with a type 5 chunk (see §7); no file has type 4 without type 5 or vice versa. Not consumed by the step engine. Believed to encode a stage-lamp on/off script synchronized to the song.

| Header field | Value / meaning                  |
|--------------|----------------------------------|
| type         | `4`                              |
| param2       | Always `1`                       |
| param3       | Entry count (N)                  |
| param4       | `0`                              |

### 6.1 Body layout

```
+-----------------------------+
| i32 time_offset[0..N−1]     |   N × 4 bytes
|   time_offset[0] = -99999   |     sentinel value — always `61 79 FE FF` LE
|   time_offset[1..N] = ticks |     (monotonically non-decreasing, measure ticks)
+-----------------------------+
| u8  data[0..N−1]            |   N × 1 byte
+-----------------------------+
| 0..3 trailing pad bytes     |   to dword-align total chunk; always zero
+-----------------------------+
```

Body size: `5N + pad`.

**Invariants**:
- `time_offset[0] == -99999` (the sentinel `61 79 FE FF`). Authoring tools **must** emit this.
- Remaining time offsets are monotonically non-decreasing.
- `data[i] ∈ {0x80, 0xFF}` — a pure binary toggle.

### 6.2 Authoring

1. Emit `offset[0] = -99999` as the first i32.
2. Emit `offset[1..N]` as measure ticks in non-decreasing order.
3. Emit `N` u8 values, each either `0x80` or `0xFF`.
4. Pad with zero bytes to make the total chunk length a multiple of 4.

Modern (TPS=1000) authoring should omit this chunk entirely.

---

## 7. Chunk type 5 — effect data stream B

Paired with type 4 — always co-occurs. Only appears in legacy (TPS=150) files. Not consumed by the step engine.

| Header field | Value / meaning      |
|--------------|----------------------|
| type         | `5`                  |
| param2       | `0`                  |
| param3       | Time-offset count (N)|
| param4       | `0`                  |

### 7.1 Body layout

```
+-----------------------------+
| i32 time_offset[0..N−1]     |   N × 4 bytes (ticks; monotonically non-decreasing)
+-----------------------------+
| record  sectA[0..N−2]       |   (N − 1) × 4 bytes  — "section A" records
+-----------------------------+
| u8[4]   separator           |   4 bytes: `95 14 00 00` (always, exactly once)
+-----------------------------+
| i32     sectB_count (M)     |   4 bytes
+-----------------------------+
| record  sectB[0..M−1]       |   M × 4 bytes  — "section B" records
+-----------------------------+
```

**Size**: body = `8N + 4M + 4` bytes. Always a multiple of 4.

**Invariants**:
- Exactly one separator `95 14 00 00` at a dword-aligned offset.
- Section A has exactly `N − 1` records (one per segment between consecutive offsets).
- `sectB_count` matches the actual number of trailing records.
- Time offsets are monotonically non-decreasing (duplicate ticks permitted).

### 7.2 Section A/B record format

Each 4-byte record:

| Offset | Type | Name   | Description                               |
|--------|------|--------|-------------------------------------------|
| +0x00  | u8   | tag    | Event-type tag                            |
| +0x01  | u8   | arg    | Tag-dependent argument                    |
| +0x02  | u16  | param  | Tag-dependent parameter                   |

The precise per-tag semantics (camera cue, lamp pattern, particle effect, etc.) are not fully documented. For authoring purposes the records can be copied through byte-for-byte from a reference file.

### 7.3 Authoring

Modern (TPS=1000) authoring should omit this chunk. For legacy preservation, the layout above round-trips byte-for-byte.

---

## 8. Chunk type 9 — song metadata (rare)

Observed in only one known file (`thr8.ssq`). Contains an embedded artist name string and a small amount of unstructured data. Not consumed by the step engine. Authoring tools can omit it.

---

## 9. Chunk type 17 — section markers

A short chunk listing pairs of tick ranges. Appears rarely.

| Header field | Value / meaning            |
|--------------|----------------------------|
| type         | `17` (`0x11`)              |
| param2       | `0`                        |
| param3       | Number of section pairs (N)|
| param4       | `0`                        |

### 9.1 Body layout

```
+--------------------------+
| i32 section[0].start     |   2N × 4 bytes
| i32 section[0].end       |
|        ...               |
| i32 section[N−1].start   |
| i32 section[N−1].end     |
+--------------------------+
```

Body size: `8N` bytes (no padding).

Semantics of the sections are not fully documented. Not consumed by the step engine, so safe to omit.

---

## 10. Runtime note-stream (post-parse) — for reference only

This section describes what the game builds from the SSQ data in memory. It is **not** part of the file format.

After parsing, the game has a note vector where each note carries a **marker** byte:

| Marker | Meaning                                       | Source                            |
|--------|-----------------------------------------------|-----------------------------------|
| 0x00   | Normal step OR freeze head                    | non-zero step byte                |
| 0x02   | Freeze tail (synthesized post-parse)          | generated from freeze head + duration |
| 0x80   | Tempo-change event                            | tempo chunk                       |
| 0xF8   | Event-chunk code-2 arg=5                      | event chunk                       |
| 0xF9   | Event-chunk code-2 arg=3                      | event chunk                       |
| 0xFA   | Event-chunk code-2 arg=2 (chart start)        | event chunk                       |
| 0xFB   | Event-chunk code-2 arg=1 (song start)         | event chunk                       |
| 0xFD   | Placeholder/default (not emitted)             | (internal)                        |
| 0xFE   | Event-chunk code-2 arg=4 (song end)           | event chunk                       |

Each note carries per-panel hit flags and per-panel freeze durations. The freeze post-processing walks these to synthesize tail notes (marker `0x02`).

---

## 11. Open questions

1. **Type 4 per-byte semantics** — layout is fully decoded (§6) and round-trips byte-for-byte; the `data` byte is always `0x80` or `0xFF`. The precise meaning (stage lamp, tape-LED, dim-lamp) requires further investigation.
2. **Type 5 per-tag semantics** — layout is fully decoded (§7) and round-trips byte-for-byte, but the semantics of each section-A/B tag are not established.
3. **Type 9 metadata format** — only one known sample. A larger corpus might clarify whether this chunk has a stable schema.
4. **Type 17 section semantics** — layout is known but the gameplay effect is not.
5. **`tempo_data[0]` exact sign convention** — §3 describes the value as a seconds-ticks audio-sync offset. The exact direction convention (does a positive value delay the chart, or delay the audio?) is inferred but not confirmed by live testing.
6. **Code-1 events** — the step parser silently ignores them. Their purpose is unknown; they may be reserved for an external system or a future feature.
