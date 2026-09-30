"""Fits the local tone operator on features computed by Sieve itself (parity_eval
SIEVE_DUMP_EV on the `neutral` variant: per-pixel scene EV, the uncropped context's fine /
coarse adaptation bases, source statistics and total exposure), so the tables match the
engine exactly. Loss: L*-weighted RMS of the EV delta (see fit_ref.py).

    python3 fit2.py [--write] [--loo]

Writes src/develop/local_tone_data.rs with --write.
"""
import os, sys
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fit_local as fl
import tonefit as tf
from fit_ref import lstar

P2 = tf.P2
RNG = np.random.default_rng(3)
STATS = ["key", "p50", "p90", "p95", "p99", "white"]
SLIDERS = {"S": ("SHADOWS", "bright"), "H": ("HIGHLIGHTS", "dark")}
AMOUNTS = (-100, -50, 50, 100)


def load(st, variants, n=40000):
    d0 = os.path.join(P2, "out", "neutral")
    s = tf.ppm(os.path.join(d0, st + ".sieve.ppm"))
    shape = s.shape[:2]
    rd = lambda k: np.fromfile(os.path.join(d0, "%s.%s.f32" % (st, k)), dtype="<f4").reshape(shape).astype(np.float64)
    ev, m1, m2 = rd("ev"), rd("m1"), rd("m2")
    vals = [float(v) for v in open(os.path.join(d0, st + ".stats.txt")).read().split()]
    stats = dict(zip(STATS, vals[:6]))
    x = vals[6]
    yn = tf.lum(tf.ppm(os.path.join(d0, st + ".ref.ppm")))
    ds, g, v = {}, None, None
    for var in variants:
        _, en, es, d, (g, v) = tf.delta(st, var)
        ds[var] = d
    slope = np.gradient(lstar(v), g)
    h, w = shape
    idx = RNG.choice(h * w, min(n, h * w), replace=False)
    ok = (yn.ravel()[idx] > 0.002) & (yn.ravel()[idx] < 0.97)
    idx = idx[ok]
    post = {k: stats[k] + x for k in STATS}
    wt = np.interp(ev.ravel()[idx], g, np.maximum(slope, 0.5))
    return dict(stem=st, x=x, m1=m1.ravel()[idx] + x, m2=m2.ravel()[idx] + x, stats=post,
                d={k: a.ravel()[idx] for k, a in ds.items()}, w=wt)


def ref(im, c, wclip):
    s = im["stats"]
    return c.get("one", 0) + sum(c.get(k, 0) * s[k] for k in STATS) + c.get("wclip", 0) * min(s["white"], wclip)


def design(data, variant, c, w1, wclip):
    X, Y, W = [], [], []
    for im in data:
        m = w1 * im["m1"] + (1 - w1) * im["m2"]
        d = im["d"][variant]
        ok = np.isfinite(d)
        X.append(fl.hat(m[ok] - ref(im, c, wclip)))
        Y.append(d[ok])
        W.append(im["w"][ok])
    return X, Y, W


def solve(X, Y, W, zero):
    X, Y, W = np.vstack(X), np.concatenate(Y), np.concatenate(W)
    K = X.shape[1]
    D = np.diff(np.eye(K), 2, axis=0)
    Xw = X * W[:, None]
    G = Xw.T @ Xw
    # Anchor the inactive side at exactly 0 (strong relative to the data term).
    P = np.zeros(K)
    if zero == "bright":
        P[fl.KNOTS >= 3.0] = 1e4 * np.mean(np.diag(G))
    else:
        P[fl.KNOTS <= -9.0] = 1e4 * np.mean(np.diag(G))
    A = G + (3e-2 if zero == "bright" else 3e-1) * (W ** 2).sum() / K * D.T @ D + np.diag(P) + 1e-6 * np.eye(K)
    return np.linalg.solve(A, Xw.T @ (Y * W))


def loss(data, variant, c, w1, wclip, zero, T=None):
    X, Y, W = design(data, variant, c, w1, wclip)
    if T is None:
        T = solve(X, Y, W, zero)
    per = [np.sqrt(np.mean(((y - x @ T) * w) ** 2)) if len(y) else 0.0 for x, y, w in zip(X, Y, W)]
    return float(np.mean(per)), per, T


def optimize(data, variants, keys, zero, c0, w1, wclip):
    c = dict(c0)
    f = lambda cc: sum(loss(data, v, cc, w1, wclip, zero)[0] for v in variants)
    best = f(c)
    step = 0.25
    while step > 0.01:
        improved = False
        for k in keys:
            for s in (step, -step):
                cc = dict(c)
                cc[k] = cc.get(k, 0) + s
                v = f(cc)
                if v < best - 1e-4:
                    best, c, improved = v, cc, True
        if not improved:
            step /= 2
    return c, best


