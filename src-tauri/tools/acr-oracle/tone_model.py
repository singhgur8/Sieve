"""Fits Sieve's PV2012 tone model to the ACR oracle measurements and emits Rust tables."""
import json, sys, numpy as np
HERE = __file__.rsplit("/", 1)[0]
R = {}
EV = None
for f in ["ramp.json", "expo.json", "combo.json", "more.json"]:
    d = json.load(open(HERE + "/" + f))
    EV = np.array(d["ev"])
    for k, v in d["res"].items():
        R[k] = np.array(v)
SETS = {}
for f in ["combo.json", "more.json"]:
    d = json.load(open(HERE + "/" + f))
    SETS.update(d.get("combos", {}))
    SETS.update(d.get("sets", {}))

def srgb_dec(e):
    e = np.asarray(e, float) / 255.0
    return np.where(e <= 0.04045, e / 12.92, ((e + 0.055) / 1.055) ** 2.4)

def srgb_enc(v):
    v = np.clip(v, 0, 1)
    return 255 * np.where(v <= 0.0031308, 12.92 * v, 1.055 * v ** (1 / 2.4) - 0.055)

# ---- Base curve T: EV grid -> display linear. Combine patch data (to -16 EV) and ramp.
tp = np.load(HERE + "/tone_default.npy")  # lin in, display lin out
GRID = np.arange(-14.0, 0.0001, 0.125)
ramp_out = srgb_dec(R["default"])
xs = np.concatenate([np.log2(tp[:, 0]), EV])
ys = np.concatenate([tp[:, 1], ramp_out])
o = np.argsort(xs)
xs, ys = xs[o], ys[o]
# Smooth in log-output domain with a local linear regression, then force monotone.
T = np.empty_like(GRID)
for i, g in enumerate(GRID):
    w = np.exp(-0.5 * ((xs - g) / 0.2) ** 2)
    m = w > 1e-4
    A = np.stack([np.ones(m.sum()), xs[m] - g], 1)
    W = w[m]
    yy = ys[m]
    coef = np.linalg.lstsq(A * W[:, None], yy * W, rcond=None)[0]
    T[i] = max(coef[0], 0)
T = np.maximum.accumulate(T)
T[-1] = 1.0
# Toe: below ~-11 EV the data is quantised; use a straight line through the origin.
k = np.searchsorted(GRID, -10.0)
slope = T[k] / 2.0 ** GRID[k]
T[:k] = slope * 2.0 ** GRID[:k]

def t_enc(e):
    lin = np.interp(e, GRID, T, left=0.0, right=1.0)
    lin = np.where(e < GRID[0], slope * 2.0 ** e, lin)
    return srgb_enc(lin)

def t_inv(enc):
    """Encoded output -> input EV (NaN when saturated)."""
    lin = srgb_dec(enc)
    out = np.interp(lin, T, GRID, left=np.nan, right=np.nan)
    out = np.where(lin <= T[0], np.log2(np.maximum(lin, 1e-9) / slope), out)
    out = np.where(enc >= 254.6, np.nan, out)
    return out

err = t_enc(EV) - R["default"]
print("base curve fit: mean %.2f max %.2f" % (np.abs(err[EV > -11]).mean(), np.abs(err[EV > -11]).max()))

NAMES = {"S": "Shadows2012", "H": "Highlights2012", "W": "Whites2012", "C": "Contrast2012", "B": "Blacks2012"}
AMPS = [-100, -75, -50, -25, -10, 0, 10, 25, 50, 75, 100]
DGRID = np.arange(-13.0, 0.0001, 0.25)

def fill(d):
    """Fills NaN deltas (saturated output) by holding the last finite value."""
    d = d.copy()
    idx = np.where(np.isfinite(d))[0]
    if len(idx) == 0:
        return np.zeros_like(d)
    for i in range(len(d)):
        if not np.isfinite(d[i]):
            j = idx[np.argmin(np.abs(idx - i))]
            d[i] = d[j]
    return d

def key_for(name, a):
    for k in ["%s=%d" % (name, a)]:
        if k in R:
            return R[k]
    short = {"Contrast2012": "C", "Highlights2012": "H", "Shadows2012": "S", "Whites2012": "W", "Blacks2012": "B"}[name]
    return R.get("%s%d" % (short, a))

TABLES = {}
for s, name in NAMES.items():
    rows = []
    for a in AMPS:
        if a == 0:
            rows.append(np.zeros_like(DGRID))
            continue
        out = key_for(name, a)
        d = fill(t_inv(out) - EV)
        # Clamp crushed blacks to a finite floor.
        d = np.maximum(d, -12.0)
        row = np.interp(DGRID, EV, d)
        # Smooth (8-bit quantisation noise in the shadows makes the raw deltas wiggle,
        # which shows up as contour bands when the table is used locally).
        k = np.exp(-0.5 * (np.arange(-6, 7) / 2.0) ** 2)
        k /= k.sum()
        pad = np.concatenate([np.full(6, row[0]), row, np.full(6, row[-1])])
        row = np.convolve(pad, k, mode="valid")
        rows.append(row)
    TABLES[s] = np.array(rows)

