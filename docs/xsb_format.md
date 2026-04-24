# XSB Format (XACT2 Sound Bank, DDR World profile)

This document specifies the subset of the Microsoft XACT2 Sound Bank (XSB) binary
format that DDR World uses for its song sound banks, at the level of detail needed
to write correct files from scratch.

The full XSB format is much larger than what's described here. This document
covers **only what DDR's XSBs actually use** — one wave bank, two simple cues
(main + preview), one simple sound, one complex sound with a loop event. Fields
the format defines but DDR doesn't populate are noted but not fully specified.

DDR's XACT2 runtime is `xactengine2_10.dll` (v2.10). The format matches what
that specific DLL accepts; newer XACT versions add fields not covered here.

## Conventions

- **Endianness**: little-endian throughout.
- **Alignment**: none required inside the file (unlike XWB). Offsets are raw
  byte offsets from the start of the XSB.
- **Strings**: 8-bit ASCII. Fixed-size name fields (64 bytes) are null-padded.
  Cue-name-table strings are null-terminated and packed back-to-back.
- **Sentinels**: `0xFFFFFFFF` (as `i32`) and `0xFFFF` (as `u16`) mark "not
  present" / "end of chain" throughout the file.

## Top-Level Structure

An XSB is a fixed header followed by variable-length sections. The header lists
byte offsets to each section. The sections can appear in any order; DDR emits
them in the order below.

```
offset  size   section
------  -----  ---------------------------------------------------------
0x00    0x4a   Header
0x4a    0x40   Soundbank name (64 bytes, null-padded ASCII)
0x8a    ----   Wave bank names        (wavebank_count × 64 bytes)
0xca    ----   Sound entries          (sound_count sounds, variable size)
----    ----   Simple cue entries     (simple_cue_count × 5 bytes)
----    ----   Cue name hash table    (total_cues × 2 bytes)
----    ----   Cue name index         (cue_count × 6 bytes)
----    ----   Cue name strings       (cue_name_table_length bytes)
```

For a standard DDR song (4-char code, 1 wave bank, 2 cues, 2 sounds, 16 hash
buckets), the total size is **326 bytes**. A 5-char code adds 2 bytes to the
cue name string table.

## Header (offset 0x00, length 0x4a)

```
offset  size   field                 value (DDR profile)
------  -----  --------------------- -----------------------------------
0x00    u32    magic                 0x4B424453 ("SDBK")
0x04    u16    content_version       0x002B (43)
0x06    u16    tool_version          0x002B (43)
0x08    u16    crc                   (computed, see CRC-16 below)
0x0a    u64    timestamp             arbitrary (DDR uses real FILETIME,
                                     but zero is accepted by the engine)
0x12    u8    platform              0x01 (Windows)
0x13    u16    simple_cue_count      2
0x15    u16    complex_cue_count     0
0x17    u16    (unknown, always 0)   0x0000
0x19    u16    total_cues            16 (the hash-table bucket count —
                                     always max(16, simple+complex))
0x1b    u8    wavebank_count        1
0x1c    u16    sound_count           2
0x1e    u16    cue_name_table_length (12 for 4-char, 14 for 5-char codes)
0x20    u16    (unknown, always 0)   0x0000
0x22    i32    simple_cue_offset     byte offset of the simple-cue array
0x26    i32    complex_cue_offset    -1 (no complex cues)
0x2a    i32    cue_name_offset       byte offset of the cue name strings
0x2e    i32    (unknown, always -1)  0xFFFFFFFF
0x32    i32    variation_offset      -1 (no variation tables)
0x36    i32    transition_offset     -1 (no transition tables)
0x3a    i32    wavebank_name_offset  byte offset of wave bank names
0x3e    i32    cue_hash_offset       byte offset of the hash table
0x42    i32    cue_name_index_offset byte offset of the name index array
0x46    i32    sound_offset          byte offset of the sound entries
```

The **CRC-16** at `0x08` covers bytes `[0x12 .. end_of_file]` (see below). The
engine validates this on load and silently rejects the file — muting all audio
for that song — if it doesn't match.

The three "unknown" fields at `0x17`, `0x20`, and `0x2e` are validated as
exact values (0, 0, −1) by the engine's XSB structure validator in
`xactengine2_10.dll` (function `FUN_0040e970`). We must write them as shown.

## Soundbank Name (offset 0x4a, length 0x40)

A 64-byte fixed-size field holding an ASCII name padded with null bytes. DDR
uses the 4-char song code (e.g. `"acef"`). The engine doesn't appear to use
this string for lookup — it's descriptive.

