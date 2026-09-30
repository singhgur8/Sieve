"""Are the parametric / point curves image-adaptive or exposure-dependent?"""
import sys, numpy as np
sys.path.insert(0, __file__.rsplit("/", 1)[0])
from oracle import *
sys.argv = ["x"]
import tone_model as tm
HERE_ = __file__.rsplit("/", 1)[0]
src = open(HERE_ + "/param_model.py").read().split("tot = []")[0].replace("__file__", repr(HERE_ + "/param_model.py"))
g = {}
exec(src, g)
src2 = open(HERE_ + "/spline_test.py").read().split("for k in")[0].replace("__file__", repr(HERE_ + "/spline_test.py"))
g2 = {}
exec(src2, g2)
curve, natural = g["curve"], g2["natural"]

W, H = 2048, 192
P = dict(ParametricShadows=-5, ParametricDarks=-15, ParametricLights=20, ParametricHighlights=-15,
         ParametricShadowSplit=15, ParametricMidtoneSplit=35, ParametricHighlightSplit=75)
pts = [(0, 14), (44, 46), (106, 110), (255, 252)]
f = natural([p[0] for p in pts], [p[1] for p in pts])
for lo, hi in [(-13, 0), (-6, 0), (-9, -3)]:
    evs = np.linspace(lo, hi, W)
    img16 = (np.clip(np.repeat(np.repeat((2.0 ** evs)[None, :, None], H, 0), 3, 2), 0, 1) * 65535 + 0.5).astype(np.uint16)
    cols = np.arange(8, W - 8, 16)
    for E in [0, -1]:
        for name, extra, seq in [("param", P, None), ("point", {"ToneCurveName2012": "Custom"}, {"ToneCurvePV2012": ["%d, %d" % p for p in pts]})]:
            c = dict(Exposure2012=E, **extra)
            out = render(img16, neutral_crs(**c), seqs=seq)
            o = out[H // 2 - 24:H // 2 + 24].astype(np.float64).mean(0)[cols, 1]
            base = tm.model({"Exposure2012": float(E)}, x=evs[cols]) / 255.0
            if name == "param":
                p = curve(base, {k: float(v) for k, v in P.items()}) * 255
            else:
                p = f(base * 255)
            d = o - p
            print("[%d,%d] E%+d %-5s acr-model mean %+.1f |d| %.1f  lo %+.1f mid %+.1f hi %+.1f" % (lo, hi, E, name, d.mean(), np.abs(d).mean(), d[:10].mean(), d[len(d)//2-5:len(d)//2+5].mean(), d[-10:].mean()))
