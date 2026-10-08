# Hudson Lane-Format SSQ Step Chunk (Type 16)

DDR Hottest Party 4 and DDR Hottest Party 5 (Wii) store each chart in a
**type 16** chunk. It replaces the type 9 chart used by Hottest Party 1 and
Mario Mix (`hudson_ssq_format.md`); the two share nothing beyond the 12-byte
chunk header. Where type 3 and type 9 rows are panel **masks**, a type 16
row names a single **lane**.

Everything here was verified against the HP4 and HP5 main DOLs and the full
set of charts on both discs (57 HP4 songs / 1,098 charts; 121 HP5 songs /
798 charts). Function addresses refer to the HP4 DOL unless noted. All
multi-byte values are **little-endian** (the PowerPC game byte-swaps them on
load).

Only foot charts are relevant to this tool. See §1.1 for the other chart
styles HP4 ships in the same chunk type.

## 1. Chunk header

Standard 12-byte SSQ header (`ssq_format.md` §2):

| Offset | Type | Field |
|--------|------|-------|
| +0x00 | u32 | chunk length incl. header, dword-aligned |
| +0x04 | u16 | `16` |
| +0x06 | u16 | `param2` — style code (low byte) and difficulty slot (high byte), see §1.1 |
| +0x08 | u16 | `param3` — row count N |
| +0x0A | u16 | 0 |

The dispatcher (`FUN_8002d020`) routes types 3, 9 and 16 into the same
chart-slot table, so a disc could mix them; HP4/HP5 discs use type 16 only.
Chunks are advanced by `length` rounded up to a dword.

### 1.1 Style codes

`FUN_8002d9e0` decodes `param2`. The difficulty high byte is the arcade one
(`0x01` Basic, `0x02` Difficult, `0x03` Expert, `0x04` Beginner; HP5 also
accepts `0x06` Challenge). The low byte is split into a low nibble (lane
layout) and a high nibble (controller variant):

| Low byte | Game slot | Meaning | Lanes | This tool |
|----------|-----------|---------|-------|-----------|
| `0x14` | 0 | Single, foot | 0–3 | converted |
| `0x18` | 2 | Double, foot (HP5 only) | 0–7 | converted |
| `0x1a` | 1 | Single, foot + Wii Remote hands | 0–3 foot, 4–5 hand | dropped |
| `0x24`, `0x34` | 3, 4 | Single, Wii-controller variants (HP4 only) | 0–3 | dropped |
| `0x2a`, `0x3a` | 5, 6 | foot + hands, controller variants (HP4 only) | 0–5 | dropped |
| `0x4a` | 7 | foot + hands, controller variant (HP4 only) | 0–5 | dropped |

Which controller each of slots 3–7 maps to (Balance Board, Remote-only,
Remote+Nunchuk, …) was not traced; none of them are dance-panel charts, so
the tool drops them with a `warn` and converts only `0x14` / `0x18`.

## 2. Body

| Offset | Type | Field |
|--------|------|-------|
| +0x0C | N × i32 | row ticks, measure-ticks (4096 per measure), ascending |
| +0x0C + 4N | N × u8 | row lane index (§2.1) |
| … | 0–3 bytes | pad so the item records start on a dword |
| … | N × 4 bytes | row item records (§2.1) |

Total body size is `4N + ceil4(N) + 4N`. There is **no** extra block, no
`CHAR` / `FREZ` table and no offset field; the loader (`FUN_8002c858`,
HP5 `FUN_8002ecd4`) computes the item-record offset directly from N.

### 2.1 Row record

A row is the i-th element of each of the three arrays:

- **tick** — i32 measure-ticks.
- **lane** — u8 panel/lane index: bit position in the type 3 mask (0 P1 Left
  … 3 P1 Right, 4 P2 Left … 7 P2 Right; 4–5 are hands in the `0x?a`
  styles). `0xFF` = no note; the row is skipped.
- **item** — `{ i16 type, u16 param }`:

| type | param | Meaning |
|------|-------|---------|
| 0 | 0 | plain arrow on `lane` |
| 0 | 1 | **freeze end** on `lane` (§3) |
| -1 | any | no note; the row is skipped |
| k ≥ 1 | any | arrow on `lane` carrying gimmick item `k` with parameter `param` (§4) |

### 2.2 Row merging

The loader walks rows in order and merges every run of rows whose tick is
`<=` the run's first tick into one step record (mask of arrow lanes) and one
hold record (mask of freeze-end lanes), exactly as a type 3 step byte and
freeze entry would be. Rows are always sorted ascending on disc, so a run is
simply "all rows at the same tick". A tick can therefore yield both a step
record and a hold record.

Hard limits: 0x800 step records, 0x800 hold records, 128 distinct gimmick
items per chart (`SSQ step count over` / `SSQ hold count over`).

## 3. Freeze ends

A `(0, 1)` item marks where a freeze on `lane` ends. It closes the most
recent earlier arrow on that lane, like a type 3 freeze entry with kind 1
(`ssq_format.md` §5.4). In the whole HP4/HP5 corpus every freeze end has a
head, no head and end share a tick, and no lane has two arrows at one tick.

When a freeze end and a new arrow on the same lane share a tick, the freeze
end must resolve first so it closes the earlier head rather than the arrow
starting at that tick. This tool emits the freeze-end row ahead of the step
row at equal ticks.

## 4. Gimmick items and "gimmicks off"

Foot charts (`0x14`, `0x18`) in the HP4 and HP5 corpus carry **only** type 0
items. HP4/HP5 moved foot-chart gimmicks out of the SSQ into separate files
(`ssq/GIMMICK/%s.bin`), so there is nothing to neutralize: a type 16 foot
chart is already the game's gimmicks-off chart.

Non-zero item types appear only in the hand styles (`0x?a`): HP4 uses types
1–7 with parameters 0–8 and 100–108 for Wii Remote actions. Their gameplay
semantics were not traced because those charts are not converted.

If a foot chart ever carries a non-zero item type, this tool keeps the arrow
(the loader always sets the step bit for it) and logs a `warn`. The same goes
for a type 0 item with a parameter other than 0 or 1.

## 5. Other chunks on these discs

- Type 1 tempo (TPS=150) and type 2 events as on HP1.
- Type 17 section markers as on arcade (`ssq_format.md` §9).
- **Type 18** (HP5 only, 4 songs): `param3 = N`, body `N × u32` ticks
  followed by `N × u32` zeros. Not consumed by the step engine; dropped as
  auxiliary.
- One HP5 file carries an arcade-style type 9 metadata chunk (`param2 = 0`),
  dropped as auxiliary.
