"""Local tone operator analysis on real frames.

For each image: ev(x) = Sieve's scene log2 luminance (parity_eval SIEVE_DUMP_EV, variant
`neutral`), Yn / Ys = Camera Raw display luminance of the neutral and the slider variant.
G = the image's neutral tone mapping ev -> Yn (binned medians, monotone); the slider's
effect as a scene-EV delta is d(x) = Ginv(Ys(x)) - Ginv(Yn(x)).
"""
import os, sys
import numpy as np

P2 = os.environ.get("P2", "/Users/gurjotsingh/Documents/GitHub/Sieve/test-data/p2")


def ppm(path):
    with open(path, "rb") as f:
        data = f.read()
    parts = data.split(b"\n", 3)
    w, h = map(int, parts[1].split())
    return np.frombuffer(parts[3], dtype=np.uint8).reshape(h, w, 3)


def lin(img):
    e = img.astype(np.float64) / 255.0
    return np.where(e <= 0.04045, e / 12.92, ((e + 0.055) / 1.055) ** 2.4)


def lum(img):
    l = lin(img)
    return 0.2126 * l[..., 0] + 0.7152 * l[..., 1] + 0.0722 * l[..., 2]


def box(a, r):
    if r <= 0:
        return a
    k = 2 * r + 1
    p = np.pad(a, r, mode="edge")
    c = np.cumsum(np.cumsum(p, 0), 1)
    c = np.pad(c, ((1, 0), (1, 0)))
    return (c[k:, k:] - c[:-k, k:] - c[k:, :-k] + c[:-k, :-k]) / (k * k)


def gblur(a, sigma):
    """Gaussian-like blur: three box passes with the same variance."""
    if sigma <= 0:
        return a
    r = max(1, int(round((np.sqrt(4 * sigma * sigma + 1) - 1) / 2)))
    for _ in range(3):
        a = box(a, r)
    return a


def load_ev(stem):
    d = os.path.join(P2, "out", "neutral")
    s = ppm(os.path.join(d, stem + ".sieve.ppm"))
    ev = np.fromfile(os.path.join(d, stem + ".ev.f32"), dtype="<f4").reshape(s.shape[:2]).astype(np.float64)
    return ev


def tone_map(ev, y, lo=-14.0, hi=1.0, step=0.125):
    """Monotone ev -> y from binned medians; returns (grid, values)."""
    grid = np.arange(lo, hi + 1e-9, step)
    idx = np.clip(((ev - lo) / step).round().astype(int), 0, len(grid) - 1)
    vals = np.full(len(grid), np.nan)
    cnt = np.bincount(idx.ravel(), minlength=len(grid))
    order = np.argsort(idx.ravel())
    ys = y.ravel()[order]
    starts = np.concatenate([[0], np.cumsum(cnt)])
    for i in range(len(grid)):
        if cnt[i] >= 30:
            vals[i] = np.median(ys[starts[i]:starts[i + 1]])
    ok = ~np.isnan(vals)
    g, v = grid[ok], vals[ok]
    v = np.maximum.accumulate(v)
    return g, v


def ginv(g, v, y):
    # strictly increasing for interp
    v2 = v + np.arange(len(v)) * 1e-9
    return np.interp(y, v2, g, left=np.nan, right=np.nan)


def delta(stem, variant, smooth=1):
    ev = load_ev(stem)
    yn = lum(ppm(os.path.join(P2, "out", "neutral", stem + ".ref.ppm")))
    ys = lum(ppm(os.path.join(P2, "out", variant, stem + ".ref.ppm")))
    if smooth:
        ev, yn, ys = box(ev, smooth), box(yn, smooth), box(ys, smooth)
    g, v = tone_map(ev, yn)
    # usable range: slope of G not tiny
    en, es = ginv(g, v, yn), ginv(g, v, ys)
    return ev, en, es, es - en, (g, v)


if __name__ == "__main__":
    variant = sys.argv[1]
    stems = sys.argv[2:]
    for st in stems:
        ev, en, es, d, (g, v) = delta(st, variant)
        ok = np.isfinite(d)
        print("== %s %s  key %.2f  p99.5 %.2f" % (st, variant, ev.mean(), np.percentile(ev, 99.5)))
        edges = np.arange(-14, 1.01, 1.0)
        row = []
        for a, b in zip(edges[:-1], edges[1:]):
            m = ok & (ev >= a) & (ev < b)
            if m.mean() > 0.003:
                row.append("%+.0f:%4.0f%% d%+.2f" % (a, 100 * m.mean(), np.median(d[m])))
        print("   " + " | ".join(row))
