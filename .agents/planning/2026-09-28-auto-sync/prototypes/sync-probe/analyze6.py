#!/usr/bin/env python3
"""Throwaway: 'True to DDR' (integer-ms anchors) vs raw SSQ timing, split by TPS.
Usage: analyze6.py <raw_out_dir> <ddr_out_dir> <ssq2sm.py dir>"""
import os
import sys
import numpy as np

RAW, DDR, SSQ2SM = sys.argv[1], sys.argv[2], sys.argv[3]
sys.path.insert(0, SSQ2SM)
import ssq2sm  # noqa: E402

results = {}
for label, d in (("raw", RAW), ("ddr", DDR)):
    sys.argv = [sys.argv[0], d]
    ns = {}
    exec(open("analyze4.py").read().split('print(f"songs {len(codes)}')[0], ns)
    m, rival, edge, split = ns["measure"]("rise", "notes", 70)
    results[label] = (ns["codes"], ns["c"], m, split)
codes, c = results["raw"][0], results["raw"][1]
assert codes == results["ddr"][0]
data = os.environ["DDR_WORLD_INSTALL"] + "/data"
tps = np.array([ssq2sm.parse_tempo(open(f"{data}/mdb_apx/ssq/{k}.ssq", "rb").read())[1] for k in codes])


def rstd(x):
    return 1.4826 * np.median(np.abs(x - np.median(x)))


print(f"{'timing':6s} {'subset':9s} {'n':>5s} {'T':>6s} {'rstd':>5s} {'<=0.5':>6s} {'<=1':>6s} {'<=2':>6s} {'<=3':>6s} {'int==':>6s} {'int±1':>6s}")
for label in ("raw", "ddr"):
    _, _, m, split = results[label]
    T = np.median(m + c)
    e = m + c - T
    for name, sel in (("all", np.ones_like(c, bool)), ("TPS 1000", tps == 1000), ("TPS 150", tps == 150)):
        ea = e[sel]
        print(f"{label:6s} {name:9s} {sel.sum():5d} {T:+6.2f} {rstd(ea):5.3f} "
              f"{100 * np.mean(abs(ea) <= 0.5):5.1f}% {100 * np.mean(abs(ea) <= 1):5.1f}% {100 * np.mean(abs(ea) <= 2):5.1f}% {100 * np.mean(abs(ea) <= 3):5.1f}% "
              f"{100 * np.mean(np.round(ea) == 0):5.1f}% {100 * np.mean(abs(np.round(ea)) <= 1):5.1f}%")
mr, md = results["raw"][2], results["ddr"][2]
diff = md - mr
print(f"\nper-song shift of our estimate, ddr - raw timing: TPS1000 max |d| {np.abs(diff[tps == 1000]).max():.3f} ms;"
      f" TPS150 median {np.median(diff[tps == 150]):+.3f} P90 |d| {np.percentile(abs(diff[tps == 150]), 90):.3f} max |d| {np.abs(diff[tps == 150]).max():.3f} ms")
