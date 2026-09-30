import re, glob, os, sys, random

root = "/Users/gurjotsingh/Pictures/Jasmit Natalie Proposal"
rows = []
for x in sorted(glob.glob(root + "/**/*.xmp", recursive=True)):
    stem = x[:-4]
    raw = None
    for e in ("ARW", "CR3", "RAF"):
        if os.path.exists(stem + "." + e):
            raw = stem + "." + e
    if not raw:
        continue
    s = open(x).read()
    f = {}
    f["ext"] = raw[-3:]
    f["mask"] = "MaskGroupBasedCorrections" in s or "crs:GradientBasedCorrections" in s or "PaintBasedCorrections" in s
    f["crop"] = 'crs:HasCrop="True"' in s
    m = re.search(r'crs:CropAngle="([^"]*)"', s)
    f["angle"] = float(m.group(1)) if m else 0.0
    f["mono"] = "Adobe Monochrome" in s
    f["gray"] = 'crs:ConvertToGrayscale="True"' in s
    m = re.search(r'<crs:Look>.*?crs:Name="([^"]*)"', s, re.S)
    f["look"] = m.group(1) if m else ""
    f["grade"] = bool(re.search(r'crs:ColorGradeMidtoneSat="[1-9]', s)) or bool(re.search(r'crs:SplitToningShadowSaturation="[1-9]', s))
    f["vig"] = bool(re.search(r'crs:PostCropVignetteAmount="-?[1-9]', s))
    f["grain"] = bool(re.search(r'crs:GrainAmount="[1-9]', s))
    f["lnr"] = bool(re.search(r'crs:LuminanceSmoothing="[1-9]', s))
    f["retouch"] = "RetouchAreas" in s
    f["lens"] = 'crs:LensProfileEnable="1"' in s
    f["upright"] = bool(re.search(r'crs:PerspectiveUpright="[1-9]', s))
    rows.append((raw, f))

print(len(rows), "pairs")
from collections import Counter
print(Counter(f["ext"] for _, f in rows))
print("mask", sum(f["mask"] for _, f in rows), "crop", sum(f["crop"] for _, f in rows), "mono", sum(f["mono"] for _, f in rows),
      "vig", sum(f["vig"] for _, f in rows), "grain", sum(f["grain"] for _, f in rows), "lnr", sum(f["lnr"] for _, f in rows),
      "retouch", sum(f["retouch"] for _, f in rows), "lens", sum(f["lens"] for _, f in rows), "upright", sum(f["upright"] for _, f in rows))
clean = [(r, f) for r, f in rows if not f["mask"] and not f["retouch"] and not f["upright"]]
print("clean", len(clean), Counter(f["ext"] for _, f in clean))
random.seed(7)
pick = []
def take(pred, n):
    c = [r for r in clean if pred(r[1]) and r not in pick]
    random.shuffle(c)
    pick.extend(c[:n])
take(lambda f: f["ext"] == "ARW" and f["crop"] and abs(f["angle"]) > 0.5, 2)
take(lambda f: f["ext"] == "ARW" and f["crop"], 2)
take(lambda f: f["ext"] == "ARW" and f["mono"], 2)
take(lambda f: f["ext"] == "ARW" and f["vig"], 1)
take(lambda f: f["ext"] == "ARW", 5)
take(lambda f: f["ext"] == "CR3" and f["crop"], 1)
take(lambda f: f["ext"] == "CR3", 4)
take(lambda f: f["ext"] == "RAF" and f["crop"], 1)
take(lambda f: f["ext"] == "RAF", 3)
for r, f in pick:
    print(r, {k: v for k, v in f.items() if v})
if len(sys.argv) > 1:
    open(sys.argv[1], "w").write("\n".join(r for r, _ in pick) + "\n")
