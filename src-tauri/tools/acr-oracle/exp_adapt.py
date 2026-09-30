"""Is PV2012 tone image-adaptive? Same settings on ramps of different range."""
import sys, numpy as np
sys.path.insert(0, __file__.rsplit("/", 1)[0])
from oracle import *
sys.argv = ["x"]
import tone_model as tm

W, H = 2048, 192
sets = {
    "user": dict(Exposure2012=-1.59, Contrast2012=-59, Highlights2012=-66, Shadows2012=37, Whites2012=-9, Blacks2012=25),
    "H-66": dict(Highlights2012=-66),
    "E-1.59": dict(Exposure2012=-1.59),
    "W-50": dict(Whites2012=-50),
    "S+50": dict(Shadows2012=50),
    "C-60": dict(Contrast2012=-60),
    "B+50": dict(Blacks2012=50),
    "none": dict(),
}
for lo, hi in [(-13, 0), (-4, 0), (-8, -2), (-3, -1)]:
    evs = np.linspace(lo, hi, W)
    row = 2.0 ** evs
    img = np.repeat(np.repeat(row[None, :, None], H, 0), 3, 2)
    img16 = (np.clip(img, 0, 1) * 65535 + 0.5).astype(np.uint16)
    cols = np.arange(8, W - 8, 16)
    for name, c in sets.items():
        out = render(img16, neutral_crs(**c))
        o = out[H // 2 - 24:H // 2 + 24].astype(np.float64).mean(0)[cols, 1]
        p = tm.model({k: float(v) for k, v in c.items()}, x=evs[cols])
        d = o - p
        print("range [%d,%d] %-7s acr-model mean %+.1f  at lo %+.1f mid %+.1f hi %+.1f   (acr hi %.0f)" % (lo, hi, name, d.mean(), d[:10].mean(), d[len(d)//2-5:len(d)//2+5].mean(), d[-10:].mean(), o[-10:].mean()))
