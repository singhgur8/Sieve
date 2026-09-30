import sys, json, numpy as np
sys.path.insert(0, __file__.rsplit("/", 1)[0])
from oracle import *
from exp_ramp import img16, cols, ev, H

res = {}
for e in [-3, -2, -1.5, -1, -0.5, -0.25, 0, 0.25, 0.5, 1, 1.5, 2, 3, 4]:
    out = render(img16, neutral_crs(Exposure2012=e))
    res["E%s" % e] = out[H // 2 - 24:H // 2 + 24].astype(np.float64).mean(0)[cols, 1].tolist()
for b in [0.35, 1.0]:
    out = render(img16, neutral_crs(), baseline=b)
    res["B%s" % b] = out[H // 2 - 24:H // 2 + 24].astype(np.float64).mean(0)[cols, 1].tolist()
json.dump({"ev": ev[cols].tolist(), "res": res}, open(HERE + "/expo.json", "w"))
for k, v in res.items():
    print(k, " ".join("%.0f" % x for x in v[::8]))
