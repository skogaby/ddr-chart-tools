# DDR Ultramix Archive Formats

Byte-level specification of the two archive formats used by DDR Ultramix (Xbox, 2003)
for packing game assets. Reverse-engineered from the US release's `default.xbe`;
the same structure is reused by the EU release (Dancing Stage Unleashed) with
different offsets.

The extractor script at `scripts/extract_ultramix_data.py` implements this spec.

## File Layout on Disc

```
default.xbe       Game executable (contains the x_data TOC as static data)
x_data_US.bin     Textures, charts, song-info, background-animation data, etc.
music_US.sng      Audio streams (XBOX-IMA ADPCM, 44.1 kHz stereo, headerless)
```

Both archives are flat and sector-aligned to 0x800 (2048 bytes, the Xbox DVD read granule).

## x_data Bin Format

The bin is a concatenation of sector-aligned payloads. It has **no header of its
own** — the table-of-contents lives in the XBE as a static array of 16-byte
entries.

### Locating the TOC in the XBE

The TOC is at a game-specific file offset within `default.xbe`. For the US release:

| Parameter | Value (US) | Value (EU) |
|-----------|------------|------------|
| TOC file offset | `0x1AD890` | `0x1B06B0` |
| Entry count (upper bound) | 737 | 737 |
| String VA→file delta | `0xE780` | `0xE780` |

The entry count comes from the lookup function's loop bound (`CMP ESI, 0x2E10`
at `0x31FCA` in the US XBE, where `0x2E10 / 0x10 = 737`).

### TOC Entry (16 bytes, little-endian)

```c
struct x_data_entry {
    uint32_t name_va;       // +0x00  VA of null-terminated filename (in XBE .rdata)
    uint32_t size;          // +0x04  actual file size in bytes
    uint32_t size_aligned;  // +0x08  size rounded up to next 0x800 boundary
                            //        (number of bytes the game reads via ReadFileEx)
    uint32_t offset;        // +0x0C  byte offset within the x_data bin (0x800-aligned)
};
```

To resolve `name_va` to a file offset in the XBE, subtract the delta:
`name_file_offset = name_va - 0xE780`. The name is ASCII, null-terminated.

For all 737 Ultramix entries, `size_aligned == roundup(size, 0x800)` — confirming
the field exists to let the game issue sector-granular reads without computing
the rounded size at runtime. Extractors only need `size`.

A handful of entries are fully zero (e.g. the `all.ssq` sentinel used as a
fallback). Skip these.

### Lookup Semantics (for reference)

From `FUN_00031F90` in the US XBE:

- Filenames with `\` are rejected (archive is flat, no subdirectories).
- Lookup is **case-insensitive** (`_stricmp`) — filenames in the TOC preserve
  case but matching ignores it.
- Linear scan, O(n) per lookup. The TOC is not sorted or hashed.

### What's In It

737 entries totaling ~126 MB (the bin is ~127 MB; ~1 MB is sector padding).

For Ultramix 1 US:

| Extension | Count | What it is |
|-----------|-------|------------|
| `.tga`    | 249   | Uncompressed textures (32-bit BGRA) |
| `.dds`    | 231   | Compressed textures |
| `.ssq`    | 60    | Chart files (legacy DDR format; TPS=75 or 150 per file) |
| `.csv`    | 54    | Per-song background-animation stage direction |
| `.sif`    | 51    | Per-song info (title, subtitle, artist) |
| `.act`    | 33    | Animation curve data |
| `.ani`    | 27    | Character animations |
| `.bmp`    | 22    | Pad/arrow indicator graphics |
| `.txt`    | 3     | Readme, credits |
| `.pd`     | 3     | Particle/effect scripts |
| `.ddm`    | 2     | Unknown, very small |
| `.xpu`    | 2     | Pixel shader blobs |

Per-song asset group (by 4-char song ID, e.g. `abs2`):

- `{id}.sif` — song info file (see below)
- `{id}.csv` — background animation / camera / lighting choreography
- `{id}_all.ssq` — chart data (all difficulties)
- `{id}_bk.dds` + `{id}_bk.tga` — background image (large)
- `{id}_tb.dds` + `{id}_tb.tga` — banner (medium)
- `{id}_th.dds` + `{id}_th.tga` — thumbnail (small)

Some songs have extra SSQs with suffixes like `_org_all.ssq` (early/unused
charts, documented on TCRF) or `_all_chris.ssq` (developer attribution).

## .sif — Song Info File

Null-terminated ASCII strings at fixed indices, padded with zero bytes to a
fixed total size (typically 512-1024 bytes). The file begins with a single
empty leader field; actual content starts at index 1:

```
[0]  empty leader  (a single NUL byte)
[1]  short ID      e.g. "abs2"
[2]  title         e.g. "ABSOLUTE"
[3]  subtitle      e.g. "Cuff -N- Stuff it Mix"  (may be empty)
[4]  artist        e.g. "Thuggie D."
[5+] trailing NUL padding to the file's fixed size
```

The short ID in field [1] matches both the `.sif` filename prefix and the audio
tag in `music_US.sng`.

## music .sng Format

Unlike the bin, the `.sng`'s TOC is stored **inline at the start of the file**,
not in the XBE.

### File Layout

```
+0x0000   u32   entry_count (N)
+0x0004   N × 20-byte entries
+0x0800   payload data (sector-aligned)
...
```

Entry 0's payload starts at offset `0x800` — the first sector is exclusively
for the TOC (plenty of room; the TOC for Ultramix 1 US has 61 entries = 1224
bytes, leaving the rest of the sector as padding).

### .sng Entry (20 bytes, little-endian)

```c
struct sng_entry {
    char     tag[4];         // +0x00  4 ASCII chars (compared as u32 integer, not string)
    uint32_t offset;         // +0x04  file offset of main stream (0x800-aligned)
    uint32_t size;           // +0x08  main stream size in bytes
    uint32_t loop_offset;    // +0x0C  file offset of preview-loop stream (0 if none)
    uint32_t loop_size;      // +0x10  preview-loop stream size (0 if none)
};
```

### Tag Semantics

- Entries with `loop_* == 0` are **UI/non-song audio**: menu BGM, intro stinger,
  credits roll, live-mode music, etc. Tags: `uime`, `uire`, `uigo`, `uisp`,
  `uirc`, `intr`, `inus`, `inuk`, `cred`, `live`.
- Entries with `loop_* != 0` are **songs**: `main` is the full track played
  during gameplay, `loop` is the ~8-second preview clip that loops on the song
  wheel. The tag matches the corresponding `.sif` / `.ssq` / texture filenames
  in x_data.

### Lookup Semantics (for reference)

From `FUN_0005AD00` in the US XBE:

- The first sector of the `.sng` is read into memory once at startup and kept
  resident.
- Lookup iterates the copied TOC and compares the input key (a 4-char tag
  packed as a `u32`) against entry +0x00 with `CMP`. Linear scan.

### Audio Format

The stream data is headerless **XBOX-IMA ADPCM**, stereo, 44.1 kHz. Block size
is 0x48 bytes (stereo), producing 64 samples per channel. This is exactly the
WAVM format the `src/wavm/` decoder in this crate already handles.

Stream sizes are always exact multiples of 0x48 (verified across all entries).

## Ultramix 2/3/4

Not yet investigated. The bin and sng filenames follow the same convention
(`x_data_US.bin`, `music_US.sng`), and TCRF documents offsets of known content
using the same "all offsets relative to `x_data_US.bin`" phrasing — suggesting
the container format is reused. The TOC location within the XBE likely differs
per game; a short Ghidra session on each `default.xbe` would confirm.
