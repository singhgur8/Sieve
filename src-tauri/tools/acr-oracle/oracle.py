"""ACR oracle: writes synthetic linear DNGs (camera space = linear ProPhoto, D50) with
embedded crs: settings, renders them with Adobe DNG Converter (Camera Raw engine, full-size
sRGB preview) and reads the result back. Local calibration tool (test-data/, not shipped).
"""
import os, struct, subprocess, sys, tempfile, hashlib
import numpy as np
from PIL import Image

CONV = "/Applications/Adobe DNG Converter.app/Contents/MacOS/Adobe DNG Converter"
HERE = os.path.dirname(os.path.abspath(__file__))
CACHE = os.path.join(HERE, "oracle-cache")

# ProPhoto (ROMM) <-> XYZ D50
PP_TO_XYZ = np.array([[0.7976749, 0.1351917, 0.0313534], [0.2880402, 0.7118741, 0.0000857], [0.0, 0.0, 0.8252100]])
XYZ_TO_PP = np.linalg.inv(PP_TO_XYZ)


def srat(v):
    n = int(round(v * 1000000))
    return struct.pack("<ii", n, 1000000)


def xmp_packet(crs, seqs=None, extra=""):
    attrs = "\n".join('   crs:%s="%s"' % (k, v) for k, v in crs.items())
    seq_xml = ""
    for k, items in (seqs or {}).items():
        lis = "".join("<rdf:li>%s</rdf:li>" % i for i in items)
        seq_xml += "   <crs:%s><rdf:Seq>%s</rdf:Seq></crs:%s>\n" % (k, lis, k)
    return ('<x:xmpmeta xmlns:x="adobe:ns:meta/">\n <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">\n'
            '  <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"\n%s>\n%s%s'
            '  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>' % (attrs, seq_xml, extra)).encode()


def write_dng(path, img16, xmp, baseline=0.0, camera_matrix=None, forward=None, neutral=(1, 1, 1)):
    """img16: HxWx3 uint16 camera values (white 65535)."""
    h, w, _ = img16.shape
    data = img16.astype("<u2").tobytes()
    cm = camera_matrix if camera_matrix is not None else XYZ_TO_PP
    fm = forward if forward is not None else PP_TO_XYZ
    tags = []  # (tag, type, count, bytes)
    def add(tag, typ, count, b):
        tags.append((tag, typ, count, b))
    def ascii(s):
        b = s.encode() + b"\0"
        return len(b), b
    add(254, 4, 1, struct.pack("<I", 0))
    add(256, 4, 1, struct.pack("<I", w))
    add(257, 4, 1, struct.pack("<I", h))
    add(258, 3, 3, struct.pack("<HHH", 16, 16, 16))
    add(259, 3, 1, struct.pack("<H", 1))
    add(262, 3, 1, struct.pack("<H", 34892))
    n, b = ascii("Sieve"); add(271, 2, n, b)
    n, b = ascii("Oracle"); add(272, 2, n, b)
    add(273, 4, 1, None)  # strip offset, patched
    add(274, 3, 1, struct.pack("<H", 1))
    add(277, 3, 1, struct.pack("<H", 3))
    add(278, 4, 1, struct.pack("<I", h))
    add(279, 4, 1, struct.pack("<I", len(data)))
    add(284, 3, 1, struct.pack("<H", 1))
    add(700, 1, len(xmp), xmp)
    add(50706, 1, 4, bytes([1, 4, 0, 0]))
    add(50707, 1, 4, bytes([1, 1, 0, 0]))
    n, b = ascii("Sieve Oracle"); add(50708, 2, n, b)
    add(50714, 4, 3, struct.pack("<III", 0, 0, 0))
    add(50717, 4, 3, struct.pack("<III", 65535, 65535, 65535))
    add(50721, 10, 9, b"".join(srat(v) for v in np.asarray(cm).flatten()))
    add(50728, 5, 3, b"".join(struct.pack("<II", int(v * 1000000), 1000000) for v in neutral))
    add(50730, 10, 1, srat(baseline))
    add(50778, 3, 1, struct.pack("<H", 23))
    n, b = ascii("Oracle"); add(50936, 2, n, b)
    add(50964, 10, 9, b"".join(srat(v) for v in np.asarray(fm).flatten()))
    tags.sort(key=lambda t: t[0])
    ifd_off = 8
    ifd_size = 2 + 12 * len(tags) + 4
    blob_off = ifd_off + ifd_size
    blobs = b""
    entries = b""
    strip_entry_index = None
    for tag, typ, count, b in tags:
        if b is None:
            b = b"\0\0\0\0"
        if len(b) <= 4:
            entries += struct.pack("<HHI", tag, typ, count) + b.ljust(4, b"\0")
        else:
            entries += struct.pack("<HHI", tag, typ, count) + struct.pack("<I", blob_off + len(blobs))
            blobs += b
            if len(blobs) % 2:
                blobs += b"\0"
        if tag == 273:
            strip_entry_index = len(entries) - 4
    data_off = blob_off + len(blobs)
    head = b"II" + struct.pack("<HI", 42, ifd_off)
    ifd = struct.pack("<H", len(tags)) + entries + struct.pack("<I", 0)
    ifd = bytearray(ifd)
    ifd[2 + strip_entry_index - 0:2 + strip_entry_index + 4] = struct.pack("<I", data_off)
    with open(path, "wb") as f:
        f.write(head + bytes(ifd) + blobs + data)


