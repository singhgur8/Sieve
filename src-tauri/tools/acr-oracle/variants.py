"""Renders settings variants of real frames with Camera Raw (via variant.sh), in parallel.

    python3 variants.py <list> <variant>[,<variant>...] [-j N]

Variants zero groups of settings of the user's edit (the rest stays as edited), or set
explicit values ("set:Shadows2012=50;Highlights2012=0" style names are also accepted as
`name=set:...`).
"""
import os, subprocess, sys
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
COLORS = ["Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta"]
Z = {
    "tone": ["Contrast2012", "Highlights2012", "Shadows2012", "Whites2012", "Blacks2012"],
    "local": ["Texture", "Clarity2012", "Dehaze"],
    "color": ["Vibrance", "Saturation"] + ["%sAdjustment%s" % (k, c) for k in ("Hue", "Saturation", "Luminance") for c in COLORS]
    + ["ColorGradeMidtoneSat", "ColorGradeGlobalSat", "ColorGradeShadowLum", "ColorGradeMidtoneLum",
       "ColorGradeHighlightLum", "ColorGradeGlobalLum", "SplitToningShadowSaturation", "SplitToningHighlightSaturation"],
    "cal": ["RedHue", "RedSaturation", "GreenHue", "GreenSaturation", "BlueHue", "BlueSaturation", "ShadowTint"],
    "pcurve": ["ParametricShadows", "ParametricDarks", "ParametricLights", "ParametricHighlights"],
    "detail": ["Sharpness", "LuminanceSmoothing", "ColorNoiseReduction"],
}
LINEAR = ["ToneCurvePV2012", "ToneCurvePV2012Red", "ToneCurvePV2012Green", "ToneCurvePV2012Blue"]


def args_for(groups, sets=()):
    a = []
    for g in groups:
        if g == "curve":
            for t in LINEAR:
                # Repeated `-TAG=v` in one command builds a new list (`+=` would append).
                a += ["-XMP-crs:%s=0, 0" % t, "-XMP-crs:%s=255, 255" % t]
            g = "pcurve"
        for t in Z[g]:
            a.append("-XMP-crs:%s=0" % t)
    for kv in sets:
        a.append("-XMP-crs:%s" % kv)
    return a


ALL = ["tone", "local", "color", "cal", "curve", "detail"]
VARIANTS = {
    "asis": [],
    "neutral": args_for(ALL),
    "tone": args_for([g for g in ALL if g != "tone"]),
    "tonecurve": args_for(["local", "color", "cal", "detail"]),
    "nocolor": args_for(["color", "cal"]),
    "nolocal": args_for(["local"]),
    "notone": args_for(["tone"]),
}
# Single-slider sweeps on the neutral base.
for k, tag in [("S", "Shadows2012"), ("H", "Highlights2012"), ("W", "Whites2012"), ("B", "Blacks2012"),
               ("C", "Contrast2012"), ("Cl", "Clarity2012"), ("Tx", "Texture"), ("Dh", "Dehaze")]:
    for v in (-100, -50, 50, 100):
        VARIANTS["n%s%+d" % (k, v)] = args_for(ALL, ["%s=%d" % (tag, v)])


def main():
    lst, names = sys.argv[1], sys.argv[2].split(",")
    jobs = int(sys.argv[sys.argv.index("-j") + 1]) if "-j" in sys.argv else 6
    raws = [l.strip() for l in open(lst) if l.strip()]
    tasks = []
    for n in names:
        if "=" in n and n.split("=", 1)[1].startswith("set:"):
            name, spec = n.split("=", 1)
            base, _, sets = spec[4:].partition("|")
            a = args_for(ALL if base == "neutral" else [], [s for s in sets.split(";") if s])
        else:
            name, a = n, VARIANTS[n]
        for r in raws:
            tasks.append([os.path.join(HERE, "variant.sh"), r, name] + a)
    # Base DNGs first (serial per file, avoids concurrent creation).
    with ThreadPoolExecutor(jobs) as ex:
        list(ex.map(lambda t: subprocess.run(t, check=False), tasks))


if __name__ == "__main__":
    main()
