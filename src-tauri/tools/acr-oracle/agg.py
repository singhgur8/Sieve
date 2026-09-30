"""Aggregate dL / dE by reference L* over all images of variant output dirs.

    python3 agg.py <variant>...   (dirs under p2/out)
"""
import os, sys
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import diff as df
import tonefit as tf

P2 = os.path.join(tf.P2, "out", "")
stems = os.environ.get("STEMS")
for v in sys.argv[1:]:
    d = P2 + v
    ss = sorted(f[:-8] for f in os.listdir(d) if f.endswith(".ref.ppm"))
    if stems:
        ss = [s for s in ss if any(s.startswith(t) for t in stems.split(","))]
    acc = np.zeros((10, 4))
    for st in ss:
        s, r = df.load(d, st)
        ls, lr = df.srgb_to_lab(df.box3(s)), df.srgb_to_lab(df.box3(r))
        de = df.de2000(lr, ls)
        dl = ls[..., 0] - lr[..., 0]
        dc = np.hypot(ls[..., 1], ls[..., 2]) - np.hypot(lr[..., 1], lr[..., 2])
        b = np.clip((lr[..., 0] / 10).astype(int), 0, 9)
        for i in range(10):
            m = b == i
            if m.any():
                acc[i] += [m.sum(), dl[m].sum(), de[m].sum(), dc[m].sum()]
    n = acc[:, 0].sum()
    print("%-8s" % v[:8] + " ".join("L%d0:%3.0f%% %+5.1f/%4.1f/%+4.1f" % (i, 100 * a[0] / n, a[1] / a[0], a[2] / a[0], a[3] / a[0])
                                    for i, a in enumerate(acc) if a[0] > n * 0.005))
