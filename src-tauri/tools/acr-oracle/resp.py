"""Slider response: output L*/C* vs the neutral variant's L*, Camera Raw vs Sieve (all fit frames).

    python3 resp.py <variant>...
"""
import os, sys
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import diff as df
import tonefit as tf
P = os.path.join(tf.P2, "out", "")
bins = np.arange(0, 101, 10)
for v in sys.argv[1:]:
    acc = np.zeros((len(bins) - 1, 7))
    for f in sorted(os.listdir(P + v)):
        if not f.endswith(".ref.ppm") or not os.path.exists(P + "neutral/" + f):
            continue
        st = f[:-8]
        s, r = df.load(P + v, st)
        sn, rn = df.load(P + "neutral", st)
        L = [df.srgb_to_lab(df.box3(x)) for x in (rn, r, sn, s)]
        base = L[0][..., 0]
        for i in range(len(bins) - 1):
            m = (base >= bins[i]) & (base < bins[i + 1])
            if m.sum() < 100:
                continue
            c = lambda lab: np.hypot(lab[..., 1], lab[..., 2])[m].mean()
            acc[i] += [m.sum(), (L[1][..., 0] - L[0][..., 0])[m].sum() * 1, (L[3][..., 0] - L[2][..., 0])[m].sum(),
                       (c(L[1]) - c(L[0])) * m.sum(), (c(L[3]) - c(L[2])) * m.sum(), 0, 0]
    print("== %s   neutral-L bin: ACR dL / Sieve dL | ACR dC / Sieve dC" % v)
    for i in range(len(bins) - 1):
        a = acc[i]
        if a[0] > 0:
            print("   L%02d-%02d %5.1f%%  dL %+6.1f / %+6.1f | dC %+5.1f / %+5.1f" % (bins[i], bins[i + 1], 100 * a[0] / acc[:, 0].sum(), a[1] / a[0], a[2] / a[0], a[3] / a[0], a[4] / a[0]))
