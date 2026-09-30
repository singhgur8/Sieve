"""Key-relative Shadows/Highlights: tables indexed by ev - key + KEY_CAL."""
import sys, numpy as np
sys.path.insert(0, __file__.rsplit("/", 1)[0])
from oracle import *
sys.argv = ["x"]
import tone_model as tm

KEY_CAL = -6.5


def model(c, x, key):
    e = x.copy()
    shift = KEY_CAL - key
    for s in "SH":
        if tm.NAMES[s] in c:
            sh = shift if s == "H" else 0.0
            e = tm.slider(s, c[tm.NAMES[s]], e + sh) - sh
    if "Whites2012" in c:
        e = tm.slider("W", c["Whites2012"], e)
    e = tm.exposure(e, c.get("Exposure2012", 0))
    for s in "CB":
        if tm.NAMES[s] in c:
            e = tm.slider(s, c[tm.NAMES[s]], e)
    return tm.t_enc(e)


W, H = 2048, 192
sets = {
    "user": dict(Exposure2012=-1.59, Contrast2012=-59, Highlights2012=-66, Shadows2012=37, Whites2012=-9, Blacks2012=25),
    "H-66": dict(Highlights2012=-66),
    "H+50": dict(Highlights2012=50),
    "S+50": dict(Shadows2012=50),
    "S-50": dict(Shadows2012=-50),
}
for lo, hi in [(-13, 0), (-4, 0), (-8, -2), (-3, -1), (-10, -4), (-6, 0)]:
    evs = np.linspace(lo, hi, W)
    img = np.repeat(np.repeat((2.0 ** evs)[None, :, None], H, 0), 3, 2)
    img16 = (np.clip(img, 0, 1) * 65535 + 0.5).astype(np.uint16)
    cols = np.arange(8, W - 8, 16)
    for name, c in sets.items():
        out = render(img16, neutral_crs(**c))
        o = out[H // 2 - 24:H // 2 + 24].astype(np.float64).mean(0)[cols, 1]
        cc = {k: float(v) for k, v in c.items()}
        for kname, key in [("abs", KEY_CAL), ("mean", evs.mean())]:
            p = model(cc, evs[cols], key)
            d = o - p
            print("[%d,%d] %-5s %-4s mean|d| %5.1f lo %+6.1f mid %+6.1f hi %+6.1f" % (lo, hi, name, kname, np.abs(d).mean(), d[:10].mean(), d[len(d)//2-5:len(d)//2+5].mean(), d[-10:].mean()))
