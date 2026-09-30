import re, zlib, struct, hashlib, itertools, sys

s = open(sys.argv[1]).read()
tabs = dict(re.findall(r'crs:Table_(\w+)="([^"]*)"', s))
base = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?"
tail = "()[]{}@%$#"
for md5, val in tabs.items():
    print("table", md5, len(val), sorted(set(val) - set(base + tail)))
    for perm in itertools.permutations("`'|"):
        alpha = base + ''.join(perm) + tail
        idx = {c: i for i, c in enumerate(alpha)}
        for lsd in (True, False):
            out = bytearray()
            ok = True
            for i in range(0, len(val), 5):
                g = val[i:i + 5]
                try:
                    d = [idx[c] for c in g]
                except KeyError:
                    ok = False
                    break
                v = 0
                if lsd:
                    for k in reversed(range(len(d))):
                        v = v * 85 + d[k]
                else:
                    for x in d:
                        v = v * 85 + x
                n = len(g) - 1 if len(g) < 5 else 4
                out += struct.pack('<I', v & 0xffffffff)[:n]
            if not ok:
                continue
            try:
                ln = struct.unpack('<I', out[:4])[0]
                raw = zlib.decompress(bytes(out[4:]))
            except Exception:
                continue
            print(md5, perm, lsd, ln, len(raw), hashlib.md5(raw).hexdigest().upper(), raw[:64].hex())
            if len(sys.argv) > 2:
                open(sys.argv[2] + "/" + md5 + ".bin", "wb").write(raw)
