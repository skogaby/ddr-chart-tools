#!/usr/bin/env python3
"""Throwaway: headline stats, failure anatomy, confidence gating.
Usage: analyze3.py <out_dir> <feature> <template>"""
import csv
import sys
from pathlib import Path

import numpy as np

out, FEAT, TPL = Path(sys.argv[1]), sys.argv[2], sys.argv[3]
rows = list(csv.DictReader(open(out / "songs.csv")))
order = (out / "curve_order.txt").read_text().split("\n")
CMS = int((out / "curve_ms.txt").read_text())
L = 2 * CMS + 1
idx = {n: i for i, n in enumerate(order)}
codes = [r["code"] for r in rows]
c = np.array([float(r["c"]) for r in rows])
bpm_max = np.array([float(r["bpm_max"]) for r in rows])
bpm_min = np.array([float(r["bpm_min"]) for r in rows])
n_stops = np.array([int(r["n_stops"]) for r in rows])
dur = np.array([float(r["dur_s"]) for r in rows])
cur = {k: np.fromfile(out / "curves" / f"{k}.bin", dtype="<f4").reshape(len(order), L) for k in codes}


def norm(x):
    med = np.median(x)
    mad = np.median(np.abs(x - med)) * 1.4826
    return (x - med) / (mad if mad > 0 else 1.0)


def get(k, feat, tpl, part="all"):
    if "+" in feat:
        a, b = feat.split("+")
        return norm(get(k, a, tpl, part)) + norm(get(k, b, tpl, part))
    return cur[k][idx[f"{feat}_{tpl}_{part}"]].astype(np.float64)


def est(x, center, half):
    lo, hi = CMS + center - half, CMS + center + half
    w = x[lo : hi + 1]
    med = np.median(w)
    i = int(np.argmax(w))
    v = w[i]
    pos = float(i)
    if 0 < i < len(w) - 1:
        a, b, cc = w[i - 1], w[i], w[i + 1]
        d = a - 2 * b + cc
        if abs(d) > 1e-12:
            pos += 0.5 * (a - cc) / d
    lm = [j for j in range(1, len(w) - 1) if w[j] >= w[j - 1] and w[j] >= w[j + 1] and abs(j - i) > 25]
    rival = max([(w[j] - med) / (v - med) for j in lm] + [0]) if v > med else 1.0
    mad = np.median(np.abs(w - med)) * 1.4826
    z = (v - med) / mad if mad > 0 else 0
    edge = min(i, len(w) - 1 - i) < 5
    return pos + lo - CMS, rival, z, edge


def rstd(x):
    return 1.4826 * np.median(np.abs(x - np.median(x)))


def run(half_fn, label):
    E = [est(get(k, FEAT, TPL), 0, half_fn(j)) for j, k in enumerate(codes)]
    m = np.array([e[0] for e in E])
    rival = np.array([e[1] for e in E])
    z = np.array([e[2] for e in E])
    H = [(est(get(k, FEAT, TPL, "h1"), 0, half_fn(j))[0], est(get(k, FEAT, TPL, "h2"), 0, half_fn(j))[0]) for j, k in enumerate(codes)]
    split = np.array([abs(a - b) for a, b in H])
    T = np.median(m + c)
    e = m + c - T
    zero = c == 0
    print(f"\n#### {FEAT}/{TPL} window={label}  n={len(codes)}")
    print(f"corr(m, c) = {np.corrcoef(m, c)[0, 1]:+.3f}; robust-std(m-c) = {rstd(m - c):.2f}, robust-std(m+c) = {rstd(m + c):.2f}")
    print(f"T = median(m + c) = {T:+.2f} ms; median(m | c == 0, n={zero.sum()}) = {np.median(m[zero]):+.2f} ms")
    print(f"residual robust-std = {rstd(e):.2f} ms; |res| P50 {np.percentile(abs(e), 50):.2f}  P90 {np.percentile(abs(e), 90):.2f}  P95 {np.percentile(abs(e), 95):.1f}  P99 {np.percentile(abs(e), 99):.1f}")
    for lim in (1, 2, 3, 5, 10, 20):
        print(f"  within {lim:2d} ms: {100 * np.mean(abs(e) <= lim):5.1f}%   (stock |c| within {lim}: {100 * np.mean(abs(c) <= lim):5.1f}%)")
    bad = abs(e) > 5
    beat = 60000.0 / bpm_max
    frac = e / beat
    halfbeat = bad & (abs(abs(frac) - 0.5) < 0.06)
    fullbeat = bad & (abs(abs(frac) - 1.0) < 0.06)
    print(f"failures (|res|>5): {bad.sum()}  half-beat: {halfbeat.sum()}  one-beat: {fullbeat.sum()}  other: {(bad & ~halfbeat & ~fullbeat).sum()}")
    other = np.where(bad & ~halfbeat & ~fullbeat)[0]
    for j in other[:25]:
        print(f"   {codes[j]:8s} c={c[j]:+4.0f} m={m[j]:+7.1f} res={e[j]:+7.1f} bpm={bpm_min[j]:.0f}-{bpm_max[j]:.0f} stops={n_stops[j]} rival={rival[j]:.2f} z={z[j]:.1f} split={split[j]:.1f} dur={dur[j]:.0f}")
    print("gating (refuse if rival >= t OR split-half > s):")
    for t in (0.8, 0.85, 0.9, 0.95, 1.01):
        for s in (5, 10, 1e9):
            acc = (rival < t) & (split <= s)
            if acc.sum() == 0:
                continue
            wrong = np.mean(abs(e[acc]) > 5) * 100
            print(f"   rival<{t:.2f} split<={s:>4.0f}: accept {100 * acc.mean():5.1f}%  wrong among accepted {wrong:4.1f}%  ({(abs(e[acc]) > 5).sum()} songs)")


