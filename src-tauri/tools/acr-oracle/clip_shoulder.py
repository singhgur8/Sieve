"""Clip-anchored highlight shoulder (round 2). Equivalent scene-EV shift delta(ev) = ev - ev*, where Sieve's L*(ev*) = Camera Raw's L*(ev)
(binned medians of the neutral variant, luminance only), per image, vs highlight statistics."""
import os, sys
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import diff as df
import tonefit as tf

v = os.environ.get("V", "neutral")
d = os.path.join(tf.P2, "out", "") + v
POST = [-6, -4, -3, -2, -1.5, -1, -0.5, 0, 0.5, 1.0]
print("%-9s %5s %5s %5s %5s |" % ("stem", "x", "clip%", "p999", "max") + "".join("%6.1f" % p for p in POST))
for f in sorted(os.listdir(d)):
    if not f.endswith(".ref.ppm"):
        continue
    st = f[:-8]
    sp = os.path.join(tf.P2, "out", "neutral", st + ".stats.txt")
    if not os.path.exists(sp):
        continue
    x = float(open(sp).read().split()[6])
    s, r = df.load(d, st)
    Ls, Lr = df.srgb_to_lab(df.box3(s))[..., 0], df.srgb_to_lab(df.box3(r))[..., 0]
    ev = tf.box(tf.load_ev(st), 1) + x
    grid = np.arange(-10, 3.01, 0.125)
    def curve(L):
        out = np.full(len(grid), np.nan)
        for i, g in enumerate(grid):
            m = (ev >= g - 0.0625) & (ev < g + 0.0625)
            if m.sum() > 150:
                out[i] = np.median(L[m])
        return out
    cs, cr = curve(Ls), curve(Lr)
    ok = ~np.isnan(cs)
    gs, vs = grid[ok], np.maximum.accumulate(cs[ok])
    row = []
    for p in POST:
        i = np.argmin(np.abs(grid - p))
        if np.isnan(cr[i]) or cr[i] >= vs[-1] - 0.05 or cr[i] <= vs[0]:
            row.append("   .  ")
            continue
        es = np.interp(cr[i], vs + np.arange(len(vs)) * 1e-6, gs)
        row.append("%+6.2f" % (p - es))
    e0 = ev - x
    print("%-9s %+5.2f %5.1f %+5.2f %+5.2f |" % (st, x, 100 * np.mean(e0 > -0.05), np.percentile(e0, 99.9), e0.max()) + "".join(row))
