#!/usr/bin/env python3
"""Throwaway: narrow-window (±55/±60 ms) precision study against the community values.
Usage: analyze4.py <out_dir>"""
import csv
import sys
from pathlib import Path

import numpy as np

out = Path(sys.argv[1])
rows = list(csv.DictReader(open(out / "songs.csv")))
order = (out / "curve_order.txt").read_text().split("\n")
CMS = int((out / "curve_ms.txt").read_text())
L = 2 * CMS + 1
idx = {n: i for i, n in enumerate(order)}
codes = [r["code"] for r in rows]
c = np.array([float(r["c"]) for r in rows])
bpm_max = np.array([float(r["bpm_max"]) for r in rows])
bpm_min = np.array([float(r["bpm_min"]) for r in rows])
n_notes = np.array([float(r["n_notes"]) for r in rows])
cur = {k: np.fromfile(out / "curves" / f"{k}.bin", dtype="<f4").reshape(len(order), L) for k in codes}


def rstd(x):
    return 1.4826 * np.median(np.abs(x - np.median(x)))


def est(x, center, half, rival_excl=15, edge_margin=3):
    lo, hi = CMS + center - half, CMS + center + half
    w = x[lo : hi + 1].astype(np.float64)
    med = np.median(w)
    i = int(np.argmax(w))
    v = w[i]
    pos = float(i)
    if 0 < i < len(w) - 1:
        a, b, cc = w[i - 1], w[i], w[i + 1]
        d = a - 2 * b + cc
        if abs(d) > 1e-12:
            pos += 0.5 * (a - cc) / d
    lm = [j for j in range(1, len(w) - 1) if w[j] >= w[j - 1] and w[j] >= w[j + 1] and abs(j - i) > rival_excl]
    rival = max([(w[j] - med) / (v - med) for j in lm] + [0]) if v > med else 1.0
    edge = min(i, len(w) - 1 - i) < edge_margin
    return pos + lo - CMS, rival, edge


def curve(k, feat, tpl, part="all"):
    return cur[k][idx[f"{feat}_{tpl}_{part}"]]


def measure(feat, tpl, half, center=0):
    E = [est(curve(k, feat, tpl), center, half) for k in codes]
    H1 = np.array([est(curve(k, feat, tpl, "h1"), center, half)[0] for k in codes])
    H2 = np.array([est(curve(k, feat, tpl, "h2"), center, half)[0] for k in codes])
    m = np.array([e[0] for e in E])
    rival = np.array([e[1] for e in E])
    edge = np.array([e[2] for e in E])
    return m, rival, edge, np.abs(H1 - H2)


def report(label, e, accept=None):
    if accept is None:
        accept = np.ones_like(e, dtype=bool)
    ea = e[accept]
    ours_int = np.round(ea)  # the applied correction is whole ms at TPS=1000
    print(
        f"{label:34s} acc {100 * accept.mean():5.1f}%  rstd {rstd(ea):4.2f}  "
        f"P50 {np.percentile(abs(ea), 50):4.2f} P90 {np.percentile(abs(ea), 90):5.2f} P99 {np.percentile(abs(ea), 99):6.2f}  "
        f"<=0.5 {100 * np.mean(abs(ea) <= 0.5):4.1f}%  <=1 {100 * np.mean(abs(ea) <= 1):4.1f}%  <=2 {100 * np.mean(abs(ea) <= 2):4.1f}%  <=3 {100 * np.mean(abs(ea) <= 3):4.1f}%  "
        f"int==: {100 * np.mean(ours_int == 0):4.1f}%  int±1: {100 * np.mean(abs(ours_int) <= 1):4.1f}%"
    )


print(f"songs {len(codes)}; stock |c| <=1: {100 * np.mean(abs(c) <= 1):.1f}%  <=2: {100 * np.mean(abs(c) <= 2):.1f}%  <=3: {100 * np.mean(abs(c) <= 3):.1f}%")
print("\n# residual e = (our correction) - (community correction); e = m + c - T. All songs, no gating unless noted.")
results = {}
for half in (200, 60, 55):
    for feat in ("flux", "rise", "rise1k", "riselp"):
        for tpl in ("notes", "metric", "beats"):
            m, rival, edge, split = measure(feat, tpl, half)
            T = np.median(m + c)
            e = m + c - T
            results[(half, feat, tpl)] = (m, rival, edge, split, T)
            if half != 200 or tpl == "notes":
                report(f"±{half} {feat}/{tpl} T={T:+.2f}", e)

print("\n# ensembles (mean of per-feature corrections), ±60")
for combo in (("rise", "rise1k"), ("rise1k", "flux"), ("rise", "rise1k", "flux"), ("rise1k", "rise1k_metric"), ("rise1k", "rise1k_beats")):
    es = []
    for f in combo:
        feat, tpl = (f.split("_") + ["notes"])[:2]
        m, _, _, _, T = results[(60, feat, tpl)]
        es.append(m - T)
    e = np.mean(es, axis=0) + c
    report("±60 mean(" + "+".join(combo) + ")", e)

print("\n# gating at ±60, rise1k/notes")
m, rival, edge, split, T = results[(60, "rise1k", "notes")]
e = m + c - T
for rt in (0.8, 0.9, 1.01):
    for st in (3, 5, 10, 1e9):
        acc = (rival < rt) & (split <= st) & ~edge
        report(f"rival<{rt} split<={st:g} !edge", e, acc)

print("\n# what explains the residual? (rise1k/notes ±60, |e|<10 to exclude outliers)")
sel = np.abs(e) < 10
for name, x in (("c", c), ("bpm_max", bpm_max), ("n_notes", n_notes), ("split", split), ("rival", rival)):
    print(f"   corr(e, {name:8s}) = {np.corrcoef(e[sel], x[sel])[0, 1]:+.3f}")
# slope of e vs c: if our corrections are systematically shrunk/stretched vs theirs
A = np.vstack([c[sel], np.ones(sel.sum())]).T
slope, icpt = np.linalg.lstsq(A, e[sel], rcond=None)[0]
print(f"   fit e = {slope:+.3f}*c {icpt:+.3f}")
for lo, hi in ((0, 130), (130, 160), (160, 190), (190, 1000)):
    s = sel & (bpm_max >= lo) & (bpm_max < hi)
    print(f"   bpm_max [{lo},{hi}): n={s.sum():4d} median e {np.median(e[s]):+.2f}  rstd {rstd(e[s]):.2f}")

print("\n# large |c| songs (what the user most needs right): rise1k/notes ±60")
for lim in (5, 10, 15):
    s = np.abs(c) >= lim
    report(f"|c|>={lim} (n={s.sum()})", e[s] if False else e, s)

print("\n# synthetic shifts, rise1k/notes, ±60 window centered on the (shifted) source; truth from community")
m0, _, _, _, T0 = results[(60, "rise1k", "notes")]
truth = T0 - c
for sh in (-55, -50, -40, -25, 0, 25, 40, 50, 55):
    ms, rv, ed, sp = measure("rise1k", "notes", 60, center=sh)
    err = ms - truth
    acc = (rv < 0.9) & (sp <= 10) & ~ed
    ok = acc & (abs(err) <= 2)
    print(f"   shift {sh:+4d}: accepted {100 * acc.mean():5.1f}%  within 2 ms {100 * ok.mean():5.1f}%  accepted-but->2ms {100 * (acc & (abs(err) > 2)).mean():4.1f}%  accepted-but->5ms {100 * (acc & (abs(err) > 5)).mean():4.1f}%")
