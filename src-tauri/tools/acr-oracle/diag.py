"""Per-image diagnostics: mean L*/a*/b* differences (Sieve - reference) by luminance band."""
import glob, os, numpy as np
from PIL import Image
H = os.path.dirname(os.path.abspath(__file__))
M = np.array([[0.4124564, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750], [0.0193339, 0.1191920, 0.9503041]])
def lab(a):
    e = a / 255.0
    lin = np.where(e <= 0.04045, e / 12.92, ((e + 0.055) / 1.055) ** 2.4)
    xyz = lin @ M.T / np.array([0.95047, 1, 1.08883])
    f = np.where(xyz > 216 / 24389, np.cbrt(xyz), (24389 / 27 * xyz + 16) / 116)
    return np.stack([116 * f[..., 1] - 16, 500 * (f[..., 0] - f[..., 1]), 200 * (f[..., 1] - f[..., 2])], -1)
for p in sorted(glob.glob(H + "/../../../test-data/parity-out/*.sieve.jpg")):
    stem = os.path.basename(p).split(".")[0]
    r = H + "/../../../test-data/parity-ref/%s.ref.jpg" % stem
    if not os.path.exists(r):
        continue
    s = Image.open(p).convert("RGB")
    ref = Image.open(r).convert("RGB")
    if (ref.width > ref.height) != (s.width > s.height):
        ref = ref.transpose(Image.ROTATE_90)
    ref = ref.resize(s.size, Image.BOX)
    a, b = lab(np.asarray(s, float)[::4, ::4]), lab(np.asarray(ref, float)[::4, ::4])
    Lr = b[..., 0]
    out = []
    for lo, hi in [(0, 25), (25, 50), (50, 75), (75, 101)]:
        m = (Lr >= lo) & (Lr < hi)
        if m.sum() < 50:
            out.append("   -  ")
            continue
        d = (a - b)[m].mean(0)
        out.append("%+5.1f/%+4.1f/%+4.1f" % tuple(d))
    print("%-9s dL/da/db by ref L band [0-25 25-50 50-75 75-100]: %s" % (stem, "  ".join(out)))
