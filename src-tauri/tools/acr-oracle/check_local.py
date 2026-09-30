"""Per-image bias of the fitted local model (gen_local config) for one variant, binned by
post-exposure EV: measured d vs model prediction.

    python3 check_local.py <variant> [stems...]
"""
import os, sys
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fit_local as fl
import gen_local as gl

variant = sys.argv[1]
stems = sys.argv[2:] or fl.stems()
data = [fl.image_data(st, [variant], [gl.R1, gl.R2], [gl.EPS], n=60000) for st in fl.stems()]
T, rmse, _ = gl.fit_T(data, variant, "bright" if "nS" in variant else "dark")
print("pooled rmse %.3f" % rmse)
for im in data:
    if im["stem"] not in stems:
        continue
    m = gl.W1 * im["f"][(gl.R1, gl.EPS)] + gl.W2 * im["f"][(gl.R2, gl.EPS)]
    ref = gl.LAM * im["key"] + (1 - gl.LAM) * im["white"]
    rel = m - ref
    pred = fl.hat(rel) @ T
    d = im["d"][variant]
    ok = np.isfinite(d)
    row = []
    for a in np.arange(-12, 1, 1.0):
        s = ok & (im["e"] >= a) & (im["e"] < a + 1)
        if s.sum() > 200:
            row.append("%+.0f: %+.2f/%+.2f" % (a, np.median(d[s]), np.median(pred[s])))
    print("%s key %.2f white %.2f ref %.2f | bias %+.2f | %s" % (im["stem"], im["key"], im["white"], ref,
          np.mean(d[ok] - pred[ok]), "  ".join(row)))
