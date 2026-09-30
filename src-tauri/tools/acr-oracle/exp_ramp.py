"""Tone-slider responses on a smooth log ramp (global behaviour of PV2012 sliders)."""
import sys, json, numpy as np
sys.path.insert(0, __file__.rsplit("/", 1)[0])
from oracle import *

W, H = 2048, 192
EV0, EV1 = -13.0, 0.0
ev = np.linspace(EV0, EV1, W)
row = 2.0 ** ev
img = np.repeat(np.repeat(row[None, :, None], H, 0), 3, 2)
img16 = (np.clip(img, 0, 1) * 65535 + 0.5).astype(np.uint16)

# Sample columns (avoid the extreme ends) as EV grid.
cols = np.arange(8, W - 8, 16)


def measure(**over):
    out = render(img16, neutral_crs(**over))
    band = out[H // 2 - 24:H // 2 + 24].astype(np.float64).mean(0)  # W x 3
    return band[cols]  # N x 3 encoded 0..255


if __name__ == "__main__":
    sets = [dict()]
    for k in ["Exposure2012"]:
        for a in [-1, 1, 2]:
            sets.append({k: a})
    for k in ["Contrast2012", "Highlights2012", "Shadows2012", "Whites2012", "Blacks2012"]:
        for a in [-100, -50, -25, 25, 50, 100]:
            sets.append({k: a})
    res = {}
    for s in sets:
        key = ",".join("%s=%s" % kv for kv in s.items()) or "default"
        res[key] = measure(**s)[:, 1].tolist()
        print(key, " ".join("%.0f" % v for v in res[key][::8]))
    json.dump({"ev": ev[cols].tolist(), "res": res}, open(HERE + "/ramp.json", "w"))
