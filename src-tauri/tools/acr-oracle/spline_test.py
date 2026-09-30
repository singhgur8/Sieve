import json, numpy as np
HERE = __file__.rsplit("/", 1)[0]
C = json.load(open(HERE + "/curves.json"))
base = np.array(json.load(open(HERE + "/ramp.json"))["res"]["default"])

def natural(xs, ys):
    xs = np.array(xs, float); ys = np.array(ys, float)
    n = len(xs)
    if n == 2:
        return lambda x: np.interp(x, xs, ys)
    # DNG SDK dng_spline_solver: C2 natural spline.
    h = np.diff(xs)
    A = np.zeros((n, n)); r = np.zeros(n)
    A[0, 0] = 1; A[-1, -1] = 1
    for i in range(1, n - 1):
        A[i, i - 1] = h[i - 1]; A[i, i] = 2 * (h[i - 1] + h[i]); A[i, i + 1] = h[i]
        r[i] = 6 * ((ys[i + 1] - ys[i]) / h[i] - (ys[i] - ys[i - 1]) / h[i - 1])
    M = np.linalg.solve(A, r)
    def f(x):
        x = np.clip(x, xs[0], xs[-1])
        i = np.clip(np.searchsorted(xs, x) - 1, 0, n - 2)
        t = x - xs[i]; hh = h[i]
        a = (M[i + 1] - M[i]) / (6 * hh)
        b = M[i] / 2
        c = (ys[i + 1] - ys[i]) / hh - hh * (2 * M[i] + M[i + 1]) / 6
        return ys[i] + c * t + b * t * t + a * t ** 3
    return f

def monotone(xs, ys):
    xs = np.array(xs, float); ys = np.array(ys, float)
    n = len(xs)
    d = np.diff(ys) / np.diff(xs)
    m = np.zeros(n)
    m[0] = d[0]; m[-1] = d[-1]
    for i in range(1, n - 1):
        m[i] = 0 if d[i - 1] * d[i] <= 0 else (d[i - 1] + d[i]) / 2
    for i in range(n - 1):
        if d[i] == 0:
            m[i] = m[i + 1] = 0
        else:
            a, b = m[i] / d[i], m[i + 1] / d[i]
            s = a * a + b * b
            if s > 9:
                t = 3 / np.sqrt(s); m[i] = t * a * d[i]; m[i + 1] = t * b * d[i]
    def f(x):
        x = np.clip(x, xs[0], xs[-1])
        i = np.clip(np.searchsorted(xs, x) - 1, 0, n - 2)
        h = xs[i + 1] - xs[i]; t = (x - xs[i]) / h
        h00 = 2 * t ** 3 - 3 * t ** 2 + 1; h10 = t ** 3 - 2 * t ** 2 + t; h01 = -2 * t ** 3 + 3 * t ** 2; h11 = t ** 3 - t ** 2
        return h00 * ys[i] + h10 * h * m[i] + h01 * ys[i + 1] + h11 * h * m[i + 1]
    return f

for k in ["P128_160", "P64_96", "P0_40", "P255_200", "Pscurve", "Puser", "Pcolor"]:
    pts = [tuple(float(v) for v in p.split(",")) for p in C["sets"][k][1]["ToneCurvePV2012"]]
    o = np.array(C["res"][k])[:, 1]
    m = base > 3
    for name, fn in [("natural", natural), ("monotone", monotone)]:
        f = fn([p[0] for p in pts], [p[1] for p in pts])
        e = f(base) - o
        print("%-10s %-9s mean %.2f max %.2f" % (k, name, np.abs(e[m]).mean(), np.abs(e[m]).max()))
