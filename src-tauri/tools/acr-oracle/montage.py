"""Side-by-side montage of PPM/JPEG images (height H) for visual checks.

    python3 montage.py <out.jpg> <height> <img>...
"""
import sys
from PIL import Image

out, h = sys.argv[1], int(sys.argv[2])
ims = []
for p in sys.argv[3:]:
    im = Image.open(p).convert("RGB")
    w = round(im.width * h / im.height)
    ims.append(im.resize((w, h), Image.LANCZOS))
W = sum(i.width for i in ims) + 6 * (len(ims) - 1)
m = Image.new("RGB", (W, h), (30, 30, 30))
x = 0
for i in ims:
    m.paste(i, (x, 0))
    x += i.width + 6
m.save(out, quality=90)
