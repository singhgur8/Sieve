"""Fits the local tone operator (Shadows / Highlights) to Camera Raw renders of real frames.

Model (EV domain, post-exposure e = scene log2 Y + exposure + baseline):
    m(x)  = e(x) + sum_k w_k (GF_k(e)(x) - e(x))       multi-scale guided-filter adaptation
    R_i   = lam * key_i + (1 - lam) * white_i          image reference (key = mean e,
                                                       white = 99.5th percentile of e)
    d(x)  = T_amount(m(x) - R_i)                       T: piecewise linear table (fit by LSQ)

    python3 fit_local.py <slider S|H> [--full]
"""
import itertools, os, sys
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import tonefit as tf
from scale_probe import exposure

P2 = tf.P2
KNOTS = np.arange(-12.0, 4.01, 0.5)
EXCLUDE = {"AZA06692"}
RNG = np.random.default_rng(1)


def guided(p, r, eps):
    """Self-guided filter (He et al.) with box radius r."""
    mp = tf.box(p, r)
    var = tf.box(p * p, r) - mp * mp
    a = var / (var + eps)
    b = mp - a * mp
    return tf.box(a, r) * p + tf.box(b, r)


def hat(x):
    """Piecewise-linear basis on KNOTS: (n, K) weights."""
    t = np.clip((x - KNOTS[0]) / 0.5, 0, len(KNOTS) - 1 - 1e-9)
    i = t.astype(int)
    f = t - i
    B = np.zeros((len(x), len(KNOTS)))
    B[np.arange(len(x)), i] = 1 - f
    B[np.arange(len(x)), np.minimum(i + 1, len(KNOTS) - 1)] += f
    return B


def stems():
    s = [l.strip().split("/")[-1].rsplit(".", 1)[0] for l in open(os.path.join(P2, "fit-list.txt")) if l.strip()]
    return [x for x in s if x not in EXCLUDE]


def image_data(st, variants, scales, eps, n=30000):
    ev = tf.load_ev(st)
    e = ev + exposure(st)
    yn = tf.lum(tf.ppm(os.path.join(P2, "out", "neutral", st + ".ref.ppm")))
    feats = {(r, ep): guided(e, r, ep) for r in scales for ep in eps}
    ds = {}
    for v in variants:
        _, en, es, d, _ = tf.delta(st, v)
        ds[v] = d
    h, w = e.shape
    idx = RNG.choice(h * w, n, replace=False)
    ok = (yn.ravel()[idx] > 0.003) & (yn.ravel()[idx] < 0.95)
    idx = idx[ok]
    key, white = e.mean(), np.percentile(e, 99.5)
    return dict(stem=st, e=e.ravel()[idx], f={k: v.ravel()[idx] for k, v in feats.items()},
                d={k: v.ravel()[idx] for k, v in ds.items()}, key=key, white=white)


def evaluate(data, variant, weights, lam, ridge=1e-3, return_T=False):
    X, Y = [], []
    for im in data:
        m = im["e"].copy()
        for (k, w) in weights:
            m += w * (im["f"][k] - im["e"])
        rel = m - (lam * im["key"] + (1 - lam) * im["white"])
        d = im["d"][variant]
        ok = np.isfinite(d)
        X.append(hat(rel[ok]))
        Y.append(d[ok])
    X, Y = np.vstack(X), np.concatenate(Y)
    A = X.T @ X + ridge * np.eye(X.shape[1])
    T = np.linalg.solve(A, X.T @ Y)
    res = Y - X @ T
    rmse = np.sqrt(np.mean(res ** 2))
    return (rmse, T) if return_T else rmse


if __name__ == "__main__":
    slider = sys.argv[1]
    variants = ["n%s+50" % slider, "n%s+100" % slider, "n%s-50" % slider, "n%s-100" % slider]
    scales, eps = [16, 64, 256, 1024], [1.0, 2.0, 4.0]
    data = [image_data(st, variants, scales, eps) for st in stems()]
    # Baselines
    for v in variants:
        print(v, "pixel-only, white ref: %.3f" % evaluate(data, v, [], 0.0),
              "key ref: %.3f" % evaluate(data, v, [], 1.0))
    best = []
    main = variants[1]
    for ep in eps:
        for w4, w16, w64, w256 in itertools.product([0, 0.15], [0.2, 0.35, 0.5], [0.2, 0.35, 0.5], [0, 0.15, 0.3]):
            if w4 + w16 + w64 + w256 > 1.0:
                continue
            wts = [((16, ep), w4), ((64, ep), w16), ((256, ep), w64), ((1024, ep), w256)]
            for lam in [0.5, 0.75, 1.0]:
                r = evaluate(data, main, wts, lam)
                best.append((r, ep, w4, w16, w64, w256, lam))
    best.sort()
    for b in best[:10]:
        print("rmse %.3f eps %.1f w16 %.2f w64 %.2f w256 %.2f w1024 %.2f lam %.2f" % b)