# Exposure: out(x, E) = x + E + dE(x, E) on the pre-exposure domain.
EXPS = [-3, -2, -1.5, -1, -0.5, -0.25, 0, 0.25, 0.5, 1, 1.5, 2, 3, 4]
erows = []
for e in EXPS:
    out = R["E%s" % (e if e != 0 else 0)] if ("E%s" % e) in R else R["E0"]
    ti = t_inv(out)
    d = ti - (EV + e)
    # Saturated: keep the rendered value at >= white (0 EV): delta that maps to 0.
    sat = ~np.isfinite(d)
    d[sat] = np.maximum(-(EV[sat] + e), np.nan_to_num(d, nan=0.0)[sat])
    d = fill(d)
    row = np.interp(DGRID, EV, d)
    # Exposure is a pure gain away from the highlights (the measured deltas there are 8-bit
    # quantisation noise): keep only the highlight shoulder.
    w = np.clip((DGRID + 7.0) / 2.0, 0, 1)
    w = w * w * (3 - 2 * w)
    row = row * w
    if e == 0:
        row[:] = 0.0
    erows.append(row)
ETAB = np.array(erows)

def slider(s, a, e):
    tab = TABLES[s]
    a = float(np.clip(a, -100, 100))
    d = np.array([np.interp(a, AMPS, tab[:, j]) for j in range(len(DGRID))])
    return e + np.interp(e, DGRID, d, left=d[0], right=d[-1])

def exposure(e_in, E):
    E = float(np.clip(E, EXPS[0], EXPS[-1]))
    d = np.array([np.interp(E, EXPS, ETAB[:, j]) for j in range(len(DGRID))])
    return e_in + E + np.interp(e_in, DGRID, d, left=d[0], right=d[-1])

def model(c, x=EV):
    e = x.copy()
    for s in "SHW":
        if NAMES[s] in c:
            e = slider(s, c[NAMES[s]], e)
    e = exposure(e, c.get("Exposure2012", 0))
    for s in "CB":
        if NAMES[s] in c:
            e = slider(s, c[NAMES[s]], e)
    return t_enc(e)

if __name__ == "__main__":
    tot = []
    for k, c in SETS.items():
        c = {kk: float(v) for kk, v in c.items()}
        p = model(c)
        m = EV > -10
        e = np.abs(p - R[k])[m]
        tot.append(e.mean())
        print("%-28s mean %.1f max %.1f | %s" % (k, e.mean(), e.max(), " ".join("%+.0f" % v for v in (p - R[k])[::8])))
    print("overall mean %.2f" % np.mean(tot))
    if len(sys.argv) > 1:
        with open(sys.argv[1], "w") as f:
            f.write("//! Generated by tools/acr-oracle/tone_model.py from Adobe Camera Raw measurements (ACR 17.5\n")
            f.write("//! via Adobe DNG Converter on synthetic linear DNGs): Sieve's model of the PV2012 base\n")
            f.write("//! tone curve and the global response of the tone sliders. Measured behaviour, no Adobe\n")
            f.write("//! code or data. Regenerate with `python3 tools/acr-oracle/tone_model.py <this file>`.\n#![allow(clippy::approx_constant)]\n\n")
            f.write("/// Base curve: display-linear output at `BASE_EV0 + i * BASE_STEP` EV (scene-linear input,\n/// raw clip at 0 EV); below the table the curve is linear with slope `BASE_TOE_SLOPE`.\n")
            f.write("pub const BASE_EV0: f32 = %.4f;\npub const BASE_STEP: f32 = %.4f;\npub const BASE_TOE_SLOPE: f32 = %.6f;\n" % (GRID[0], GRID[1] - GRID[0], slope))
            f.write("pub const BASE_CURVE: [f32; %d] = [%s];\n\n" % (len(T), ", ".join("%.6f" % v for v in T)))
            f.write("/// Slider delta tables: EV delta at `DELTA_EV0 + j * DELTA_STEP` for each amount in `AMOUNTS`.\n")
            f.write("pub const DELTA_EV0: f32 = %.4f;\npub const DELTA_STEP: f32 = %.4f;\npub const DELTA_N: usize = %d;\n" % (DGRID[0], DGRID[1] - DGRID[0], len(DGRID)))
            f.write("pub const AMOUNTS: [f32; %d] = [%s];\n" % (len(AMPS), ", ".join("%.1f" % a for a in AMPS)))
            for s, nm in [("S", "SHADOWS"), ("H", "HIGHLIGHTS"), ("W", "WHITES"), ("C", "CONTRAST"), ("B", "BLACKS")]:
                f.write("pub const %s: [[f32; DELTA_N]; %d] = [\n" % (nm, len(AMPS)))
                for row in TABLES[s]:
                    f.write("    [%s],\n" % ", ".join("%.4f" % v for v in row))
                f.write("];\n")
            f.write("/// Exposure: delta (EV) on top of `x + E` for each exposure in `EXPOSURES`, over the\n/// pre-exposure EV grid.\n")
            f.write("pub const EXPOSURES: [f32; %d] = [%s];\n" % (len(EXPS), ", ".join("%.2f" % a for a in EXPS)))
            f.write("pub const EXPOSURE: [[f32; DELTA_N]; %d] = [\n" % len(EXPS))
            for row in ETAB:
                f.write("    [%s],\n" % ", ".join("%.4f" % v for v in row))
            f.write("];\n")
