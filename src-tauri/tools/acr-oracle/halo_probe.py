"""Does Camera Raw's Shadows / Highlights halo at a strong step edge? Renders a synthetic
linear DNG (left half `dark`, right half `bright`) with the oracle and prints the green
channel next to the edge vs far from it.

    python3 halo_probe.py
"""
import os, sys
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import oracle

W, H = 1536, 512
for dark, bright in [(0.004, 0.6), (0.02, 0.3)]:
    img = np.zeros((H, W, 3))
    img[:, : W // 2] = dark
    img[:, W // 2:] = bright
    img16 = (img * 65535).astype(np.uint16)
    print("== step %.3f | %.3f (%.1f EV)" % (dark, bright, np.log2(bright / dark)))
    for name, over in [("neutral", {}), ("S+100", {"Shadows2012": 100}), ("H-100", {"Highlights2012": -100}),
                       ("S+64 H-66", {"Shadows2012": 64, "Highlights2012": -66})]:
        out = oracle.render(img16, oracle.neutral_crs(**over))
        g = out[H // 2, :, 1].astype(int)
        e = W // 2
        cols = [e - 400, e - 100, e - 30, e - 10, e - 3, e + 2, e + 10, e + 30, e + 100, e + 400]
        print("  %-9s " % name + " ".join("%d:%d" % (c - e, g[c]) for c in cols))