## Wave Bank Names

`wavebank_count` × 64 bytes, each a null-padded ASCII name. DDR has exactly
one wave bank per song and names it with the 4-char song code.

The wave bank *content* (the XWB file) is loaded by filename; this field is
metadata only. It must match the XWB that pairs with this XSB or the engine
will fail to resolve the wave reference.

## Sound Entries

`sound_count` sound entries packed back-to-back. Each entry begins with a
common 9-byte prefix, then variable-size body. The `entry_length` field gives
the total size of the entry (prefix + body).

### Common prefix (9 bytes)

```
offset  size  field          description
------  ----  -------------- -------------------------------------------
 +0     u8    flags          bit 0 = complex (else simple)
                             bit 2 = has RPC curve references
                             (DDR uses 0x04 for simple, 0x05 for complex)
 +1     u16   category       index into the global XGS category table
                             (DDR: 4 for main track, 3 for preview)
 +3     u8    volume         0..255, linear. DDR uses 180 (0xB4).
 +4     i16   pitch          cents, signed. DDR uses 0.
 +6     u8    priority       0..255. DDR uses 0.
 +7     u16   entry_length   total size of this sound entry in bytes
```

### Simple sound body (10 bytes, total entry_length = 19)

Used for the main track. Immediately after the common prefix:

```
offset  size  field           description
------  ----  --------------  ------------------------------------------
 +9     u16   wave_index      index into the wave bank (DDR: 1 = main)
 +11    u8    wavebank_index  which wave bank (DDR: 0, only one exists)
 +12    u16   rpc_length      7 (length of the RPC block that follows)
 +14    u8    rpc_count       1 (one RPC reference)
 +15    u32   rpc_code        0x000000F8 (DDR's main-track RPC code)
```

Total: 9 + 3 + 7 = 19 bytes.

The trailing 7-byte block (`rpc_length`, `rpc_count`, `rpc_code`) is only
present when `flags & 0x04` is set. It references a runtime-parameter-curve
entry in the XGS that applies per-sound DSP (envelopes, volume ramps, etc.).
DDR uses one specific RPC code for all main tracks.

### Complex sound body (30 bytes, total entry_length = 39)

Used for the preview track with a loop event. Immediately after the common
prefix:

```
offset  size  field           description
------  ----  --------------  ------------------------------------------
 +9     u8    track_count     1
 +10    ....  track[0]        29 bytes — see "Preview track template"
```

Total: 9 + 1 + 29 = 39 bytes.

There is no trailing per-sound RPC block on the complex sound when `flags` is
`0x05` — the RPC data is embedded *inside* the track, not after the sound.

#### Preview track template (29 bytes)

All 12 stock DDR XSBs use a byte-identical track template for the preview,
with the single exception of one byte whose purpose is not fully understood.

```
00 01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e 0f 10 11 12 13 14 15 16 17 18 19 1a 1b 1c
07 00 01 f8 00 00 00 b4 ?? 00 00 00 01 01 00 00 20 00 00 ff 0c 00 00 00 ff 00 00 00 00
```

Broken down (interpretations are best-effort based on the FACT format family):

```
0x00..0x07  RPC preamble     len=7, count=1, code=0x000000F8
                             (same RPC block shape as simple sound, but
                             placed at the start of the track body rather
                             than trailing the sound entry)
0x07        volume           0xB4 = 180
0x08        mystery byte     see below
0x09..0x1c  clip/event body  21 bytes, identical across all stock files
```

**The mystery byte at offset 8**: observed values

| Value  |
| ------ |
| `0xE0` |
| `0xF3` |
| `0xE5` | 

The remaining 28 bytes of the track are byte-identical across all files.
Purpose of this one byte is not conclusively determined — likely a loop/event
parameter (duration, fade, or loop count scaled to preview length). We use
`0xE0` (majority value) as a safe default.

## Simple Cue Entries

`simple_cue_count` × 5 bytes, at `simple_cue_offset`. Each entry:

```
offset  size  field     description
------  ----  --------  ----------------------------------------------
 +0     u8   flags     0x04 for a playable sound cue
 +1     u32   sb_code   byte offset *within this XSB file* to the
                       referenced sound entry (e.g. 0xCA for the
                       first sound)
```

DDR's two cues reference the two sound entries. Cue order in the file
matches hash-bucket iteration order — it's not semantically meaningful; the
game looks up by name, not index.

The `flags` byte has additional bits (0x01, 0x02) that would indicate
variation/transition tables — DDR never uses those.

## Cue Name Hash Table

