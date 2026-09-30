import sys, json, numpy as np
sys.path.insert(0, __file__.rsplit("/", 1)[0])
from oracle import *
from exp_ramp import img16, cols, ev, H

combos = {
    "u1": dict(Contrast2012=-60, Highlights2012=-66, Shadows2012=51, Whites2012=-24, Blacks2012=63),
    "u2": dict(Contrast2012=-91, Highlights2012=-43, Shadows2012=37, Whites2012=-44, Blacks2012=25),
    "u1e": dict(Exposure2012=-0.87, Contrast2012=-60, Highlights2012=-66, Shadows2012=51, Whites2012=-24, Blacks2012=63),
    "CH": dict(Contrast2012=-50, Highlights2012=-50),
    "SB": dict(Shadows2012=50, Blacks2012=50),
    "HW": dict(Highlights2012=-50, Whites2012=-50),
    "E1H": dict(Exposure2012=1, Highlights2012=-50),
    "Em1S": dict(Exposure2012=-1, Shadows2012=50),
    "E1C": dict(Exposure2012=1, Contrast2012=-50),
    "C-75": dict(Contrast2012=-75),
    "H-75": dict(Highlights2012=-75),
    "S75": dict(Shadows2012=75),
    "B75": dict(Blacks2012=75),
    "W-75": dict(Whites2012=-75),
    "C75": dict(Contrast2012=75), "H75": dict(Highlights2012=75), "S-75": dict(Shadows2012=-75),
    "B-75": dict(Blacks2012=-75), "W75": dict(Whites2012=75),
}
res = {}
for k, s in combos.items():
    out = render(img16, neutral_crs(**s))
    res[k] = out[H // 2 - 24:H // 2 + 24].astype(np.float64).mean(0)[cols, 1].tolist()
    print(k, " ".join("%.0f" % x for x in res[k][::8]))
json.dump({"ev": ev[cols].tolist(), "res": res, "combos": combos}, open(HERE + "/combo.json", "w"))
