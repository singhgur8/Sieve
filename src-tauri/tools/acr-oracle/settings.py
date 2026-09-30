"""Prints the main crs: settings of each RAW's sidecar (list file: one RAW path per line)."""
import re, sys, os
keys = ["Exposure2012", "Contrast2012", "Highlights2012", "Shadows2012", "Whites2012", "Blacks2012", "Texture",
        "Clarity2012", "Dehaze", "Vibrance", "Saturation", "CameraProfile", "Temperature", "Tint",
        "WhiteBalance", "ConvertToGrayscale", "LuminanceSmoothing", "HasCrop"]
ab = {"Exposure2012": "Ex", "Contrast2012": "Co", "Highlights2012": "Hi", "Shadows2012": "Sh", "Whites2012": "Wh",
      "Blacks2012": "Bl", "Texture": "Tx", "Clarity2012": "Cl", "Dehaze": "Dh", "Vibrance": "Vi", "Saturation": "Sa",
      "CameraProfile": "Prof", "Temperature": "T", "Tint": "Ti", "WhiteBalance": "WB", "ConvertToGrayscale": "BW",
      "LuminanceSmoothing": "NR", "HasCrop": "Crop"}
for line in open(sys.argv[1]):
    p = line.strip()
    if not p:
        continue
    x = os.path.splitext(p)[0] + ".xmp"
    t = open(x).read()
    out = []
    for k in keys:
        m = re.search(r'crs:%s="([^"]*)"' % k, t) or re.search(r'<crs:%s>([^<]*)<' % k, t)
        if m and m.group(1) not in ("0", "+0", "False", "0.00"):
            out.append("%s=%s" % (ab[k], m.group(1)))
    extra = [k for k in ["ToneCurvePV2012Red", "ColorGrade", "HueAdjustment", "Look>", "MaskGroup", "PointColors"] if k in t]
    print(os.path.basename(x)[:-4], " ".join(out), extra)
