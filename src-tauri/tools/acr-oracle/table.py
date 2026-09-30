"""Runs eval.sh for several variants and prints a per-image dE2000-mean table.

    python3 table.py <list> <variant,...> [parity_eval args...]
"""
import os, re, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
lst, vs = sys.argv[1], sys.argv[2].split(",")
extra = sys.argv[3:]
rows, order = {}, []
for v in vs:
    out = subprocess.run([os.path.join(HERE, "eval.sh"), lst, v] + extra, capture_output=True, text=True).stdout
    for line in out.splitlines():
        m = re.match(r"(\S+) dE mean ([\d.]+) p95 ([\d.]+)", line)
        if m:
            if m.group(1) not in rows:
                rows[m.group(1)] = {}
                order.append(m.group(1))
            rows[m.group(1)][v] = float(m.group(2))
print("%-10s" % "image" + "".join("%10s" % v[:10] for v in vs))
for s in order:
    print("%-10s" % s + "".join("%10s" % ("%.2f" % rows[s][v] if v in rows[s] else "-") for v in vs))
for fmt in ("AZA", "IMG", "DSCF", ""):
    ss = [s for s in order if s.startswith(fmt)]
    if not ss:
        continue
    means = []
    for v in vs:
        vals = [rows[s][v] for s in ss if v in rows[s]]
        means.append(sum(vals) / len(vals) if vals else float("nan"))
    print("%-10s" % ("mean " + (fmt or "all")) + "".join("%10.2f" % m for m in means))
