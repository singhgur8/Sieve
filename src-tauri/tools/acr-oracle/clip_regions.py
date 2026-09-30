"""Sieve vs Camera Raw colour of raw-clipped regions, by number of clipped channels
(`parity_eval` with SIEVE_DUMP=1 SIEVE_DUMP_CAM=1 into $P2/out/clip).

    python3 clip_regions.py STEM...
"""
import os, sys
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import tonefit as tf
D = os.path.join(tf.P2, "out", "clip", "")

def ppm(p):
    b = open(p, "rb").read()
    parts = b.split(b"\n", 3)
    w, h = map(int, parts[1].split())
    return np.frombuffer(parts[3], np.uint8).reshape(h, w, 3).astype(np.float64) / 255

def lab(e):
    l = np.where(e <= 0.04045, e / 12.92, ((e + 0.055) / 1.055) ** 2.4)
    M = np.array([[0.4124, 0.3576, 0.1805], [0.2126, 0.7152, 0.0722], [0.0193, 0.1192, 0.9505]])
    xyz = l @ M.T / np.array([0.9505, 1.0, 1.089])
    f = np.where(xyz > 0.008856, np.cbrt(xyz), 7.787 * xyz + 16 / 116)
    return np.stack([116 * f[..., 1] - 16, 500 * (f[..., 0] - f[..., 1]), 200 * (f[..., 1] - f[..., 2])], -1)

for stem in sys.argv[1:]:
    t = open(D + stem + ".cam.txt").read().split("\n"); w, h = map(int, t[0].split()); mul = np.array(eval(t[1]))
    a = np.fromfile(D + stem + ".cam.f32", dtype='<f4').reshape(h, w, 3)
    raw = a / mul
    n = (raw >= 0.985).sum(2)
    near = (raw.max(2) > 0.9) & (n == 0)
    r, s = lab(ppm(D + stem + ".ref.ppm")), lab(ppm(D + stem + ".sieve.ppm"))
    print(stem, "clipped channels frac:", [round(float((n == k).mean()), 4) for k in range(4)])
    for name, m in [("near", near)] + [("n=%d" % k, n == k) for k in (1, 2, 3)]:
        if m.sum() < 50:
            continue
        # which channels clipped (for n=1,2)
        which = (raw[m] >= 0.985).mean(0).round(2)
        print("  %-5s %6d px  ref Lab %s  sieve Lab %s  chans %s" % (name, m.sum(), r[m].mean(0).round(1), s[m].mean(0).round(1), which))