def render(img16, crs, seqs=None, extra="", baseline=0.0, key=None, **kw):
    """Renders with ACR; returns HxWx3 uint8 sRGB (cached by content hash)."""
    os.makedirs(CACHE, exist_ok=True)
    xmp = xmp_packet(crs, seqs, extra)
    hsh = hashlib.md5(img16.tobytes() + xmp + repr((baseline, sorted(kw.items()) if kw else None)).encode()).hexdigest()[:16]
    name = key or hsh
    out_jpg = os.path.join(CACHE, name + ".jpg")
    if key is None and os.path.exists(out_jpg):
        return np.asarray(Image.open(out_jpg).convert("RGB"))
    with tempfile.TemporaryDirectory(dir=CACHE) as tmp:
        src = os.path.join(tmp, "in", "o.dng")
        os.makedirs(os.path.dirname(src))
        write_dng(src, img16, xmp, baseline=baseline, **kw)
        outdir = os.path.join(tmp, "out")
        os.makedirs(outdir)
        subprocess.run([CONV, "-c", "-p2", "-d", outdir, src], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False)
        jpg = subprocess.run(["exiftool", "-b", "-JpgFromRaw", os.path.join(outdir, "o.dng")], capture_output=True).stdout
        if not jpg:
            raise RuntimeError("no preview rendered")
        with open(out_jpg, "wb") as f:
            f.write(jpg)
    return np.asarray(Image.open(out_jpg).convert("RGB"))


def neutral_crs(**over):
    crs = {
        "Version": "17.5", "ProcessVersion": "15.4", "WhiteBalance": "As Shot", "Exposure2012": "0",
        "Contrast2012": "0", "Highlights2012": "0", "Shadows2012": "0", "Whites2012": "0", "Blacks2012": "0",
        "Texture": "0", "Clarity2012": "0", "Dehaze": "0", "Vibrance": "0", "Saturation": "0",
        "Sharpness": "0", "LuminanceSmoothing": "0", "ColorNoiseReduction": "0",
        "CameraProfile": "Embedded", "HasSettings": "True", "ToneCurveName2012": "Linear",
    }
    crs.update({k: str(v) for k, v in over.items()})
    return crs


def srgb_decode(e):
    e = np.asarray(e, dtype=np.float64)
    return np.where(e <= 0.04045, e / 12.92, ((e + 0.055) / 1.055) ** 2.4)


def patches(values, size=48, cols=32):
    """values: Nx3 linear camera (0..1) -> image of patches, and centre coordinates."""
    n = len(values)
    rows = (n + cols - 1) // cols
    img = np.zeros((rows * size, cols * size, 3), np.float64)
    centres = []
    for i, v in enumerate(values):
        r, c = divmod(i, cols)
        img[r * size:(r + 1) * size, c * size:(c + 1) * size] = v
        centres.append((r * size + size // 2, c * size + size // 2))
    return (np.clip(img, 0, 1) * 65535 + 0.5).astype(np.uint16), centres


def sample(out, centres, r=4):
    return np.array([out[y - r:y + r, x - r:x + r].reshape(-1, 3).mean(0) for y, x in centres])
