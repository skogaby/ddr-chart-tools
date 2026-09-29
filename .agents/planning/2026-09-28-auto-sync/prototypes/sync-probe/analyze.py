#!/usr/bin/env python3
"""Throwaway analysis for sync-probe output. Usage: analyze.py out.csv"""
import csv
import sys

import numpy as np

rows = list(csv.DictReader(open(sys.argv[1])))
FEATURES = ["flux", "superflux", "rise"]
TEMPLATES = ["notes", "beats"]
SHIFTS = [-200, -150, -100, -50, 50, 100, 150, 200]
c = np.array([float(r["c"]) for r in rows])
bpm_max = np.array([float(r["bpm_max"]) for r in rows])
print(f"songs: {len(rows)}   c: median {np.median(c):+.1f}  std {c.std():.2f}")


def col(p, s):
    return np.array([float(r[f"{p}_{s}"]) for r in rows])


def rstd(x):
    return 1.4826 * np.median(np.abs(x - np.median(x)))


def pct_within(x, lim):
    return 100.0 * np.mean(np.abs(x) <= lim)


for f in FEATURES:
    for t in TEMPLATES:
        p = f"{f}_{t}"
        m = col(p, "m")
        corr = np.corrcoef(m, c)[0, 1]
        # Sign test: which of m - c or m + c is tighter?
        res_minus, res_plus = m - c, m + c
        sign = -1 if rstd(res_plus) < rstd(res_minus) else +1
        res = m - sign * c
        T = np.median(res)
        e = res - T
        zero = c == 0
        h = np.abs(col(p, "h1") - col(p, "h2"))
        print(
            f"\n== {p}: corr(m,c)={corr:+.3f}  robust-std(m-c)={rstd(res_minus):.2f} "
            f"robust-std(m+c)={rstd(res_plus):.2f}  -> m ~ T {'+' if sign > 0 else '-'} c"
        )
        print(
            f"   T={T:+.2f} ms   T(c==0, n={zero.sum()})={np.median(m[zero]):+.2f}   "
            f"raw std(m)={rstd(m):.2f}   residual robust-std={rstd(e):.2f}"
        )
        print(
            "   |residual| P50={:.1f} P90={:.1f} P95={:.1f} max={:.1f}   within 3ms {:.0f}%  5ms {:.0f}%  10ms {:.0f}%".format(
                np.percentile(np.abs(e), 50), np.percentile(np.abs(e), 90), np.percentile(np.abs(e), 95),
                np.abs(e).max(), pct_within(e, 3), pct_within(e, 5), pct_within(e, 10),
            )
        )
        # How much does correcting by the estimate help vs leaving stock?
        print(
            "   stock |c| within 5ms {:.0f}%  -> after auto-sync within 5ms {:.0f}%".format(
                pct_within(c, 5), pct_within(e, 5)
            )
        )
        print(
            "   split-half |h1-h2| P50={:.1f} P90={:.1f}; corr(|h1-h2|, |res|)={:+.2f}".format(
                np.median(h), np.percentile(h, 90), np.corrcoef(h, np.abs(e))[0, 1]
            )
        )
        rival = col(p, "rival")
        z = col(p, "z")
        for lo, hi in [(0, 0.5), (0.5, 0.7), (0.7, 0.85), (0.85, 1.01)]:
            sel = (rival >= lo) & (rival < hi)
            if sel.sum():
                print(
                    f"   rival [{lo:.2f},{hi:.2f}): n={sel.sum():4d}  |res| P90={np.percentile(np.abs(e[sel]), 90):5.1f}  within 5ms {pct_within(e[sel], 5):3.0f}%"
                )
        recov = []
        for s in SHIFTS:
            rs = col(p, f"shift{s}")
            ok = np.abs(rs - m) <= 5
            recov.append(f"{s:+d}:{100 * ok.mean():.0f}%")
        print("   shift recovery (|recovered - m| <= 5ms): " + "  ".join(recov))
