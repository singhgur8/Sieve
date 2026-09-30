"""Evaluates several variants (eval.sh with SIEVE_DUMP=1) and prints diff.py's summary lines.

    python3 diag_variants.py <list> <variant,...> [--bins]
"""
import os, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import diff  # noqa: E402

P2 = os.environ.get("P2", "/Users/gurjotsingh/Documents/GitHub/Sieve/test-data/p2")
lst, vs = sys.argv[1], sys.argv[2].split(",")
bins = "--bins" in sys.argv
env = dict(os.environ, SIEVE_DUMP="1")
for v in vs:
    subprocess.run([os.path.join(HERE, "eval.sh"), lst, v], env=env, capture_output=True)
    d = os.path.join(P2, "out", "orig" if v == "-" else v)
    print("==", v)
    stems = [os.path.splitext(os.path.basename(l.strip()))[0] for l in open(lst) if l.strip()]
    for s in stems:
        if os.path.exists(os.path.join(d, s + ".sieve.ppm")):
            diff.analyse(d, s, bins=bins)