`total_cues` × `u16`, at `cue_hash_offset`. `total_cues` is always 16 for
DDR (and is the `max(16, simple_cue_count + complex_cue_count)` clamp the
format enforces).

Each bucket holds either `0xFFFF` (empty) or an index into the cue name index
array (see below). To look up a cue named `n`, the engine hashes `n`
(algorithm below), reduces modulo `total_cues`, reads the bucket, and walks
the chain via the name index's `next` field.

## Cue Name Index

`simple_cue_count + complex_cue_count` × 6 bytes, at `cue_name_index_offset`.
Each entry:

```
offset  size  field          description
------  ----  -------------  ---------------------------------------------
 +0     u32   name_offset    byte offset *within the XSB file* to the
                             null-terminated name string
 +4     u16   next_in_chain  0xFFFF = end of chain, else index of the
                             next name-index entry in the same bucket
```

The `next_in_chain` field lets multiple cues that hash to the same bucket
form a linked list. In every stock DDR XSB the two cues hash to different
buckets, so both chains are length 1 and `next` is always `0xFFFF`.

## Cue Name Strings

`cue_name_table_length` bytes at `cue_name_offset`, holding all cue names
concatenated as null-terminated ASCII.

For DDR's two-cue layout: `"{code}\0{code}_s\0"` where `{code}` is the
song's 4-char (or 5-char) code. `_s` is the preview-clip suffix.

For a 4-char code: `4 + 1 + 6 + 1 = 12` bytes.
For a 5-char code: `5 + 1 + 7 + 1 = 14` bytes.

## CRC-16

The engine validates bytes `[0x12 .. end]` against a CRC-16 stored at
`0x08` (as the bitwise NOT of the computed CRC).

The algorithm is the reflected CRC-16/CCITT variant (polynomial 0x1021,
reflected to 0x8408), init 0xFFFF, no final XOR — standard "CRC-16/X-25"
minus the final XOR. Equivalently, as a table-driven implementation:

```rust
fn xact_crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc = CRC_TABLE[((b as u16) ^ crc) as usize & 0xFF] ^ (crc >> 8);
    }
    !crc
}
```

The 256-entry `CRC_TABLE` is extracted verbatim from `xactengine2_10.dll`
at `FUN_00424200` and is present in `src/xsb/mod.rs`.

## Cue Name Hash

**The hash function used to look up cues by name.** This was the final
blocker for from-scratch XSB generation, reverse-engineered from
`xactengine2_10.dll` function `FUN_0040fad0` (called from `GetCueIndex` at
vtable slot 0 of the SoundBank COM interface).

```rust
/// Hash a cue name (ASCII, null-terminator not included) into a hash-table
/// bucket. `bucket_count` is the XSB's `total_cues` (16 in all DDR XSBs).
fn cue_name_hash_bucket(name: &[u8], bucket_count: u16) -> u16 {
    let mut h: u16 = 0;
    for &c in name {
        // h = 3*h + (h >> 1) + c, all u16 wrapping
        h = h.wrapping_mul(3)
            .wrapping_add(h >> 1)
            .wrapping_add(c as u16);
    }
    // The DLL uses IDIV (signed), but for ASCII-derived u16 values and
    // bucket_count=16 the result is identical to unsigned modulo.
    h % bucket_count
}
```

The function is 15 instructions of tight x86-64 assembly. The underlying
per-character update in the DLL is:

```
r10d = h (at start of iteration)
AX = h; AX += AX                ; AX = h*2 (u16 wrap)
R9W = h; R9W >>= 1               ; R9W = h/2
R10D += EAX                      ; h += h*2        (= 3h)
R10D += R9D                      ; h += h/2        (= 3h + h/2)
R10W += AX_LOW                   ; h_low += char   (u16 wrap; MOVSX from
                                 ;                 byte, but for ASCII
                                 ;                 equivalent to u16 zext)
```

## Full Write Procedure (DDR Profile)

Given a 4-char ASCII song code `{code}`, emit:

1. **Precompute offsets** based on the known section sizes for the DDR profile:
   - Header ends at 0x4a
   - Soundbank name: +0x40 → ends at 0x8a
   - Wave bank names (1 × 64): +0x40 → ends at 0xca
   - Sound entries (COMPLEX 39 + SIMPLE 19): +58 → ends at 0x104
   - Simple cue entries (2 × 5): +10 → ends at 0x10e
   - Hash table (16 × 2): +32 → ends at 0x12e
   - Name index (2 × 6): +12 → ends at 0x13a
   - Cue name strings (12 for 4-char code): +12 → ends at 0x146
   - File size: **326 bytes** for a 4-char code

