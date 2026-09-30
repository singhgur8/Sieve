"""Per-image diagnostics of Sieve vs Camera Raw (PPMs dumped by parity_eval with SIEVE_DUMP=1).

    python3 diff.py <out-dir> [stem...]

Prints mean dL/da/db, dE2000, and dL / dE binned by the reference's L*.
"""
import os, sys
import numpy as np
from PIL import Image


def srgb_to_lab(img):
    e = img.astype(np.float64) / 255.0
    lin = np.where(e <= 0.04045, e / 12.92, ((e + 0.055) / 1.055) ** 2.4)
    m = np.array([[0.4124564, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750], [0.0193339, 0.1191920, 0.9503041]])
    xyz = lin @ m.T / np.array([0.95047, 1.0, 1.08883])
    f = np.where(xyz > 216 / 24389, np.cbrt(xyz), (24389 / 27 * xyz + 16) / 116)
    return np.stack([116 * f[..., 1] - 16, 500 * (f[..., 0] - f[..., 1]), 200 * (f[..., 1] - f[..., 2])], -1)


def de2000(l1, l2):
    L1, a1, b1 = l1[..., 0], l1[..., 1], l1[..., 2]
    L2, a2, b2 = l2[..., 0], l2[..., 1], l2[..., 2]
    c1, c2 = np.hypot(a1, b1), np.hypot(a2, b2)
    cm = (c1 + c2) / 2
    g = 0.5 * (1 - np.sqrt(cm ** 7 / (cm ** 7 + 25 ** 7)))
    a1p, a2p = a1 * (1 + g), a2 * (1 + g)
    c1p, c2p = np.hypot(a1p, b1), np.hypot(a2p, b2)
    h1p = np.degrees(np.arctan2(b1, a1p)) % 360
    h2p = np.degrees(np.arctan2(b2, a2p)) % 360
    dl, dc = L2 - L1, c2p - c1p
    dh = h2p - h1p
    dh = np.where(dh > 180, dh - 360, np.where(dh < -180, dh + 360, dh))
    dh = np.where(c1p * c2p == 0, 0, dh)
    dhh = 2 * np.sqrt(c1p * c2p) * np.sin(np.radians(dh) / 2)
    lm, cmp_ = (L1 + L2) / 2, (c1p + c2p) / 2
    hs = h1p + h2p
    hm = np.where(c1p * c2p == 0, hs, np.where(np.abs(h1p - h2p) <= 180, hs / 2, np.where(hs < 360, (hs + 360) / 2, (hs - 360) / 2)))
    t = 1 - 0.17 * np.cos(np.radians(hm - 30)) + 0.24 * np.cos(np.radians(2 * hm)) + 0.32 * np.cos(np.radians(3 * hm + 6)) - 0.20 * np.cos(np.radians(4 * hm - 63))
    dth = 30 * np.exp(-(((hm - 275) / 25) ** 2))
    rc = 2 * np.sqrt(cmp_ ** 7 / (cmp_ ** 7 + 25 ** 7))
    sl = 1 + 0.015 * (lm - 50) ** 2 / np.sqrt(20 + (lm - 50) ** 2)
    sc, sh = 1 + 0.045 * cmp_, 1 + 0.015 * cmp_ * t
    rt = -np.sin(np.radians(2 * dth)) * rc
    return np.sqrt((dl / sl) ** 2 + (dc / sc) ** 2 + (dhh / sh) ** 2 + rt * (dc / sc) * (dhh / sh))


def box3(img):
    p = np.pad(img.astype(np.float64), ((1, 1), (1, 1), (0, 0)), mode="edge")
    return sum(p[dy:dy + img.shape[0], dx:dx + img.shape[1]] for dy in range(3) for dx in range(3)) / 9


def load(d, stem):
    s = np.asarray(Image.open(os.path.join(d, stem + ".sieve.ppm")).convert("RGB"))
    r = np.asarray(Image.open(os.path.join(d, stem + ".ref.ppm")).convert("RGB"))
    return s, r


def analyse(d, stem, bins=True):
    s, r = load(d, stem)
    ls, lr = srgb_to_lab(box3(s)), srgb_to_lab(box3(r))
    de = de2000(lr, ls)
    dd = ls - lr
    line = "%-9s dE %.2f  dL %+.2f da %+.2f db %+.2f | chroma s %.1f r %.1f" % (
        stem, de.mean(), dd[..., 0].mean(), dd[..., 1].mean(), dd[..., 2].mean(),
        np.hypot(ls[..., 1], ls[..., 2]).mean(), np.hypot(lr[..., 1], lr[..., 2]).mean())
    print(line)
    if bins:
        edges = [0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 101]
        out = []
        for a, b in zip(edges[:-1], edges[1:]):
            m = (lr[..., 0] >= a) & (lr[..., 0] < b)
            if m.mean() > 0.005:
                out.append("L%02d:%3.0f%% dL%+5.1f dE%4.1f" % (a, 100 * m.mean(), dd[..., 0][m].mean(), de[m].mean()))
        for i in range(0, len(out), 5):
            print("     " + " | ".join(out[i:i + 5]))
    return de.mean()


if __name__ == "__main__":
    d = sys.argv[1]
    stems = sys.argv[2:] or sorted(f[:-10] for f in os.listdir(d) if f.endswith(".sieve.ppm"))
    for st in stems:
        analyse(d, st)
