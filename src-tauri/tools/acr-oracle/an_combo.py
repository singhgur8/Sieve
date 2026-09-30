import json, numpy as np
H = __file__.rsplit("/", 1)[0]
r1 = json.load(open(H + "/ramp.json")); r2 = json.load(open(H + "/combo.json")); r3 = json.load(open(H + "/expo.json"))
ev = np.array(r1["ev"])
R = {k: np.array(v) for k, v in {**r1["res"], **r2["res"], **r3["res"]}.items()}
base = R["default"]
LO = -20.0
# Inverse of T (encoded 0..255 -> EV) using the default curve, extended linearly below.
bm = np.maximum.accumulate(base + np.arange(len(base)) * 1e-4)
def tinv(o):
    return np.interp(o, bm, ev, left=LO, right=0.3)
def t(e):
    return np.interp(e, ev, base, left=0, right=255)
names = {"C": "Contrast2012", "H": "Highlights2012", "S": "Shadows2012", "W": "Whites2012", "B": "Blacks2012"}
def delta(s, a):
    if a == 0:
        return np.zeros_like(ev)
    amps = [-100, -75, -50, -25, 0, 25, 50, 75, 100]
    curves = []
    for m in amps:
        if m == 0:
            curves.append(np.zeros_like(ev)); continue
        key = "%s=%d" % (names[s], m)
        alt = {"C": "C", "H": "H", "S": "S", "W": "W", "B": "B"}[s] + ("%d" % m)
        o = R.get(key)
        if o is None:
            o = R.get(alt)
        if o is None:
            curves.append(None); continue
        curves.append(tinv(o) - ev)
    pts = [(m, c) for m, c in zip(amps, curves) if c is not None]
    ms = np.array([p[0] for p in pts]); cs = np.array([p[1] for p in pts])
    out = np.empty_like(ev)
    for i in range(len(ev)):
        out[i] = np.interp(a, ms, cs[:, i])
    return out
def predict(c):
    d = np.zeros_like(ev)
    for k, v in c.items():
        s = {"Contrast2012": "C", "Highlights2012": "H", "Shadows2012": "S", "Whites2012": "W", "Blacks2012": "B"}.get(k)
        if s:
            d += delta(s, v)
    e = c.get("Exposure2012", 0)
    return t(ev + e + d)
for k, c in r2["combos"].items():
    p = predict(c)
    err = p - R[k]
    m = ev > -9
    print("%-5s mean|err| %.1f  max %.1f   | %s" % (k, np.abs(err[m]).mean(), np.abs(err[m]).max(), " ".join("%+.0f" % x for x in err[::8])))

import itertools
def compose(c, order):
    e = ev + c.get("Exposure2012", 0)
    for s in order:
        key = names[s]
        if key in c:
            d = delta(s, c[key])
            e = e + np.interp(e, ev, d, left=d[0], right=d[-1])
    return t(e)
scores = []
test = ["u1", "u2", "u1e", "CH", "SB", "HW", "E1C", "E1H", "Em1S"]
for order in itertools.permutations("CHSWB"):
    tot = 0
    for k in test:
        c = r2["combos"][k]
        p = compose(c, order)
        m = ev > -9
        tot += np.abs(p - R[k])[m].mean()
    scores.append((tot / len(test), "".join(order)))
scores.sort()
print(scores[:8], scores[-3:])
def compose2(c, order):
    e = ev.copy()
    for s in order:
        if s == "E":
            e = e + c.get("Exposure2012", 0)
            continue
        key = names[s]
        if key in c:
            d = delta(s, c[key])
            e = e + np.interp(e, ev, d, left=d[0], right=d[-1])
    return t(e)
scores = []
for order in itertools.permutations("CHSWBE"):
    tot = 0
    for k in test:
        p = compose2(r2["combos"][k], order)
        m = ev > -9
        tot += np.abs(p - R[k])[m].mean()
    scores.append((tot / len(test), "".join(order)))
scores.sort()
print(scores[:8])
best = scores[0][1]
for k in test:
    p = compose2(r2["combos"][k], best); m = ev > -9
    print(k, "%.1f" % np.abs(p - R[k])[m].mean(), " ".join("%+.0f" % x for x in (p - R[k])[::8]))