2. **Write header** with the fixed values above and the computed offsets.
   Leave the CRC field (`0x08..0x0a`) as zero; fill it in last.

3. **Write soundbank name**: 4 bytes of `{code}` null-padded to 64.

4. **Write wave bank name**: 4 bytes of `{code}` null-padded to 64.

5. **Write sound 0 (COMPLEX preview with loop)** at offset 0xCA:
   ```
   05 03 00 b4 00 00 00 27 00 01                 // flags=0x05, cat=3,
                                                 // vol=180, ..., entry_len=39,
                                                 // track_count=1
   07 00 01 f8 00 00 00                          // track RPC preamble
   b4 e0 00 00 00 01 01 00 00 20 00 00 ff 0c     // track body (22 bytes)
   00 00 00 ff 00 00 00 00
   ```

6. **Write sound 1 (SIMPLE main track)** at offset 0xCA + 39 = 0xF1:
   ```
   04 04 00 b4 00 00 00 13 00   // flags=0x04, cat=4, vol=180, pitch=0,
                                // prio=0, entry_len=19
   01 00 00                     // wave_index=1, wavebank_index=0
   07 00 01 f8 00 00 00         // RPC: len=7, count=1, code=0xF8
   ```

   > **Ordering matters.** The XACT2 engine in DDR World only plays audio when
   > the complex (preview) sound is first and cue index 0 points at it.
   > Emitting the sounds in the reverse order produces files that the engine
   > silently rejects during cue resolution, even though the structural
   > validator accepts them. Some stock files use the inverse ordering
   > and still play in game — the reason isn't fully understood; it may
   > depend on engine state we can't observe from the file alone. We
   > always emit the ordering that is empirically known to work.

7. **Write cue 0**: `04` + `u32(0xCA)` (points to preview sound).
8. **Write cue 1**: `04` + `u32(0xCA + 39) = u32(0xF1)` (points to main sound).

9. **Build hash table**: for each cue, hash its name and place the cue index
   in the corresponding bucket. If two cues hash to the same bucket, walk
   the chain to its tail and append via the tail's `next` field.

   - Cue 0 name = `{code}_s` (preview)
   - Cue 1 name = `{code}` (main)

   In practice, the two DDR cue names never collide in a 16-bucket table
   (verified across all 12 stock files), so both chains are length 1.

10. **Write name index**: for each cue, `u32(name_string_offset)` + `u16(next)`.
    The `name_string_offset` is the file-level offset into the cue name strings
    section where that cue's name begins.

11. **Write cue name strings**: `"{code}_s\0{code}\0"`.

12. **Compute and back-patch CRC** over bytes `[0x12 .. end]`; store at `0x08`
    (little-endian, as `!crc16`).

## Known Unknowns

These items are not fully understood but are not blockers:

1. **The one mystery byte** at offset 8 in the preview track template (`0xE0`,
   `0xE5`, or `0xF3` depending on the stock song). Hypothesis: loop duration
   scaled to preview length. Hardcoded to `0xE0` in our writer.

2. **The exact RPC code `0x000000F8`**: what runtime curve this references in
   DDR's XGS. We reproduce the stock value verbatim.

3. **Why sound ordering matters to the engine.** Empirically, the XACT2
   engine in DDR World only plays audio when the complex (preview) sound is
   written first and cue index 0 points at it. In-game testing shows that
   the inverse ordering (simple-first, cue 0 = main) produces silent audio
   despite being structurally valid. Several stock DDR XSBs use the inverse
   ordering and still play in-game, so the engine evidently accepts both in
   some circumstances — but reproducing those conditions from a clean slate
   does not. The complex-first ordering is the safe, universally-working choice.

4. **The XGS file itself**: DDR's `xactengine2_10.dll` references the `XGSF`
   magic but the XGS file is not in the update-package portion of the install
   we've analyzed. It must be in the base install or inside the game
   executable's resources. Understanding the XGS would let us know the
   semantic meaning of category indices 3 and 4; we use them as-is because
   all 11/12 majority stock files do.

## References

- `xactengine2_10.dll` (DDR's audio runtime)
  - `FUN_0040fad0` — cue name hash function
  - `FUN_00423d00` — `GetCueIndex` (vtable slot 0 of SoundBank COM interface)
  - `FUN_00424200` — CRC-16 computation / validation
  - `FUN_0040e970` — XSB structure validator
- [FAudio](https://github.com/FNA-XNA/FAudio) — open-source XACT
  reimplementation; source of high-level field names and format shape.
  Note: FAudio uses linear search for cue names and does not implement this
  hash function.
