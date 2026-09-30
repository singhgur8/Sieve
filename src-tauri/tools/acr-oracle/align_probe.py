"""Finds where Camera Raw's frame (ref.ppm, resized to Sieve's render size) sits inside
Sieve's frame: Sieve covers `SW x SH` source px, the reference a `RW x RH` sub-rectangle at
offset (ox, oy). Prints the offsets with the lowest luminance error.

    python3 align_probe.py <out-dir> <stem> <SW> <SH> <RW> <RH> [orient-swap]
"""
import os, sys
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import tonefit as tf

d, stem = sys.argv[1], sys.argv[2]
SW, SH, RW, RH = map(float, sys.argv[3:7])
swap = len(sys.argv) > 7
s = tf.lum(tf.ppm(os.path.join(d, stem + ".sieve.ppm")))
r = tf.lum(tf.ppm(os.path.join(d, stem + ".ref.ppm")))
if swap:  # portrait render of a landscape sensor frame
    SW, SH, RW, RH = SH, SW, RH, RW
h, w = s.shape


def sample(img, x, y):
    x = np.clip(x, 0, img.shape[1] - 1.001)
    y = np.clip(y, 0, img.shape[0] - 1.001)
    x0, y0 = x.astype(int), y.astype(int)
    fx, fy = x - x0, y - y0
    a = img[y0, x0] * (1 - fx) + img[y0, x0 + 1] * fx
    b = img[y0 + 1, x0] * (1 - fx) + img[y0 + 1, x0 + 1] * fx
    return a * (1 - fy) + b * fy


yy, xx = np.mgrid[8:h - 8:2, 8:w - 8:2].astype(float)
res = []
for ox in np.arange(0, SW - RW + 1, 2):
    for oy in np.arange(0, SH - RH + 1, 2):
        # ref pixel -> source px -> sieve pixel
        sx = (ox + (xx + 0.5) / w * RW) / SW * w - 0.5
        sy = (oy + (yy + 0.5) / h * RH) / SH * h - 0.5
        v = sample(s, sx, sy)
        e = np.mean(np.abs(v ** (1 / 2.2) - r[8:h - 8:2, 8:w - 8:2] ** (1 / 2.2)))
        res.append((e, ox, oy))
res.sort()
print(stem, "best (err, ox, oy):", ", ".join("(%.4f, %g, %g)" % t for t in res[:5]))