run(lambda j: 200, "fixed200")
run(lambda j: int(min(200, 0.45 * 60000.0 / bpm_max[j])), "adapt0.45")


def harm(half_fn, t=0.9, s=10):
    E = [est(get(k, FEAT, TPL), 0, half_fn(j)) for j, k in enumerate(codes)]
    m = np.array([e[0] for e in E])
    rival = np.array([e[1] for e in E])
    split = np.array([abs(est(get(k, FEAT, TPL, "h1"), 0, half_fn(j))[0] - est(get(k, FEAT, TPL, "h2"), 0, half_fn(j))[0]) for j, k in enumerate(codes)])
    T = np.median(m + c)
    e = m + c - T
    acc = (rival < t) & (split <= s)
    # After auto-sync the song's remaining error is e (accepted) or c (refused, untouched).
    after = np.where(acc, e, -c)  # -c: stock error expressed in the same sign as e
    print(f"\n## harm check, accept if rival<{t} & split<={s}: accepted {acc.sum()}/{len(codes)}")
    for lim in (5, 10, 20, 50):
        print(f"   accepted with |res|>{lim:2d}: {(acc & (abs(e) > lim)).sum()}")
    worse = acc & (abs(e) > abs(c) + 2)
    print(f"   accepted and made worse than stock by >2 ms: {worse.sum()}  (of which >20 ms: {(worse & (abs(e) > 20)).sum()})")
    for lim in (2, 5, 10, 20):
        print(f"   songs within {lim:2d} ms: stock {100 * np.mean(abs(c) <= lim):5.1f}%  -> after {100 * np.mean(abs(after) <= lim):5.1f}%")


harm(lambda j: int(min(200, 0.45 * 60000.0 / bpm_max[j])))
harm(lambda j: 200)
t_dec = np.array([float(r["t_decode_ms"]) for r in rows])
t_rise = np.array([float(r.get("t_rise1k_ms", 0)) for r in rows])
t_curves = np.array([float(r["t_curves_ms"]) for r in rows])
n_feat = sum(1 for k in rows[0] if k.startswith("t_") and k.endswith("_ms") and k not in ("t_decode_ms", "t_curves_ms"))
print(f"\ntiming per song (ms, 12 threads contended): decode P50 {np.median(t_dec):.0f}; one rise feature P50 {np.median(t_rise):.0f} P95 {np.percentile(t_rise, 95):.0f}; curves for all {n_feat} features x 3 templates x 3 parts P50 {np.median(t_curves):.0f}; duration P50 {np.median(dur):.0f}s max {dur.max():.0f}s")


def shifted(half_fn, label, t=0.9, s_lim=10):
    print(f"\n## shifted-source recovery with gating (rival<{t}, split<={s_lim}), window={label}")
    base = np.array([est(get(k, FEAT, TPL), 0, half_fn(j))[0] for j, k in enumerate(codes)])
    T = np.median(base + c)
    truth = T - c
    print("   shift   correct  wrong  refused   (correct = within 5 ms of community truth)")
    for sh in (-200, -150, -100, -50, 0, 50, 100, 150, 200):
        ok = wrong = ref = 0
        for j, k in enumerate(codes):
            pos, rival, _, edge = est(get(k, FEAT, TPL), sh, half_fn(j))
            h1 = est(get(k, FEAT, TPL, "h1"), sh, half_fn(j))[0]
            h2 = est(get(k, FEAT, TPL, "h2"), sh, half_fn(j))[0]
            if rival >= t or abs(h1 - h2) > s_lim or edge:
                ref += 1
            elif abs(pos - truth[j]) <= 5:
                ok += 1
            else:
                wrong += 1
        n = len(codes)
        print(f"   {sh:+5d}   {100 * ok / n:5.1f}%  {100 * wrong / n:5.1f}%  {100 * ref / n:5.1f}%")


shifted(lambda j: int(min(200, 0.45 * 60000.0 / bpm_max[j])), "adapt0.45")
shifted(lambda j: 200, "fixed200")


def hybrid(t=0.9, s_lim=10):
    wide = lambda j: 200
    narrow = lambda j: int(min(200, 0.45 * 60000.0 / bpm_max[j]))
    print(f"\n## hybrid: wide ±200 gated; if refused, narrow ±0.45 beat around the source sync, gated")
    base = np.array([est(get(k, FEAT, TPL), 0, narrow(j))[0] for j, k in enumerate(codes)])
    T = np.median(base + c)
    truth = T - c

    def attempt(k, j, sh, hf):
        pos, rival, _, edge = est(get(k, FEAT, TPL), sh, hf(j))
        h1 = est(get(k, FEAT, TPL, "h1"), sh, hf(j))[0]
        h2 = est(get(k, FEAT, TPL, "h2"), sh, hf(j))[0]
        return None if (rival >= t or abs(h1 - h2) > s_lim or edge) else pos

    print("   shift   correct  wrong  refused")
    for sh in (-150, -100, -50, 0, 50, 100, 150):
        ok = wrong = ref = 0
        for j, k in enumerate(codes):
            pos = attempt(k, j, sh, wide)
            if pos is None:
                pos = attempt(k, j, sh, narrow)
            if pos is None:
                ref += 1
            elif abs(pos - truth[j]) <= 5:
                ok += 1
            else:
                wrong += 1
        n = len(codes)
        print(f"   {sh:+5d}   {100 * ok / n:5.1f}%  {100 * wrong / n:5.1f}%  {100 * ref / n:5.1f}%")


hybrid()
