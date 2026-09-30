"""Fits the local operator's image reference R_i = c . stats_i (+ table T per amount),
minimising the L*-weighted squared delta error (residual in EV x dL*/dEV of Camera Raw's
neutral render at that pixel), with leave-one-image-out validation.

    python3 fit_ref.py <S|H> [--stats key,p95,white] [--loo]
"""
import os, sys
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fit_local as fl
import gen_local as gl
import tonefit as tf
from scale_probe import exposure

P2 = tf.P2
STATS = ["one", "key", "p50", "p90", "p95", "p99", "white", "wclip"]


def lstar(y):
    y = np.clip(y, 0, None)
    return np.where(y > 216 / 24389, 116 * np.cbrt(y) - 16, 24389 / 27 * y)


def image_data(st, variants, n=40000):
    ev = tf.load_ev(st)
    e = ev + exposure(st)
    yn = tf.lum(tf.ppm(os.path.join(P2, "out", "neutral", st + ".ref.ppm")))
    f1 = fl.guided(e, gl.R1, gl.EPS)
    f2 = fl.guided(e, gl.R2, gl.EPS)
    m = gl.W1 * f1 + gl.W2 * f2
    ds = {}
    g = v = None
    for var in variants:
        _, en, es, d, (g, v) = tf.delta(st, var)
        ds[var] = d
    # dL*/dEV of the neutral mapping at each pixel's EV.
    lg = lstar(v)
    slope = np.gradient(lg, g)
    h, w = e.shape
    idx = RNG.choice(h * w, n, replace=False)
    ok = (yn.ravel()[idx] > 0.002) & (yn.ravel()[idx] < 0.97)
    idx = idx[ok]
    ee = e.ravel()
    pct = np.percentile(ee, [50, 90, 95, 99, 99.5])
    stats = dict(one=1.0, key=ee.mean(), p50=pct[0], p90=pct[1], p95=pct[2], p99=pct[3], white=pct[4],
                 wclip=min(pct[4], -1.0))
    wt = np.interp(ee[idx], g, np.maximum(slope, 0.5))
    return dict(stem=st, m=m.ravel()[idx], e=ee[idx], d={k: x.ravel()[idx] for k, x in ds.items()}, w=wt, stats=stats)


RNG = np.random.default_rng(2)


def solve(data, variant, coef, names, zero):
    X, Y, W = [], [], []
    for im in data:
        R = sum(c * im["stats"][n] for c, n in zip(coef, names))
        rel = im["m"] - R
        d = im["d"][variant]
        ok = np.isfinite(d)
        X.append(fl.hat(rel[ok]))
        Y.append(d[ok])
        W.append(im["w"][ok])
    X, Y, W = np.vstack(X), np.concatenate(Y), np.concatenate(W)
    K = X.shape[1]
    D = np.diff(np.eye(K), 2, axis=0)
    P = np.zeros(K)
    if zero == "bright":
        P[fl.KNOTS >= 2.0] = 1e6
    else:
        P[fl.KNOTS <= -9.0] = 1e6
    Xw = X * W[:, None]
    A = Xw.T @ Xw + 3e-2 * (W ** 2).sum() / K * D.T @ D + np.diag(P) + 1e-6 * np.eye(K)
    T = np.linalg.solve(A, Xw.T @ (Y * W))
    return T


def loss(data, variant, coef, names, zero, T=None):
    if T is None:
        T = solve(data, variant, coef, names, zero)
    per = []
    for im in data:
        R = sum(c * im["stats"][n] for c, n in zip(coef, names))
        d = im["d"][variant]
        ok = np.isfinite(d)
        pred = fl.hat(im["m"][ok] - R) @ T
        per.append(np.sqrt(np.mean(((d[ok] - pred) * im["w"][ok]) ** 2)))
    return float(np.mean(per)), per, T


def optimize(data, variants, names, zero, coef0):
    coef = np.array(coef0, float)
    f = lambda c: sum(loss(data, v, c, names, zero)[0] for v in variants)
    best = f(coef)
    step = 0.25
    while step > 0.01:
        improved = False
        for i in range(len(coef)):
            for s in (step, -step):
                c = coef.copy()
                c[i] += s
                v = f(c)
                if v < best - 1e-4:
                    best, coef, improved = v, c, True
        if not improved:
            step /= 2
    return coef, best


if __name__ == "__main__":
    slider = sys.argv[1]
    zero = "bright" if slider == "S" else "dark"
    names = ["one", "key", "white"]
    if "--stats" in sys.argv:
        names = ["one"] + sys.argv[sys.argv.index("--stats") + 1].split(",")
    variants = ["n%s+100" % slider, "n%s-100" % slider, "n%s+50" % slider, "n%s-50" % slider]
    data = [image_data(st, variants) for st in fl.stems()]
    base = [0.0] + [0.75 if n == "key" else 0.25 if n == "white" else 0.0 for n in names[1:]]
    print("baseline (0.75 key + 0.25 white):", ["%.2f" % loss(data, v, [0, 0.75, 0.25], ["one", "key", "white"], zero)[0] for v in variants])
    coef, best = optimize(data, variants[:2], names, zero, base)
    print("fitted", dict(zip(names, np.round(coef, 3))), "loss %.3f" % best)
    for v in variants:
        l, per, _ = loss(data, v, coef, names, zero)
        print(v, "%.2f" % l, " ".join("%s:%.1f" % (im["stem"][-4:], p) for im, p in zip(data, per)))
    if "--loo" in sys.argv:
        out = []
        for k in range(len(data)):
            train = data[:k] + data[k + 1:]
            c, _ = optimize(train, variants[:2], names, zero, coef)
            T = solve(train, variants[0], c, names, zero)
            l, per, _ = loss([data[k]], variants[0], c, names, zero, T)
            out.append((data[k]["stem"], per[0]))
        print("LOO", variants[0], " ".join("%s:%.1f" % (s[-4:], p) for s, p in out), "mean %.2f" % np.mean([p for _, p in out]))
