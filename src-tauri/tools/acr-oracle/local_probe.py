"""Local dependence of a slider's delta: residual of d after each image's own curve
f_i(pixel ev) vs the surround contrast c = B_sigma - ev, for several sigmas.

    python3 local_probe.py <variant>
Prints the slope of residual on c (EV of delta per EV of surround), within ev bins where
the response is strong, and the variance explained by adding it.
"""
import os, sys
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import tonefit as tf
from scale_probe import exposure, fit_r2

P2 = tf.P2

variant = sys.argv[1]
stems = [l.strip().split("/")[-1].rsplit(".", 1)[0] for l in open(os.path.join(P2, "fit-list.txt")) if l.strip()]
stems = [s for s in stems if s not in ("AZA06692", "DSCF5902", "DSCF5893", "DSCF5909")]
res = {}
for st in stems:
    ev, en, es, d, _ = tf.delta(st, variant)
    x = ev + exposure(st)
    ok = np.isfinite(d)
    r2, mean, cnt = fit_r2([x], [d])
    idx = np.clip(((x + 14) / 0.25).astype(int), 0, len(mean) - 1)
    r = np.where(ok, d - mean[idx], np.nan)
    w = np.abs(mean[idx]) > 0.3  # where the slider acts
    for sig in [4, 16, 64, 256]:
        c = tf.gblur(ev, sig) - ev
        m = ok & w
        if m.sum() < 1000:
            continue
        A = np.vstack([c[m], np.ones(m.sum())]).T
        coef, *_ = np.linalg.lstsq(A, r[m], rcond=None)
        pred = A @ coef
        ex = 1 - np.var(r[m] - pred) / np.var(r[m])
        res.setdefault(sig, []).append((st, coef[0], ex, np.std(r[m])))
for sig, rows in res.items():
    print("sigma %3d: " % sig + " ".join("%s:%+.2f(%.0f%%)" % (s[-4:], k, 100 * e) for s, k, e, _ in rows))
print("resid std:", " ".join("%s:%.2f" % (s[-4:], sd) for s, _, _, sd in res[4]))
