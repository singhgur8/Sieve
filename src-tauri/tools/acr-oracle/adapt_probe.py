"""Per-image slider response curves d(ev_post) and candidate adaptation statistics.

    python3 adapt_probe.py <variant> [sigma]
Prints, per image, the within-image R2 of d = f_i(B) and the post-exposure EV at which
the response falls to half of its maximum, next to image statistics.
"""
import os, sys
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import tonefit as tf
from scale_probe import exposure, fit_r2

P2 = tf.P2

if __name__ == "__main__":
    variant = sys.argv[1]
    sig = float(sys.argv[2]) if len(sys.argv) > 2 else 2
    stems = [l.strip().split("/")[-1].rsplit(".", 1)[0] for l in open(os.path.join(P2, "fit-list.txt")) if l.strip()]
    stems = [s for s in stems if s != "AZA06692"]
    print("%-9s %5s %6s %6s %6s %6s %6s | curve (ev_post: d)" % ("image", "R2", "key", "p50", "p95", "p99.5", "half"))
    for st in stems:
        ev, en, es, d, _ = tf.delta(st, variant)
        x = tf.gblur(ev, sig) + exposure(st)
        r2, mean, cnt = fit_r2([x], [d])
        grid = -14 + 0.25 * np.arange(len(mean)) + 0.125
        ok = cnt > 200
        g, m = grid[ok], mean[ok]
        # smooth
        e = ev + exposure(st)
        k = np.percentile(e, [50, 95, 99.5])
        peak = np.max(np.abs(m)) if len(m) else 0
        sgn = np.sign(m[np.argmax(np.abs(m))]) if len(m) else 1
        half = np.nan
        for gi, mi in zip(g, m):
            if abs(mi) >= 0.5 * peak and sgn * mi > 0:
                half = gi
        curve = " ".join("%.0f:%+.2f" % (gi, mi) for gi, mi in zip(g[::4], m[::4]))
        print("%-9s %5.2f %6.2f %6.2f %6.2f %6.2f %6.2f | %s" % (st, r2, e.mean(), k[0], k[1], k[2], half, curve))
