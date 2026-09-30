"""Which spatial scale of the scene luminance explains Camera Raw's slider delta best?
For each blur sigma, pools (B_sigma(x) + exposure, d(x)) over images, fits d = f(B) by
binned medians and reports the explained variance.

    python3 scale_probe.py <variant> [stems...]
"""
import os, re, sys
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import tonefit as tf
from tonefit import gblur as gaussian_filter

P2 = tf.P2
BASE = {"AZA": 0.35, "IMG": 0.2, "DSC": 0.07}


def exposure(stem, variant="neutral"):
    t = open(os.path.join(P2, "var", variant, stem + ".xmp")).read()
    m = re.search(r"<crs:Exposure2012>([-+0-9.]+)<", t) or re.search(r'crs:Exposure2012="([-+0-9.]+)"', t)
    return float(m.group(1)) + BASE[stem[:3]]


def fit_r2(xs, ds, lo=-14, hi=2, step=0.25):
    x = np.concatenate([a.ravel() for a in xs])
    d = np.concatenate([a.ravel() for a in ds])
    ok = np.isfinite(d) & np.isfinite(x)
    x, d = x[ok], d[ok]
    idx = np.clip(((x - lo) / step).astype(int), 0, int((hi - lo) / step))
    n = idx.max() + 1
    s = np.bincount(idx, d, n)
    c = np.bincount(idx, None, n)
    mean = s / np.maximum(c, 1)
    pred = mean[idx]
    return 1 - np.var(d - pred) / np.var(d), mean, c


if __name__ == "__main__":
    variant = sys.argv[1]
    stems = sys.argv[2:] or [l.strip().split("/")[-1].rsplit(".", 1)[0] for l in open(os.path.join(P2, "fit-list.txt")) if l.strip()]
    stems = [s for s in stems if s != "AZA06692"]
    data = []
    for st in stems:
        ev, en, es, d, _ = tf.delta(st, variant)
        data.append((st, ev + exposure(st), d))
    for sig in [0, 1, 2, 4, 8, 16, 32, 64, 128]:
        xs = [gaussian_filter(e, sig) if sig else e for _, e, _ in data]
        r2, _, _ = fit_r2(xs, [d for _, _, d in data])
        print("sigma %4d px (of %d): R2 %.3f" % (sig, max(data[0][1].shape), r2))
