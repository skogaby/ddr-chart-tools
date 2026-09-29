#!/usr/bin/env python3
"""Throwaway: follow-up on analyze4 — window width, gating, and tail anatomy for the `rise` feature."""
import sys
import numpy as np
sys.argv = [sys.argv[0], sys.argv[1]]
exec(open("analyze4.py").read().split('print(f"songs {len(codes)}')[0])  # reuse loaders only

bpm_min_arr = bpm_min
print("# window width, rise feature, no gating")
for half in (55, 60, 65, 70, 80, 100):
    for tpl in ("notes", "metric", "beats"):
        m, rival, edge, split = measure("rise", tpl, half)
        T = np.median(m + c)
        report(f"±{half} rise/{tpl} T={T:+.2f}", m + c - T)

print("\n# high-BPM songs only (bpm_max >= 220), ±60")
hi = bpm_max >= 220
for tpl in ("notes", "metric", "beats"):
    m, rival, edge, split = measure("rise", tpl, 60)
    T = np.median(m + c)
    report(f"rise/{tpl} n={hi.sum()}", m + c - T, hi)

print("\n# gating, rise/notes ±60")
m, rival, edge, split = measure("rise", "notes", 60)
T = np.median(m + c)
e = m + c - T
for rt in (0.7, 0.8, 0.9, 1.01):
    for st in (2, 3, 5, 1e9):
        acc = (rival < rt) & (split <= st) & ~edge
        report(f"rival<{rt} split<={st:g} !edge", e, acc)

print("\n# worst 25 songs, rise/notes ±60")
order_ = np.argsort(-np.abs(e))
for j in order_[:25]:
    print(f"   {codes[j]:8s} c={c[j]:+4.0f} ours={-(m[j] - T):+6.1f} (community {-c[j]:+4.0f}) diff={e[j]:+6.2f} bpm={bpm_min_arr[j]:.0f}-{bpm_max[j]:.0f} rival={rival[j]:.2f} split={split[j]:.1f} edge={int(edge[j])}")

print("\n# synthetic shifts, rise/notes, windows 60/70, gated rival<0.9 split<=5 !edge")
truth = T - c
for half in (60, 70):
    for sh in (-55, -50, -40, 0, 40, 50, 55):
        ms, rv, ed, sp = measure("rise", "notes", half, center=sh)
        err = ms - truth
        acc = (rv < 0.9) & (sp <= 5) & ~ed
        print(f"   ±{half} shift {sh:+4d}: accepted {100 * acc.mean():5.1f}%   of accepted: within 1 ms {100 * np.mean(abs(err[acc]) <= 1):5.1f}%  within 2 ms {100 * np.mean(abs(err[acc]) <= 2):5.1f}%  >5 ms {100 * np.mean(abs(err[acc]) > 5):4.1f}%")
