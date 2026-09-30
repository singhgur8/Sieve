import sys, json, numpy as np
sys.path.insert(0, __file__.rsplit("/", 1)[0])
from oracle import *
from exp_ramp import img16, cols, ev, H

sets = {}
for k in ["Contrast2012", "Highlights2012", "Shadows2012", "Whites2012", "Blacks2012"]:
    for a in [-10, 10]:
        sets["%s=%d" % (k, a)] = {k: a}
for e in [-1, 1, 2]:
    for k, a in [("Contrast2012", -50), ("Contrast2012", 50), ("Blacks2012", 50), ("Blacks2012", -50),
                 ("Highlights2012", -50), ("Shadows2012", 50), ("Whites2012", -50), ("Whites2012", 50)]:
        sets["E%d,%s=%d" % (e, k, a)] = {"Exposure2012": e, k: a}
# More user-like combos for validation.
sets["v1"] = dict(Exposure2012=-1.14, Contrast2012=-91, Highlights2012=-43, Shadows2012=37, Whites2012=-44, Blacks2012=25)
sets["v2"] = dict(Exposure2012=0.55, Contrast2012=-60, Highlights2012=-66, Shadows2012=64, Whites2012=-18, Blacks2012=63)
sets["v3"] = dict(Exposure2012=1.37, Contrast2012=25, Highlights2012=21, Shadows2012=-22, Whites2012=42, Blacks2012=-28)
sets["v4"] = dict(Exposure2012=0.35, Contrast2012=-59, Highlights2012=-66, Shadows2012=30, Whites2012=-18, Blacks2012=25)
res = {}
for k, s in sets.items():
    out = render(img16, neutral_crs(**s))
    res[k] = out[H // 2 - 24:H // 2 + 24].astype(np.float64).mean(0)[cols, 1].tolist()
json.dump({"ev": ev[cols].tolist(), "res": res, "sets": sets}, open(HERE + "/more.json", "w"))
print(len(res), "renders")
