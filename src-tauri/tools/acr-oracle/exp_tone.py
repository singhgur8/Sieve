import sys, numpy as np
sys.path.insert(0, __file__.rsplit("/", 1)[0])
from oracle import *

ev = np.linspace(-16, 0, 257)
lin = 2.0 ** ev
vals = np.stack([lin, lin, lin], 1)
img, cen = patches(vals)
out = render(img, neutral_crs())
s = sample(out, cen)
print("gray check (max channel spread):", np.abs(s[:, 0] - s[:, 2]).max())
y = srgb_decode(s[:, 1] / 255.0)
np.save(HERE + "/tone_default.npy", np.stack([lin, y], 1))
for e, x, o, enc in zip(ev[::8], lin[::8], y[::8], s[::8, 1]):
    print("%6.2f  in %.6f  out %.6f  enc %.1f" % (e, x, o, enc))
