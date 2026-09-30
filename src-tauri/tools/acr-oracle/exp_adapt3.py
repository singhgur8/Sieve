"""Shadows adaptivity: which domain shift explains ACR on ramps of different ranges."""
import sys, numpy as np
sys.path.insert(0, __file__.rsplit("/", 1)[0])
from oracle import *
sys.argv = ["x"]
import tone_model as tm

W, H = 2048, 192
cases = []
for lo, hi in [(-13, 0), (-4, 0), (-8, -2), (-10, -4), (-6, 0), (-12, -6), (-9, 0), (-13, -3)]:
    evs = np.linspace(lo, hi, W)
    img = np.repeat(np.repeat((2.0 ** evs)[None, :, None], H, 0), 3, 2)
    img16 = (np.clip(img, 0, 1) * 65535 + 0.5).astype(np.uint16)
    cols = np.arange(8, W - 8, 16)
    for a in [25, 50, 100]:
        out = render(img16, neutral_crs(Shadows2012=a))
        o = out[H // 2 - 24:H // 2 + 24].astype(np.float64).mean(0)[cols, 1]
        cases.append((lo, hi, a, evs[cols], o, evs))


def pred(a, x, shift):
    e = tm.slider("S", a, x + shift) - shift
    return tm.t_enc(e)


for name, fn in [
    ("abs", lambda evs: 0.0),
    ("mean.25", lambda evs: 0.25 * (-6.5 - evs.mean())),
    ("mean.5", lambda evs: 0.5 * (-6.5 - evs.mean())),
    ("max.5", lambda evs: 0.5 * (0 - evs.max())),
    ("max1", lambda evs: (0 - evs.max())),
    ("min.5", lambda evs: 0.5 * (-13 - evs.min())),
]:
    tot = []
    per = []
    for lo, hi, a, x, o, evs in cases:
        p = pred(a, x, fn(evs))
        e = np.abs(p - o).mean()
        tot.append(e)
        per.append("%d,%d:%.1f" % (lo, hi, e))
    print("%-8s mean %.2f  %s" % (name, np.mean(tot), " ".join(per[1::3])))