def main():
    stems = fl.stems()
    variants = ["n%s%+d" % (s, a) for s in SLIDERS for a in AMOUNTS]
    data = [load(st, variants) for st in stems]
    cfg = {}
    # Shadows: reference + fine weight + clamp by search.
    best = None
    for w1 in (0.3, 0.5):
        for wclip in (-2.0, -1.5, -1.0):
            c, l = optimize(data, ["nS+100", "nS-100"], ["one", "key", "wclip"], "bright",
                            {"one": 1.0, "key": 1.0, "wclip": 1.5}, w1, wclip)
            print("S w1 %.1f wclip %.1f -> %s loss %.3f" % (w1, wclip, {k: round(v, 3) for k, v in c.items()}, l))
            if best is None or l < best[0]:
                best = (l, w1, wclip, c)
    _, w1, wclip_s, cs = best
    # Highlights: its own reference (same adaptation field).
    best = None
    for wclip in (-2.0, -1.0, 0.0):
        c, l = optimize(data, ["nH+100", "nH-100"], ["one", "key", "white", "wclip"], "dark",
                        {"one": 0.0, "key": 0.5, "white": 0.5}, w1, wclip)
        print("H wclip %.1f -> %s loss %.3f" % (wclip, {k: round(v, 3) for k, v in c.items()}, l))
        if best is None or l < best[0]:
            best = (l, wclip, c)
    _, wclip_h, ch = best
    cfg = {"S": (cs, wclip_s), "H": (ch, wclip_h)}
    tables = {}
    for s, (name, zero) in SLIDERS.items():
        c, wc = cfg[s]
        rows = []
        for a in AMOUNTS:
            v = "n%s%+d" % (s, a)
            lv, per, T = loss(data, v, c, w1, wc, zero)
            print(v, "%.2f" % lv, " ".join("%s:%.1f" % (im["stem"][-4:], p) for im, p in zip(data, per)))
            rows.append(T)
        tables[name] = rows
    if "--loo" in sys.argv:
        out = []
        for k in range(len(data)):
            train = data[:k] + data[k + 1:]
            cc, _ = optimize(train, ["nS+100", "nS-100"], ["one", "key", "wclip"], "bright", cs, w1, wclip_s)
            X, Y, W = design(train, "nS+100", cc, w1, wclip_s)
            T = solve(X, Y, W, "bright")
            lv, per, _ = loss([data[k]], "nS+100", cc, w1, wclip_s, "bright", T)
            out.append((data[k]["stem"], per[0]))
        print("LOO nS+100", " ".join("%s:%.1f" % (s[-4:], p) for s, p in out), "mean %.2f" % np.mean([p for _, p in out]))
    if "--write" in sys.argv:
        write(tables, cfg, w1)


def write(tables, cfg, w1):
    import gen_local as gl
    lines = [
        "//! Generated by `tools/acr-oracle/fit2.py` from Camera Raw 17.5 renders of real frames",
        "//! (single-slider variants of the fit set, features computed by Sieve). Do not edit.",
        "#![allow(clippy::approx_constant, clippy::excessive_precision, clippy::unreadable_literal)]",
        "",
        "/// Guided-filter box radii (px per 2048 px of the source long edge), eps (EV^2), and the",
        "/// fine base's weight in the adaptation luminance (coarse = 1 - fine).",
        "pub const RADIUS_FINE: f32 = %.1f;" % gl.R1,
        "pub const RADIUS_COARSE: f32 = %.1f;" % gl.R2,
        "pub const EPS: f32 = %.3f;" % gl.EPS,
        "pub const WEIGHT_FINE: f32 = %.3f;" % w1,
        "/// Image references (post-exposure EV) of the Shadows (S_) and Highlights (H_) tables:",
        "/// ONE + sum <STAT> * stat + WCLIP_W * min(white, WCLIP).",
    ]
    for pre, sl in (("S", "S"), ("H", "H")):
        c, wc = cfg[sl]
        for k in ["one", "key", "p50", "p90", "p95", "p99", "white"]:
            lines.append("pub const %s_%s: f32 = %.4f;" % (pre, k.upper(), c.get(k, 0.0)))
        lines.append("pub const %s_WCLIP_W: f32 = %.4f;" % (pre, c.get("wclip", 0.0)))
        lines.append("pub const %s_WCLIP: f32 = %.3f;" % (pre, wc))
    lines += [
        "/// Table domain: adaptation EV relative to the reference.",
        "pub const REL0: f32 = %.1f;" % fl.KNOTS[0],
        "pub const REL_STEP: f32 = 0.5;",
        "pub const REL_N: usize = %d;" % len(fl.KNOTS),
        "/// Slider amounts of the table rows.",
        "pub const AMOUNTS: [f32; 5] = [-100.0, -50.0, 0.0, 50.0, 100.0];",
    ]
    for name, rows in tables.items():
        rows = rows[:2] + [np.zeros(len(fl.KNOTS))] + rows[2:]
        lines.append("/// EV delta at the adaptation luminance, per amount row.")
        lines.append("pub const %s: [[f32; REL_N]; 5] = [" % name)
        for r in rows:
            lines.append("    [" + ", ".join("%.4f" % v for v in r) + "],")
        lines.append("];")
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "src", "develop", "local_tone_data.rs")
    open(path, "w").write("\n".join(lines) + "\n")
    print("wrote", os.path.normpath(path))


if __name__ == "__main__":
    main()
