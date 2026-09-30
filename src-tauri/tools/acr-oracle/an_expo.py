import json, numpy as np
d = json.load(open(__file__.rsplit("/", 1)[0] + "/expo.json"))
ev = np.array(d["ev"])
R = {k: np.array(v) for k, v in d["res"].items()}
base = R["E0"]
# Inverse of the default curve: encoded output -> input EV (monotone part only).
def tinv(o):
    m = np.maximum.accumulate(base + np.arange(len(base)) * 1e-6)
    return np.interp(o, m, ev, left=np.nan, right=np.nan)
for k in ["E-3", "E-2", "E-1", "E-0.5", "E0.5", "E1", "E2", "E3", "E4"]:
    e = float(k[1:])
    o = R[k]
    ti = tinv(o)
    print(k)
    for t in [-9, -7, -5, -4, -3, -2.5, -2, -1.5, -1, -0.7, -0.4, -0.15]:
        i = np.argmin(abs(ev - t))
        print("   x_ev %.2f  eff %.2f  out %.1f  inEV %.2f  delta %.2f" % (ev[i], ev[i] + e, o[i], ti[i], ti[i] - (ev[i] + e)))
