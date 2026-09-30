"""Slider response on a real frame: output luminance with vs without a slider, ACR vs Sieve."""
import sys, os, numpy as np
from PIL import Image
H = os.path.dirname(os.path.abspath(__file__))
stem, a, b = sys.argv[1], sys.argv[2], sys.argv[3]
V = H + "/variants/" + stem
def Y(p):
    im = np.asarray(Image.open(p).convert("RGB"), float) / 255
    lin = np.where(im <= 0.04045, im / 12.92, ((im + 0.055) / 1.055) ** 2.4)
    return lin @ np.array([0.2126, 0.7152, 0.0722])
def load(kind, name):
    if kind == "acr":
        return Image.open(V + "/%s.jpg" % name)
    return Image.open(V + "/sieve_%s/%s.sieve.jpg" % (name, stem))
for kind in ["acr", "sieve"]:
    ia, ib = load(kind, a), load(kind, b)
    size = (512, int(512 * ia.height / ia.width)) if ia.width >= ia.height else (int(512 * ia.width / ia.height), 512)
    if (ia.width > ia.height) != (ib.width > ib.height):
        ib = ib.transpose(Image.ROTATE_90)
    ya = Y_img = None
    fa = np.asarray(ia.resize(size, Image.BOX).convert("RGB"), float) / 255
    fb = np.asarray(ib.resize(size, Image.BOX).convert("RGB"), float) / 255
    lin = lambda im: np.where(im <= 0.04045, im / 12.92, ((im + 0.055) / 1.055) ** 2.4) @ np.array([0.2126, 0.7152, 0.0722])
    ya, yb = lin(fa), lin(fb)
    eb = 116 * np.cbrt(yb) - 16
    d = np.log2(np.maximum(ya, 1e-5)) - np.log2(np.maximum(yb, 1e-5))
    row = []
    for lo in range(0, 100, 10):
        m = (eb >= lo) & (eb < lo + 10)
        row.append("%+5.2f(%4.1f%%)" % (d[m].mean(), 100 * m.mean()) if m.sum() > 20 else "   -          ")
    print("%-6s %s vs %s  dEV by L* of %s: %s" % (kind, a, b, b, " ".join(row)))
