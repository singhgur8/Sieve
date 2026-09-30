import sys, json, numpy as np
sys.path.insert(0, __file__.rsplit("/", 1)[0])
from oracle import *
from exp_ramp import img16, cols, ev, H

ID = ["0, 0", "255, 255"]
sets = {}
sets["P128_160"] = (dict(ToneCurveName2012="Custom"), {"ToneCurvePV2012": ["0, 0", "128, 160", "255, 255"]})
sets["P64_96"] = (dict(ToneCurveName2012="Custom"), {"ToneCurvePV2012": ["0, 0", "64, 96", "255, 255"]})
sets["P0_40"] = (dict(ToneCurveName2012="Custom"), {"ToneCurvePV2012": ["0, 40", "255, 255"]})
sets["P255_200"] = (dict(ToneCurveName2012="Custom"), {"ToneCurvePV2012": ["0, 0", "255, 200"]})
sets["Pscurve"] = (dict(ToneCurveName2012="Custom"), {"ToneCurvePV2012": ["0, 0", "64, 48", "192, 208", "255, 255"]})
sets["Puser"] = (dict(ToneCurveName2012="Custom"), {"ToneCurvePV2012": ["0, 14", "44, 46", "106, 110", "255, 252"]})
sets["Pcolor"] = (dict(ToneCurveName2012="Custom"), {"ToneCurvePV2012": ["0, 0", "22, 16", "40, 35", "127, 127", "224, 230", "240, 246", "255, 255"]})
sets["Red128_160"] = (dict(), {"ToneCurvePV2012Red": ["0, 0", "128, 160", "255, 255"]})
for name in ["Shadows", "Darks", "Lights", "Highlights"]:
    for a in [-100, -50, 50, 100]:
        sets["%s%d" % (name, a)] = (dict(**{"Parametric" + name: a}), {})
        sets["%s%d_u" % (name, a)] = (dict(**{"Parametric" + name: a, "ParametricShadowSplit": 15, "ParametricMidtoneSplit": 35, "ParametricHighlightSplit": 75}), {})
sets["param_user"] = (dict(ParametricShadows=-5, ParametricDarks=-15, ParametricLights=20, ParametricHighlights=-15,
                           ParametricShadowSplit=15, ParametricMidtoneSplit=35, ParametricHighlightSplit=75), {})
sets["param_user_point"] = (dict(ParametricShadows=-5, ParametricDarks=-15, ParametricLights=20, ParametricHighlights=-15,
                                 ParametricShadowSplit=15, ParametricMidtoneSplit=35, ParametricHighlightSplit=75, ToneCurveName2012="Custom"),
                            {"ToneCurvePV2012": ["0, 14", "44, 46", "106, 110", "255, 252"]})
res = {}
for k, (c, s) in sets.items():
    out = render(img16, neutral_crs(**c), seqs=s)
    res[k] = out[H // 2 - 24:H // 2 + 24].astype(np.float64).mean(0)[cols].tolist()
json.dump({"ev": ev[cols].tolist(), "res": res, "sets": {k: [c, s] for k, (c, s) in sets.items()}}, open(HERE + "/curves.json", "w"))
base = np.array(json.load(open(HERE + "/ramp.json"))["res"]["default"])
for k in res:
    o = np.array(res[k])
    print("%-22s" % k, " ".join("%3.0f>%3.0f" % (b, v) for b, v in zip(base[::10], o[::10, 1])), " R-G %.0f" % (o[:, 0] - o[:, 1]).max())
