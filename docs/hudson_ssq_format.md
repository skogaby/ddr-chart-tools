# Hudson SSQ Step Chunk (Type 9) Reference

Hudson Soft's Wii/GameCube DDR titles (DDR Mario Mix, DDR Hottest Party 1)
store each chart in a **type 9** chunk instead of the standard type 3 step
chunk. The chunk is a type 3 chart extended with per-arrow "chara note"
(gimmick item) references and a trailing extra-data block.

Everything here was verified against the DDR Hottest Party 1 (Wii) main DOL
and the 52 songs / 211 charts shipped on that disc. Function addresses refer
to that DOL. All multi-byte values are **little-endian** (the PowerPC game
byte-swaps them on load).

> Not to be confused with the arcade type 9 "song metadata" chunk
> (`ssq_format.md` §8). The two are told apart by `param2`: a Hudson step
> chunk carries a valid step difficulty code there (§1); the arcade chunk does
> not.

---

## 1. Header

Standard 12-byte SSQ chunk header (`ssq_format.md` §2):

| Offset | Type | Field |
|--------|------|-------|
| +0x00 | u32 | chunk length (dword-aligned) |
| +0x04 | u16 | type = `9` |
| +0x06 | u16 | `param2` — difficulty code, same encoding as type 3 (`ssq_format.md` §5.1) |
| +0x08 | u16 | `param3` — row count `N` |
| +0x0A | u16 | `param4` — unused (0) |

## 2. Body

| Offset | Type | Field |
|--------|------|-------|
| +0x0C | u32 | `extra_offset` — offset of the extra block (§3), **relative to chunk start + 0x10**. 0 = no extra block. |
| +0x10 | N × i32 | row time offsets, measure-ticks (4096 per measure), as type 3 |
| … | variable | N row records (§2.1), packed with no padding between them |
| … | 0–3 bytes | pad to the next dword |

### 2.1 Row record

Each row is one mask byte followed by zero or more item bytes:

- **mask ≠ 0** — a step row. One item byte follows for each set bit of the
  mask, lowest bit first. Item byte `0` = plain arrow; `k ≥ 1` = entry `k`
  (1-based) of the `CHAR` table (§3.1).
- **mask = 0** — a freeze-end row. No item bytes follow here; its data is the
  next entry of the `FREZ` table (§3.2).

Bit layout of the mask is the type 3 panel layout (bit 0 P1 Left … bit 7 P2
Right). The Hottest Party engine only builds notes from the low nibble.

## 3. Extra block

Located at `chunk start + 0x10 + extra_offset`:

| Offset | Type | Field |
|--------|------|-------|
| +0x00 | 4 bytes | magic `EXDT` (not validated by the game) |
| +0x04 | u32 | block length, including this 8-byte header |
| +0x08 | … | sub-blocks |

Each sub-block is `{ 4-byte ASCII magic, u32 length including its 8-byte
header, payload }`. The game (`FUN_801603e4`) walks the sub-blocks by length
and looks them up by magic. Two are defined: `CHAR` and `FREZ`. Either can be
absent.

### 3.1 `CHAR` — chara note table

`(length − 8) / 12` entries of 12 bytes:

| Offset | Type | Field |
|--------|------|-------|
| +0 | u16 | item type (§4) |
| +2 | u16 | unused |
| +4 | i32 | parameter 1 (always 0 on HP1) |
| +8 | i32 | parameter 2 (always 0 on HP1) |

### 3.2 `FREZ` — freeze-end table

One **fixed 3-byte** entry per freeze-end row, in row order:

| Offset | Type | Field |
|--------|------|-------|
| +0 | u8 | panel mask of the freezes that end here |
| +1 | u8 | kind (always `1`; the game ignores it) |
| +2 | u8 | item byte (the game ignores it) |

A freeze end closes, for each bit of its mask, the most recent earlier note on
that panel in the game's note list (`FUN_80161c08`). That list is in row
order, with item-2 echo notes (§4) appended straight after the row that made
them.

## 4. Item types and "gimmicks off" behavior

The note builder (`FUN_8016137c`) decides per arrow. With **Groove Gimmick
off** and **hand markers off**, every arrow becomes a normal step except:

| Item type | Hottest Party meaning | Gimmicks off |
|-----------|-----------------------|--------------|
| 0 (none), 3, 4, 7, 9, 10, 11, others | assorted gimmick arrows | normal arrow |
| 1 | Wii Remote "hand" marker (Left/Right panels only) | normal arrow |
| 2 | "Koopa" double arrow | normal arrow **plus an echo arrow on the same panel 1024 ticks (one beat) later**; at most 2 echoes per row |
| 5, 14 | hazard ("Spiny") — mine-like | **removed** |

Freezes are kept whatever item their head carries. A freeze whose end resolves
to a removed hazard note disappears with it.

With hand markers **on**, an item-1 arrow on panel 0 or 3 becomes a hand-only
note (no foot arrow) instead. This tool does not model that mode.

The option gates involved: per-player option word bit 29 (hand markers) and
the per-player Groove Gimmick flag at `0x802f4cfc`. With gimmicks on, item
types in the set `{2, 3, 4, 5, 7, 9, 14}` keep their gimmick behavior.
