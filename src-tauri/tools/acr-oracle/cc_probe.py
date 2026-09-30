"""Probe: how does Adobe derive the per-unit CameraCalibration (CC) of a raw file?
Hypothesis: CC = camera preset neutral / ColorMatrix-predicted neutral at the preset's
illuminant (reference unit). Prints the implied CC per preset for comparison with the DNG's.
"""
import numpy as np

A = (0.44757, 0.40745)
D65 = (0.31271, 0.32902)


def xy_to_xyz(x, y):
    return np.array([x / y, 1.0, (1 - x - y) / y])


def cct_xy(t):  # CIE daylight locus (4000..25000 K)
    if t <= 7000:
        x = -4.607e9 / t ** 3 + 2.9678e6 / t ** 2 + 0.09911e3 / t + 0.244063
    else:
        x = -2.0064e9 / t ** 3 + 1.9018e6 / t ** 2 + 0.24748e3 / t + 0.23704
    return x, -3.0 * x * x + 2.87 * x - 0.275


def planck_xy(t):
    # Kim et al. cubic spline approximation of the Planckian locus.
    if t <= 4000:
        x = -0.2661239e9 / t ** 3 - 0.2343589e6 / t ** 2 + 0.8776956e3 / t + 0.179910
    else:
        x = -3.0258469e9 / t ** 3 + 2.1070379e6 / t ** 2 + 0.2226347e3 / t + 0.240390
    if t <= 2222:
        y = -1.1063814 * x ** 3 - 1.34811020 * x ** 2 + 2.18555832 * x - 0.20219683
    elif t <= 4000:
        y = -0.9549476 * x ** 3 - 1.37418593 * x ** 2 + 2.09137015 * x - 0.16748867
    else:
        y = 3.0817580 * x ** 3 - 5.87338670 * x ** 2 + 3.75112997 * x - 0.37001483
    return x, y


def interp_cm(cm1, cm2, t, t1=2856.0, t2=6504.0):
    if t <= t1:
        return cm1
    if t >= t2:
        return cm2
    g = (1 / t - 1 / t2) / (1 / t1 - 1 / t2)
    return g * cm1 + (1 - g) * cm2


def neutral(cm, xy):
    n = cm @ xy_to_xyz(*xy)
    return n / n[1]


CAMS = {
    "Sony ILCE-7M4": dict(
        cm1=[0.8784, -0.4791, 0.1177, -0.3468, 1.0693, 0.3213, 0.0009, 0.0507, 0.7395],
        cm2=[0.746, -0.2365, -0.0588, -0.5687, 1.3442, 0.2474, -0.0624, 0.1156, 0.6584],
        cc=(0.9488, 1.0161),
        presets={"Daylight": (2541, 1612), "Cloudy": (2741, 1478), "Tungsten": (1578, 2908), "Flash": (2798, 1435),
                 "4500K": (2311, 1770), "6000K": (2684, 1476), "8500K": (3048, 1245), "3200K": (1822, 2401),
                 "2500K": (1451, 3244), "Shade": (2977, 1306)},
        norm=1024),
}


def main():
    import subprocess, re
    dng = "/Users/gurjotsingh/Documents/GitHub/Sieve/test-data/p2/dng/"
    for name, f, presets, norm in [
        ("Canon", "IMG_5767.dng", {"Daylight": (1820, 1596)}, 1024),
        ("Canon5627", "IMG_5627.dng", {"Daylight": (1783, 1601)}, 1024),
        ("Fuji", "DSCF5902.dng", {"StdA": (356 / 302 * 1024, 877 / 302 * 1024), "D65": (605 / 302 * 1024, 502 / 302 * 1024)}, 1024),
        ("Sony", "AZA06911.dng", CAMS["Sony ILCE-7M4"]["presets"], 1024),
    ]:
        out = subprocess.run(["exiftool", "-m", "-s3", "-ColorMatrix1", "-ColorMatrix2", "-CameraCalibration1"], capture_output=True, text=True, cwd=dng, input=None, args=None) if False else None
        o = subprocess.run(["exiftool", "-m", "-s3", "-ColorMatrix1", "-ColorMatrix2", "-CameraCalibration1", dng + f], capture_output=True, text=True).stdout.split("\n")
        cm1 = np.array([float(v) for v in o[0].split()]).reshape(3, 3)
        cm2 = np.array([float(v) for v in o[1].split()]).reshape(3, 3)
        cc = [float(v) for v in o[2].split()]
        print("==", name, "DNG CC R %.4f B %.4f" % (cc[0], cc[8]))
        for p, (r, b) in presets.items():
            ncam = np.array([norm / r, 1.0, norm / b])
            for label, xy in [("D65", D65), ("D55", cct_xy(5503)), ("D50", cct_xy(5003)), ("A", A),
                              ("P5500", planck_xy(5500)), ("P5200", planck_xy(5200))]:
                # temperature for interpolation ~ label
                t = {"D65": 6504, "D55": 5503, "D50": 5003, "A": 2856, "P5500": 5500, "P5200": 5200}[label]
                nref = neutral(interp_cm(cm1, cm2, t), xy)
                ccr = ncam / nref
                print("  %-9s @%-5s implied CC R %.4f B %.4f" % (p, label, ccr[0], ccr[2]))


if __name__ == "__main__":
    main()
