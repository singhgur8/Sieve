import struct, sys
b = open(sys.argv[1], 'rb').read()
t, ver, h, s, v = struct.unpack('<5I', b[:20])
print("type", t, "ver", ver, "dims", h, s, v, "len", len(b))
n = h * s * v
d = struct.unpack('<%df' % (3 * n), b[20:20 + 12 * n])
rest = b[20 + 12 * n:]
print("rest", rest.hex())
cols = [d[i::3] for i in range(3)]
for c in cols:
    print(min(c), max(c), sum(c) / len(c))
# print a slice: val index vi, hue 0.., sat
for vi in (0, v // 2, v - 1):
    for hi in (0, 3, 9):
        row = []
        for si in range(s):
            k = (vi * h + hi) * s + si
            row.append("%.2f/%.2f/%.2f" % (d[3 * k], d[3 * k + 1], d[3 * k + 2]))
        print(vi, hi, " ".join(row))
