#!/usr/bin/env python3
"""Throwaway analysis of sync-probe curves. Usage: analyze2.py <out_dir>

Evaluates onset features, templates, and peak-selection rules against the
community offsets, on stock charts and on synthetically shifted charts.
Empirically (see prototype-results.md) the community value c relates to the
measured offset m as m = T - c.
"""
import csv
import sys
from pathlib import Path

import numpy as np

out = Path(sys.argv[1])
rows = list(csv.DictReader(open(out / "songs.csv")))
order = (out / "curve_order.txt").read_text().split("\n")
CMS = int((out / "curve_ms.txt").read_text())
L = 2 * CMS + 1
idx = {name: i for i, name in enumerate(order)}
SHIFTS = [-200, -150, -100, -50, 0, 50, 100, 150, 200]

codes = [r["code"] for r in rows]
c = np.array([float(r["c"]) for r in rows])
bpm_max = np.array([float(r["bpm_max"]) for r in rows])
curves = {code: np.fromfile(out / "curves" / f"{code}.bin", dtype="<f4").reshape(len(order), L) for code in codes}


def get(code, feat, tpl, part="all"):
    if "+" in feat:
        a, b = feat.split("+")
        return norm(get(code, a, tpl, part)) + norm(get(code, b, tpl, part))
    return curves[code][idx[f"{feat}_{tpl}_{part}"]].astype(np.float64)


def norm(x):
    med = np.median(x)
    mad = np.median(np.abs(x - med)) * 1.4826
    return (x - med) / (mad if mad > 0 else 1.0)


def local_maxima(x):
    return [i for i in range(1, len(x) - 1) if x[i] >= x[i - 1] and x[i] >= x[i + 1]]


def refine(x, i):
    if 0 < i < len(x) - 1:
        a, b, cc = x[i - 1], x[i], x[i + 1]
        den = a - 2 * b + cc
        if abs(den) > 1e-12:
            return i + 0.5 * (a - cc) / den
    return float(i)


def select(x, center, half, rule, tau=0.9, voter=None):
    """Return (position in curve coords (ms), rival) for window [center-half, center+half]."""
    lo = CMS + center - half
    hi = CMS + center + half
    w = x[lo : hi + 1]
    med = np.median(w)
    imax = int(np.argmax(w))
    vmax = w[imax]
    peaks = sorted(set(local_maxima(w)) | {imax})
    heights = {i: (w[i] - med) / (vmax - med) if vmax > med else 0 for i in peaks}
    rival = max([h for i, h in heights.items() if abs(i - imax) > 25] + [0])
    if rule == "max":
        pick = imax
    elif rule.startswith("vote:"):
        # Top two candidates >25 ms apart; if the rival is close, let a
        # second feature's curve (evaluated near each) cast the deciding vote.
        pick = imax
        riv = [i for i in peaks if abs(i - imax) > 25]
        if riv and rival >= tau:
            j = max(riv, key=lambda i: heights[i])
            v = norm(voter)[lo : hi + 1]
            near = lambda i: v[max(0, i - 30) : i + 31].max()
            if near(j) > near(imax):
                pick = j
    elif rule == "nearzero":
        cands = [i for i in peaks if heights[i] >= tau]
        pick = min(cands, key=lambda i: abs(i - half))
    else:
        raise ValueError(rule)
    return refine(w, pick) + lo - CMS, rival


def voter_for(k, rule):
    if rule.startswith("vote:"):
        f, t = rule[5:].split("/")
        return get(k, f, t)
    return None


def evaluate(feat, tpl, rule, half_fn, tau=0.9):
    stock = np.array([select(get(k, feat, tpl), 0, half_fn(j), rule, tau, voter_for(k, rule))[0] for j, k in enumerate(codes)])
    T = np.median(stock + c)
    truth = T - c
    res: dict[object, float] = {"T": T}
    for s in SHIFTS:
        pos = np.array([select(get(k, feat, tpl), s, half_fn(j), rule, tau, voter_for(k, rule))[0] for j, k in enumerate(codes)])
        err = pos - truth  # recovered position should equal the stock truth
        res[s] = 100 * np.mean(np.abs(err) <= 5)
    return res


def fixed(h):
    return lambda j: h


def adaptive(frac, cap=200):
    return lambda j: int(min(cap, frac * 60000.0 / bpm_max[j]))


print(f"songs: {len(codes)}")
configs = []
for feat in ["flux", "rise", "rise1k", "riselp", "rise+riselp", "rise1k+riselp", "flux+riselp"]:
    configs.append((feat, "notes", "max", "fixed200", fixed(200)))
for feat in ["rise", "rise1k"]:
    configs.append((feat, "beats", "max", "fixed200", fixed(200)))
    configs.append((feat, "metric", "max", "fixed200", fixed(200)))
for feat in ["rise", "rise1k", "rise1k+riselp"]:
    configs.append((feat, "notes", "vote:riselp/notes", "fixed200", fixed(200)))
    configs.append((feat, "notes", "nearzero", "fixed200", fixed(200)))
    for frac in (0.4, 0.45):
        configs.append((feat, "notes", "max", f"adapt{frac}", adaptive(frac)))
hdr = f"{'feature':10s} {'template':7s} {'rule':16s} {'window':9s} {'T':>6s} " + " ".join(f"{s:+5d}" for s in SHIFTS)
print("% within 5 ms of community truth, by synthetic shift (0 = stock):")
print(hdr)
for feat, tpl, rule, wname, wfn in configs:
    r = evaluate(feat, tpl, rule, wfn)
    print(f"{feat:10s} {tpl:7s} {rule:16s} {wname:9s} {r['T']:+6.2f} " + " ".join(f"{r[s]:5.0f}" for s in SHIFTS))
