"""Colour-slider comparison: ACR (oracle) vs Sieve (oracle_render) on ProPhoto patches."""
import sys, os, json, subprocess, colorsys, numpy as np
sys.path.insert(0, __file__.rsplit("/", 1)[0])
from oracle import *
from PIL import Image

SIEVE = HERE + "/../../target/release/examples/oracle_render"

# Patches: HSV hue sweep in *sRGB* terms converted to ProPhoto linear, 3 sats x 2 values.
SRGB_TO_XYZ = np.array([[0.4124564, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750], [0.0193339, 0.1191920, 0.9503041]])
BRAD = np.array([[1.0478112, 0.0228866, -0.0501270], [0.0295424, 0.9904844, -0.0170491], [-0.0092345, 0.0150436, 0.7521316]])
def srgb_to_pp(e):
    lin = np.where(e <= 0.04045, e / 12.92, ((e + 0.055) / 1.055) ** 2.4)
    return XYZ_TO_PP @ BRAD @ SRGB_TO_XYZ @ lin
vals = []
for v in [0.45, 0.8]:
    for s in [0.25, 0.5, 0.8]:
        for h in range(0, 360, 15):
            e = np.array(colorsys.hsv_to_rgb(h / 360, s, v))
            vals.append(np.clip(srgb_to_pp(e), 0, 1) * 0.55)
for g in [0.02, 0.05, 0.1, 0.2, 0.4, 0.7]:
    vals.append(np.array([g, g, g]))
img16, centres = patches(np.array(vals))
h, w, _ = img16.shape


def sieve(over):
    p_in = CACHE + "/sieve_in.rgb16"
    img16.astype("<u2").tofile(p_in)
    js = CACHE + "/sieve_adj.json"
    json.dump(over, open(js, "w"))
    out = CACHE + "/sieve_out.ppm"
    subprocess.run([SIEVE, p_in, str(w), str(h), js, out], check=True)
    return np.asarray(Image.open(out).convert("RGB"))


def lab(rgb):
    lin = srgb_decode(rgb / 255.0)
    xyz = lin @ SRGB_TO_XYZ.T / np.array([0.95047, 1.0, 1.08883])
    f = np.where(xyz > 216 / 24389, np.cbrt(xyz), (24389 / 27 * xyz + 16) / 116)
    return np.stack([116 * f[:, 1] - 16, 500 * (f[:, 0] - f[:, 1]), 200 * (f[:, 1] - f[:, 2])], 1)


def compare(name, crs, over):
    a = sample(render(img16, neutral_crs(**crs)), centres)
    b = sample(sieve(over), centres)
    a0 = sample(render(img16, neutral_crs()), centres)
    b0 = sample(sieve({}), centres)
    la, lb, la0, lb0 = lab(a), lab(b), lab(a0), lab(b0)
    de = np.linalg.norm(la - lb, axis=1)
    eff_a = la - la0
    eff_b = lb - lb0
    # Ratio of effect magnitudes (Sieve/ACR) where ACR's effect is visible.
    ma, mb = np.linalg.norm(eff_a, axis=1), np.linalg.norm(eff_b, axis=1)
    sel = ma > 2
    ratio = np.median(mb[sel] / ma[sel]) if sel.any() else float("nan")
    cos = np.median((eff_a[sel] * eff_b[sel]).sum(1) / (ma[sel] * mb[sel] + 1e-9)) if sel.any() else float("nan")
    print("%-22s dE76 mean %5.2f max %5.2f | effect ratio %.2f dir-cos %.2f | acr effect mean %.2f" % (name, de.mean(), de.max(), ratio, cos, ma.mean()))
    return la, lb, la0, lb0


if __name__ == "__main__":
    tests = [
        ("baseline", {}, {}),
        ("RedSat+20", {"RedSaturation": 20}, {"calibration": {"red": {"hue": 0, "saturation": 20}}}),
        ("GreenSat-25", {"GreenSaturation": -25}, {"calibration": {"green": {"hue": 0, "saturation": -25}}}),
        ("BlueHue-10", {"BlueHue": -10}, {"calibration": {"blue": {"hue": -10, "saturation": 0}}}),
        ("BlueSat+20", {"BlueSaturation": 20}, {"calibration": {"blue": {"hue": 0, "saturation": 20}}}),
        ("RedHue+50", {"RedHue": 50}, {"calibration": {"red": {"hue": 50, "saturation": 0}}}),
        ("GreenHue+50", {"GreenHue": 50}, {"calibration": {"green": {"hue": 50, "saturation": 0}}}),
        ("Vibrance+25", {"Vibrance": 25}, {"vibrance": 25}),
        ("Vibrance+75", {"Vibrance": 75}, {"vibrance": 75}),
        ("Saturation+25", {"Saturation": 25}, {"saturation": 25}),
        ("Saturation-50", {"Saturation": -50}, {"saturation": -50}),
        ("HSLsatGreen-40", {"SaturationAdjustmentGreen": -40}, {"hsl": {"saturation": {"green": -40}}}),
        ("HSLsatYellow-20", {"SaturationAdjustmentYellow": -20}, {"hsl": {"saturation": {"yellow": -20}}}),
        ("HSLhueRed+15", {"HueAdjustmentRed": 15}, {"hsl": {"hue": {"red": 15}}}),
        ("HSLhueYellow-20", {"HueAdjustmentYellow": -20}, {"hsl": {"hue": {"yellow": -20}}}),
        ("HSLhueGreen+50", {"HueAdjustmentGreen": 50}, {"hsl": {"hue": {"green": 50}}}),
        ("HSLlumOrange-30", {"LuminanceAdjustmentOrange": -30}, {"hsl": {"luminance": {"orange": -30}}}),
        ("HSLlumBlue-50", {"LuminanceAdjustmentBlue": -50}, {"hsl": {"luminance": {"blue": -50}}}),
        ("GradeShadow30/50", {"SplitToningShadowHue": 30, "SplitToningShadowSaturation": 50, "ColorGradeBlending": 100},
         {"colorGrading": {"shadows": {"hue": 30, "saturation": 50, "luminance": 0}, "blending": 100}}),
        ("GradeHigh30/50", {"SplitToningHighlightHue": 30, "SplitToningHighlightSaturation": 50, "ColorGradeBlending": 100},
         {"colorGrading": {"highlights": {"hue": 30, "saturation": 50, "luminance": 0}, "blending": 100}}),
        ("GradeMid185/50", {"ColorGradeMidtoneHue": 185, "ColorGradeMidtoneSat": 50, "ColorGradeBlending": 100},
         {"colorGrading": {"midtones": {"hue": 185, "saturation": 50, "luminance": 0}, "blending": 100}}),
        ("GradeGlobal220/50", {"ColorGradeGlobalHue": 220, "ColorGradeGlobalSat": 50},
         {"colorGrading": {"global": {"hue": 220, "saturation": 50, "luminance": 0}}}),
        ("GradeMidLum+50", {"ColorGradeMidtoneLum": 50}, {"colorGrading": {"midtones": {"hue": 0, "saturation": 0, "luminance": 50}}}),
        ("GrayMix", {"ConvertToGrayscale": "True"}, {"blackAndWhite": {"enabled": True}}),
        ("GrayMixRed+50", {"ConvertToGrayscale": "True", "GrayMixerRed": 50}, {"blackAndWhite": {"enabled": True, "mixer": {"red": 50}}}),
        ("ShadowTint+50", {"ShadowTint": 50}, {"calibration": {"shadowTint": 50}}),
    ]
    sel = sys.argv[1:]
    for name, crs, over in tests:
        if sel and name not in sel:
            continue
        compare(name, crs, over)
