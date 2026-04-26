#!/usr/bin/env python3
"""Extract assets from DDR Ultramix archive files.

Two archive formats are supported, both flat and sector-aligned to 0x800:

    x_data bin (textures, charts, song-info files, etc.)
      The table-of-contents is a static array embedded in default.xbe.
      Each 16-byte entry: { name_va, size, size_aligned, offset }.

    music sng (audio streams; XBOX-IMA ADPCM, 44.1 kHz stereo, headerless)
      The table-of-contents lives in the first sector of the archive itself.
      Layout: u32 count, then `count` 20-byte entries of
      { tag[4], offset, size, loop_offset, loop_size }.
      `loop_*` describes a looping preview stream for song-wheel (zero for
      UI/non-song audio). Main streams are extracted as `{tag}.wavm`,
      loops as `{tag}_loop.wavm`.

Usage:
    extract_ultramix_data.py <game> <game_dir> <out_dir>

Example:
    extract_ultramix_data.py ultramix_us ~/Desktop/ultramix ./extracted
"""

import argparse
import csv
import struct
import sys
from pathlib import Path

# Per-game parameters, discovered by reverse-engineering default.xbe.
#   xdata_toc_offset / xdata_count / xdata_str_delta: x_data bin TOC in the XBE
#   xdata_bin:                                        x_data bin filename in game dir
#   sng:                                              music .sng filename in game dir
GAMES = {
    "ultramix_us": {
        "xdata_toc_offset": 0x1AD890,
        "xdata_count": 737,
        "xdata_str_delta": 0xE780,
        "xdata_bin": "x_data_US.bin",
        "sng": "music_US.sng",
    },
    "ultramix_uk": {
        "xdata_toc_offset": 0x1B06B0,
        "xdata_count": 737,
        "xdata_str_delta": 0xE780,
        "xdata_bin": "x_data_UK.bin",
        "sng": "music_UK.sng",
    },
}


def parse_xdata_toc(xbe_bytes, toc_offset, count, str_delta):
    """Yield (name, size, offset) for each non-empty x_data TOC entry."""
    for i in range(count):
        entry = xbe_bytes[toc_offset + i * 16 : toc_offset + (i + 1) * 16]
        name_va, size, _size_aligned, offset = struct.unpack("<IIII", entry)
        if name_va == 0 and size == 0 and offset == 0:
            continue  # unused slot
        name_off = name_va - str_delta
        end = xbe_bytes.find(b"\x00", name_off)
        name = xbe_bytes[name_off:end].decode("latin1")
        yield name, size, offset


def parse_sng_toc(sng_f):
    """Yield (tag, offset, size, loop_offset, loop_size) for each .sng entry."""
    sng_f.seek(0)
    (count,) = struct.unpack("<I", sng_f.read(4))
    toc_bytes = sng_f.read(count * 20)
    for i in range(count):
        tag, offset, size, loop_offset, loop_size = struct.unpack_from(
            "<4sIIII", toc_bytes, i * 20
        )
        yield tag.decode("ascii"), offset, size, loop_offset, loop_size


def extract_xdata(xbe_path, bin_path, out_dir, cfg):
    """Extract x_data bin contents and write a manifest. Returns file count."""
    xbe_bytes = xbe_path.read_bytes()
    entries = list(
        parse_xdata_toc(
            xbe_bytes,
            cfg["xdata_toc_offset"],
            cfg["xdata_count"],
            cfg["xdata_str_delta"],
        )
    )

    written = 0
    with open(bin_path, "rb") as bin_f, open(out_dir / "xdata_manifest.csv", "w", newline="") as mf:
        w = csv.writer(mf)
        w.writerow(["name", "size", "offset"])
        for name, size, offset in entries:
            w.writerow([name, size, f"0x{offset:x}"])
            if size == 0:
                continue
            bin_f.seek(offset)
            (out_dir / name).write_bytes(bin_f.read(size))
            written += 1
    return written


def extract_sng(sng_path, out_dir):
    """Extract .sng audio streams as {tag}.wavm and {tag}_loop.wavm. Returns file count."""
    written = 0
    with open(sng_path, "rb") as sng_f, open(out_dir / "sng_manifest.csv", "w", newline="") as mf:
        w = csv.writer(mf)
        w.writerow(["tag", "stream", "size", "offset"])
        entries = list(parse_sng_toc(sng_f))
        for tag, offset, size, loop_offset, loop_size in entries:
            for suffix, off, sz in [("", offset, size), ("_loop", loop_offset, loop_size)]:
                if sz == 0:
                    continue
                sng_f.seek(off)
                (out_dir / f"{tag}{suffix}.wavm").write_bytes(sng_f.read(sz))
                w.writerow([tag, suffix.lstrip("_") or "main", sz, f"0x{off:x}"])
                written += 1
    return written


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("game", choices=sorted(GAMES), help="game identifier")
    ap.add_argument("game_dir", type=Path, help="directory containing default.xbe, x_data bin, and music .sng")
    ap.add_argument("out_dir", type=Path, help="output directory for extracted files")
    args = ap.parse_args()

    cfg = GAMES[args.game]
    xbe_path = args.game_dir / "default.xbe"
    bin_path = args.game_dir / cfg["xdata_bin"]
    sng_path = args.game_dir / cfg["sng"]
    for p in (xbe_path, bin_path, sng_path):
        if not p.is_file():
            sys.exit(f"error: {p} not found")

    args.out_dir.mkdir(parents=True, exist_ok=True)

    xdata_count = extract_xdata(xbe_path, bin_path, args.out_dir, cfg)
    sng_count = extract_sng(sng_path, args.out_dir)

    print(f"x_data:  {xdata_count} files (manifest: xdata_manifest.csv)")
    print(f"sng:     {sng_count} files (manifest: sng_manifest.csv)")
    print(f"output:  {args.out_dir}")


if __name__ == "__main__":
    main()
